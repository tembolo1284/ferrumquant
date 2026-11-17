"""Commodity tests for the ferrumquant Python module and the fqview book.

Three layers:
  * exchange date rules checked against the hand-verified values in the
    Rust unit tests (NYMEX, CME/ICE oil, NERC/ISO hours, LME prompts,
    CBOT/ICE ags);
  * pricing identities that must hold whatever the numbers are
    (put-call parity, par swaps, intrinsic <= option <= upper bound,
    Kirk vs Bjerksund-Stensland, Monte Carlo vs Black within error);
  * the fqview commodities book building and pricing end to end.

Run from the repo root with ``pytest python/tests`` after
``./build.sh --py build``.
"""

from __future__ import annotations

import datetime as dt
import math

import pytest

import ferrumquant as fq
from fqview import CommodBook, sample_commod_market, sample_commod_positions

REF = fq.Date(2026, 11, 9)


def d(y, m, dd):
    return fq.Date(y, m, dd)


def months(n=36, start=(2026, 12)):
    return [fq.ContractMonth(*start).add_months(i) for i in range(n)]


@pytest.fixture(scope="module")
def disc():
    return fq.DiscountCurve.flat(REF, 0.04, d(2036, 12, 31))


@pytest.fixture(scope="module")
def gas():
    ms = months()
    return fq.FuturesCurve(fq.gas_hub("HH"), REF, [(m, 3.0 + (1.0 if m.is_winter else 0.0) + 0.01 * i) for i, m in enumerate(ms)])


@pytest.fixture(scope="module")
def gas_vol():
    return fq.SeasonalVol([(m, 0.6 if m.is_winter else 0.4) for m in months()])


@pytest.fixture(scope="module")
def model():
    return fq.ClewlowStrickland(0.9, 2.5, 0.25, 0.3)


# ---------------------------------------------------------------------------
# Core: months, curves, options
# ---------------------------------------------------------------------------


def test_contract_month_parsing_forms():
    m = fq.ContractMonth(2027, 1)
    assert fq.ContractMonth.from_any("F27") == m
    assert fq.ContractMonth.from_any("2027-01") == m
    assert fq.ContractMonth.from_any((2027, 1)) == m
    assert fq.ContractMonth.from_any(d(2027, 1, 15)) == m
    assert fq.ContractMonth.from_any(dt.date(2027, 1, 3)) == m
    assert m.code == "F27" and m.is_winter and m.next().code == "G27"
    assert m.months_until(fq.ContractMonth(2027, 12)) == 11
    assert fq.ContractMonth.from_any("Q27").month == 8  # Q is August
    with pytest.raises(fq.FerrumError):
        fq.ContractMonth.from_any("XX27")


def test_futures_curve_queries(gas):
    assert gas.front_month(REF) == fq.ContractMonth(2026, 12)
    assert gas.price("F27") == pytest.approx(4.01)
    f, g, h = (gas.price(c) for c in ("F27", "G27", "H27"))
    assert gas.average("F27", "H27") == pytest.approx((f + g + h) / 3)
    strip = gas.strip_price(d(2027, 1, 1), d(2027, 3, 31))
    assert strip == pytest.approx((31 * f + 28 * g + 31 * h) / 90)
    assert gas.calendar_spread("H27", "F27") == pytest.approx(f - h)
    assert gas.shifted(0.5).price("F27") == pytest.approx(4.51)
    assert gas.bumped_month("F27", 0.1).price("G27") == pytest.approx(4.02)


def test_futures_option_parity_and_implied_vol(gas, gas_vol, disc):
    call = fq.FuturesOption("call", "F27", 4.0, d(2026, 12, 29), 1.0).value(gas, gas_vol, disc)
    put = fq.FuturesOption("put", "F27", 4.0, d(2026, 12, 29), 1.0).value(gas, gas_vol, disc)
    assert call["vol"] == 0.6
    assert call["forward_price"] - put["forward_price"] == pytest.approx(gas.price("F27") - 4.0, abs=1e-12)
    assert call["pv"] - put["pv"] == pytest.approx((gas.price("F27") - 4.0) * call["discount"], abs=1e-12)
    iv = fq.FuturesOption("call", "F27", 4.0, d(2026, 12, 29), 1.0).implied_vol(gas, call["forward_price"])
    assert iv == pytest.approx(0.6, abs=1e-8)
    # A flat number works where a SeasonalVol does.
    flat = fq.FuturesOption("call", "F27", 4.0, d(2026, 12, 29), 1.0).value(gas, 0.6, disc)
    assert flat["pv"] == pytest.approx(call["pv"])


def test_spread_option_methods_agree_near_zero_strike():
    kirk = fq.spread_price("call", 50.0, 45.0, 0.3, 0.3, 0.8, 0.5, 0.0, method="kirk")
    bs = fq.spread_price("call", 50.0, 45.0, 0.3, 0.3, 0.8, 0.5, 0.0, method="bs")
    margrabe = fq.spread_price("call", 50.0, 45.0, 0.3, 0.3, 0.8, 0.5, 0.0, method="margrabe")
    assert kirk == pytest.approx(margrabe, rel=1e-12)
    assert bs == pytest.approx(margrabe, rel=1e-12)
    price = fq.spread_price("call", 50.0, 45.0, 0.3, 0.3, 0.8, 0.5, 2.0)
    assert fq.implied_correlation("call", 50.0, 45.0, 0.3, 0.3, 0.5, 2.0, price) == pytest.approx(0.8, abs=1e-6)


def test_asian_moment_matching_collapses_to_black(gas, disc):
    expiry = d(2027, 1, 15)
    one = fq.AsianOption("call", 4.0, [(expiry, "G27", None)], expiry, 1.0).value(gas, 0.5, disc)
    euro = fq.FuturesOption("call", "G27", 4.0, expiry, 1.0).value(gas, 0.5, disc)
    assert one["forward_price"] == pytest.approx(euro["forward_price"], rel=1e-10)
    fixings = fq.AsianOption.daily_fixings(d(2027, 1, 1), d(2027, 1, 31), "G27")
    assert len(fixings) == 31 and fixings[0][1] == fq.ContractMonth(2027, 2)


def test_clewlow_strickland_mc_matches_black(model):
    paths = model.simulate([(0.5, 4.0)], [0.1, 0.25], paths=40_000, seed=7)
    mean, se = paths.mean_forward(1, 0)
    assert abs(mean - 4.0) < 4 * se
    price, se = paths.european("call", 4.0, 1, 0)
    vol = model.black_vol(0.25, 0.5)
    # Black-76 with the model's own integrated vol, undiscounted.
    dplus = 0.5 * vol * math.sqrt(0.25)
    ncdf = lambda x: 0.5 * (1 + math.erf(x / math.sqrt(2)))  # noqa: E731
    black = 4.0 * (ncdf(dplus) - ncdf(-dplus))
    assert abs(price - black) < 4 * se


# ---------------------------------------------------------------------------
# Gas
# ---------------------------------------------------------------------------


def test_nymex_dates():
    cal = fq.NymexCalendar()
    assert cal.expiry("F27") == d(2026, 12, 29)
    assert cal.expiry("G27") == d(2027, 1, 27)
    assert cal.expiry("M27") == d(2027, 5, 26)
    bw = cal.bidweek("F27")
    assert bw[-1] == d(2026, 12, 29) and len(bw) == 5
    assert fq.gas_hubs()["HH"] == "Henry Hub"
    with pytest.raises(fq.FerrumError):
        fq.gas_hub("NOPE")


def test_gas_swap_par_is_fair(gas, disc):
    cal = fq.NymexCalendar()
    v = fq.GasSwap("HH", "F27", "H27", 10_000, 4.0).value(gas, disc, cal)
    assert v["total_quantity"] == pytest.approx(10_000 * 90)
    assert len(v["months"]) == 3 and v["months"][0]["days"] == 31
    par = fq.GasSwap("HH", "F27", "H27", 10_000, v["par_fixed"]).value(gas, disc, cal)
    assert par["pv"] == pytest.approx(0.0, abs=1e-6)
    rec = fq.GasSwap("HH", "F27", "H27", 10_000, 4.0, pay_fixed=False).value(gas, disc, cal)
    assert rec["pv"] == pytest.approx(-v["pv"])


def test_basis_swap_round_trip(gas, disc):
    cal = fq.NymexCalendar()
    tet = fq.FuturesCurve(fq.gas_hub("TETM3"), REF, [(m, p + 0.5 + (2.0 if m.is_winter else 0.0)) for m, p in gas.quotes()])
    basis = fq.BasisCurve.from_outrights(tet, gas)
    assert basis.basis("F27") == pytest.approx(2.5)
    assert basis.apply_to(gas).price("F27") == pytest.approx(tet.price("F27"))
    v = fq.GasBasisSwap("TETM3", "F27", "H27", 10_000, 1.0).value(basis, disc, cal)
    assert v["par_fixed"] == pytest.approx(2.5)
    assert v["pv"] > 0


def test_transport_bounds(gas, gas_vol, disc):
    cal = fq.NymexCalendar()
    tet = fq.FuturesCurve(fq.gas_hub("TETM3"), REF, [(m, p + 0.5 + (2.0 if m.is_winter else 0.0)) for m, p in gas.quotes()])
    tr = fq.TransportCapacity("HH", "TETM3", "F27", "Z27", 10_000, 0.02, 0.05, 0.3)
    v = tr.value(gas, tet, gas_vol, gas_vol, 0.9, disc, cal)
    assert len(v["months"]) == 12
    assert v["option_pv"] >= v["intrinsic_pv"] >= 0
    assert v["extrinsic_pv"] == pytest.approx(v["option_pv"] - v["intrinsic_pv"])
    assert v["net_pv"] == pytest.approx(v["option_pv"] - v["demand_pv"])
    # Perfect correlation with equal vols leaves little extrinsic on a deep ITM margin.
    tight = tr.value(gas, tet, 0.4, 0.4, 0.999, disc, cal)
    assert tight["extrinsic_pv"] < v["extrinsic_pv"]
    with pytest.raises(fq.FerrumError):
        fq.TransportCapacity("HH", "HH", "F27", "Z27", 10_000, 0.02, 0.05, 0.3)


def test_storage_intrinsic_and_rolling(gas, disc, model):
    cal = fq.NymexCalendar()
    fac = fq.StorageFacility("HH", "Z26", "X27", 1_000_000, 10_000, 20_000, 0.02, 0.02, 0.01)
    assert len(fac.months()) == 12
    intr = fac.intrinsic(gas, disc, cal)
    sched = intr["schedule"]
    inv = 0.0
    for s in sched:
        assert s["inventory_start"] == pytest.approx(inv)
        inv += s["change"]
        assert -1e-6 <= inv <= fac.capacity + 1e-6
    assert intr["intrinsic_pv"] >= 0
    assert intr["rolling_pv"] is None
    ri = fac.rolling_intrinsic(gas, model, disc, cal, paths=200, seed=3)
    assert ri["intrinsic_pv"] == pytest.approx(intr["intrinsic_pv"])
    assert ri["rolling_pv"] >= ri["intrinsic_pv"] - 3 * ri["rolling_std_err"]
    assert ri["extrinsic_pv"] == pytest.approx(ri["rolling_pv"] - ri["intrinsic_pv"])


def test_swing_ordering(gas, gas_vol, disc, model):
    cal = fq.NymexCalendar()
    sw = fq.SwingOption("HH", "F27", "H27", 4.0, 5_000, 15_000, 500_000, 900_000)
    v = sw.value(gas, gas_vol, disc, cal)
    assert len(v["intrinsic_takes"]) == 3
    assert 500_000 - 1e-6 <= sum(v["intrinsic_takes"]) <= 900_000 + 1e-6
    assert v["upper_bound_pv"] >= v["intrinsic_pv"]
    mc = sw.value_with_model(gas, gas_vol, model, disc, cal, paths=200, seed=5)
    assert mc["intrinsic_pv"] == pytest.approx(v["intrinsic_pv"])
    assert mc["rolling_pv"] is not None and mc["rolling_std_err"] > 0


# ---------------------------------------------------------------------------
# Power
# ---------------------------------------------------------------------------


def test_nerc_and_block_hours():
    assert fq.is_nerc_holiday(d(2027, 7, 5)) and not fq.is_nerc_holiday(d(2027, 7, 4))
    assert fq.is_nerc_holiday(d(2027, 12, 25)) and not fq.is_nerc_holiday(d(2027, 12, 27))
    assert len(fq.nerc_holidays(2027)) == 6
    assert fq.hours_in_day(d(2027, 3, 14)) == 23
    assert fq.hours_in_day(d(2027, 11, 7)) == 25
    assert fq.block_hours("PJM", "ATC", d(2027, 3, 1), d(2027, 3, 31)) == 31 * 24 - 1
    assert fq.block_hours("PJM", "PK", d(2027, 1, 1), d(2027, 1, 31)) == 20 * 16
    assert fq.block_hours("PJM", "OP", d(2027, 1, 1), d(2027, 1, 31)) == 744 - 320
    assert fq.block_hours("CAISO", "PK", d(2027, 1, 1), d(2027, 1, 31)) == 25 * 16
    assert fq.is_peak_hour(7) and fq.is_peak_hour(22) and not fq.is_peak_hour(23)
    assert not fq.is_peak_day("PJM", d(2027, 1, 2)) and fq.is_peak_day("CAISO", d(2027, 1, 2))
    assert fq.power_hubs()["PJMW"] == "PJM"


@pytest.fixture(scope="module")
def power():
    ms = months()
    pk = fq.FuturesCurve(fq.power_hub("PJMW"), REF, [(m, 45.0 + 15.0 * (m.month in (1, 2, 7, 8))) for m in ms])
    op = fq.FuturesCurve(fq.power_hub("PJMW"), REF, [(m, 30.0 + 5.0 * (m.month in (1, 2))) for m in ms])
    return fq.PowerCurve("PJMW", pk, op)


def test_power_curve_blocks(power):
    assert power.hub == "PJMW" and power.iso == "PJM"
    assert power.hours("F27", "PK") == 320
    atc = power.block_price("F27", "ATC")
    assert atc["hours"] == 744
    assert atc["price"] == pytest.approx((320 * 60.0 + 424 * 35.0) / 744)
    hours, price = power.period_price(d(2027, 1, 1), d(2027, 1, 31), "7x8")
    assert hours == 248 and price == pytest.approx(35.0)
    assert power.shifted_block("PK", 1.0).block_price("F27", "PK")["price"] == pytest.approx(61.0)
    hourly = power.shape("F27")
    assert len(hourly) == 744
    pk_avg = sum(h["price"] for h in hourly if h["peak"]) / sum(1 for h in hourly if h["peak"])
    assert pk_avg == pytest.approx(60.0, rel=1e-9)


def test_block_swap_par(power, disc):
    v = fq.BlockSwap("PJMW", "PK", "F27", "H27", 50, 55.0).value(power, disc)
    assert v["total_mwh"] == pytest.approx(50 * (320 + 320 + 368))
    par = fq.BlockSwap("PJMW", "PK", "F27", "H27", 50, v["par_fixed"]).value(power, disc)
    assert par["pv"] == pytest.approx(0.0, abs=1e-6)


def test_heat_rate_and_tolling(power, gas, gas_vol, disc):
    hro = fq.HeatRateOption("call", "PJMW", "PK", "F27", "Z27", 100, 7.5, 2.0)
    v = hro.value(power, gas, 0.5, gas_vol, 0.6, disc)
    assert len(v["months"]) == 12
    assert v["option_pv"] >= v["intrinsic_pv"] >= 0
    m = v["months"][0]
    assert m["implied_heat_rate"] == pytest.approx(60.0 / gas.price("F27"))
    assert m["forward_spark"] == pytest.approx(60.0 - 7.5 * gas.price("F27") - 2.0)
    toll = fq.TollingAgreement("PJMW", "PK", "F27", "Z27", 100, 7.5, 2.0, 3.0).value(power, gas, 0.5, gas_vol, 0.6, disc)
    assert toll["spark"]["option_pv"] == pytest.approx(v["option_pv"])
    assert toll["net_pv"] == pytest.approx(toll["spark"]["option_pv"] - toll["capacity_pv"])
    # $/kW-month on 100 MW for 12 months, undiscounted upper bound.
    assert toll["capacity_pv"] <= 3.0 * 100 * 1000 * 12


def test_dispatch_respects_frictions():
    unit = fq.ThermalUnit(500, 200, 7.0, 2.5, 20_000, 4, 4)
    assert unit.margin(60.0, 4.0) == pytest.approx(60.0 - 7.0 * 4.0 - 2.5)  # $/MWh
    prices = [30 + 40 * math.sin(h / 24 * 2 * math.pi) ** 2 for h in range(72)]
    r = unit.dispatch(prices, [4.0])
    assert len(r["schedule"]) == 72
    assert r["value"] <= r["frictionless_value"] + 1e-9
    assert r["hours_on"] == sum(1 for h in r["schedule"] if h["on"])
    assert r["starts"] == sum(1 for h in r["schedule"] if h["started"])
    free = fq.ThermalUnit(500, 200, 7.0, 2.5, 0.0, 1, 1).dispatch(prices, [4.0])
    assert free["value"] == pytest.approx(free["frictionless_value"])


# ---------------------------------------------------------------------------
# Oil
# ---------------------------------------------------------------------------


def test_oil_dates():
    mkt = fq.OilMarket()
    assert mkt.expiry("CL", "F27") == d(2026, 12, 21)
    assert mkt.expiry("CL", "G27") == d(2027, 1, 20)
    assert mkt.expiry("CL", "H27") == d(2027, 2, 22)
    assert mkt.expiry("BRN", "H27") == d(2027, 1, 29)
    assert mkt.expiry("BRN", "N27") == d(2027, 5, 28)
    assert mkt.expiry("RB", "G27") == d(2027, 1, 29)
    assert mkt.expiry("GAS", "G27") == d(2027, 2, 11)
    assert mkt.front_month("CL", d(2026, 12, 21)) == fq.ContractMonth(2027, 1)
    assert mkt.front_month("CL", d(2026, 12, 22)) == fq.ContractMonth(2027, 2)
    assert fq.oil_contracts()["RB"]["to_per_barrel"] == 42.0


@pytest.fixture(scope="module")
def oil_curves():
    ms = months()
    cl = fq.FuturesCurve(fq.oil_hub("CL"), REF, [(m, 70.0 - 0.2 * i) for i, m in enumerate(ms)])
    brn = fq.FuturesCurve(fq.oil_hub("BRN"), REF, [(m, 74.0 - 0.2 * i) for i, m in enumerate(ms)])
    rb = fq.FuturesCurve(fq.oil_hub("RB"), REF, [(m, 2.1 + 0.2 * (m.month in (5, 6, 7, 8))) for m in ms])
    ho = fq.FuturesCurve(fq.oil_hub("HO"), REF, [(m, 2.3) for m in ms])
    return cl, brn, rb, ho


def test_average_swap_rolls_and_par(oil_curves, disc):
    cl, *_ = oil_curves
    mkt = fq.OilMarket()
    v = fq.AverageSwap("CL", "F27", "H27", 100_000, 68.0).value(cl, disc, mkt)
    jan = v["months"][0]
    assert sum(n for _, n in jan["roll"]) == jan["pricing_days"]
    assert {c.code for c, _ in jan["roll"]} == {"G27", "H27"}
    par = fq.AverageSwap("CL", "F27", "H27", 100_000, v["par_fixed"]).value(cl, disc, mkt)
    assert par["pv"] == pytest.approx(0.0, abs=1e-6)
    with pytest.raises(fq.FerrumError):
        fq.AverageSwap("BRN", "F27", "H27", 1.0, 70.0).value(cl, disc, mkt)


def test_apo_parity_and_observed(oil_curves, disc):
    _, brn, *_ = oil_curves
    mkt = fq.OilMarket()
    call = fq.AveragePriceOption("call", "BRN", "F27", 70.0, 1.0).value(brn, 0.3, disc, mkt)
    put = fq.AveragePriceOption("put", "BRN", "F27", 70.0, 1.0).value(brn, 0.3, disc, mkt)
    assert call["fixings"] == 20 and call["observed"] == 0
    assert call["forward_price"] - put["forward_price"] == pytest.approx(call["forward_average"] - 70.0, abs=1e-10)
    fix = fq.AveragePriceOption("call", "BRN", "F27", 70.0, 1.0).fixings(mkt)
    assert len(fix) == 20 and all(p is None for _, _, p in fix)
    seen = [(dte, 80.0) for dte, _, _ in fix[:5]]
    part = fq.AveragePriceOption("call", "BRN", "F27", 70.0, 1.0, observed=seen).value(brn, 0.3, disc, mkt)
    assert part["observed"] == 5
    assert part["fixed_contribution"] == pytest.approx(80.0 * 5 / 20)
    assert part["pv"] > call["pv"]


def test_crack_options(oil_curves, disc):
    cl, _, rb, ho = oil_curves
    mkt = fq.OilMarket()
    co = fq.CrackOption("call", "RB", "CL", "H27", 20.0, 1.0)
    fwd = co.forward(rb, cl)
    assert fwd == pytest.approx(42.0 * rb.price("H27") - cl.price("H27"))
    v = co.value(rb, cl, 0.35, 0.3, 0.7, disc, mkt)
    assert v["forward_crack"] == pytest.approx(fwd)
    assert v["forward_price"] >= max(fwd - 20.0, 0.0)
    c321 = fq.Crack321("H27").value(rb, ho, cl, (0.35, 0.3, 0.3), 0.8, 0.7, disc, mkt)
    assert c321["forward_crack"] == pytest.approx((2 * 42.0 * rb.price("H27") + 42.0 * ho.price("H27") - 3 * cl.price("H27")) / 3)
    assert c321["option_pv"] is None
    opt = fq.Crack321("H27").value(rb, ho, cl, (0.35, 0.3, 0.3), 0.8, 0.7, disc, mkt, kind="call", strike=0.0, barrels=1.0)
    assert opt["option_forward_price"] >= opt["forward_crack"]
    with pytest.raises(fq.FerrumError):
        fq.CrackOption("call", "CL", "RB", "H27", 20.0, 1.0)


def test_calendar_spread_and_carry(oil_curves, disc, model):
    cl, brn, *_ = oil_curves
    mkt = fq.OilMarket()
    cso = fq.OilCalendarSpreadOption("call", "CL", "F27", "M27", 0.0, 1.0)
    v = cso.value(cl, 0.3, 0.95, disc, mkt)
    assert v["forward_spread"] == pytest.approx(cl.price("F27") - cl.price("M27"))
    assert v["correlation"] == 0.95
    vm = cso.value_with_model(cl, model, disc, mkt)
    assert 0.0 < vm["correlation"] < 1.0
    carry = fq.contango_carry("BRN", brn, "F27", "N27", 0.4, disc, mkt)
    assert carry["months"] == 6
    assert carry["spread"] == pytest.approx(brn.price("N27") - brn.price("F27"))
    assert carry["net_carry"] == pytest.approx(carry["spread"] - carry["storage_cost"] - carry["financing_cost"])


# ---------------------------------------------------------------------------
# Metals
# ---------------------------------------------------------------------------


def test_metal_codes_and_names():
    assert fq.metals()["XAU"]["is_precious"] and not fq.metals()["CA"]["is_precious"]
    assert fq.metal_hub("copper").code == fq.metal_hub("CA").code
    assert fq.metal_hub("aluminum").code == fq.metal_hub("AH").code
    with pytest.raises(fq.FerrumError):
        fq.metal_hub("unobtainium")


def test_precious_parity(disc):
    usd = fq.DiscountCurve.flat(REF, 0.045, d(2036, 12, 31))
    lease = fq.flat_lease_curve(REF, 0.005)
    pm = fq.PreciousMarket("XAU", REF, 2650.0, usd, lease)
    vd = d(2027, 11, 9)
    fwd = pm.forward(vd)
    assert fwd > 2650.0
    assert pm.gofo(vd) == pytest.approx(pm.usd_rate(vd) - 0.005, abs=2e-3)
    assert pm.implied_lease_rate(vd, fwd) == pytest.approx(0.005, abs=1e-9)
    assert pm.with_spot(2700.0).forward(vd) == pytest.approx(fwd * 2700.0 / 2650.0)
    f = fq.PreciousForward("XAU", 1000, fwd, vd).value(pm)
    assert f["pv"] == pytest.approx(0.0, abs=1e-6)
    call = fq.PreciousVanilla("call", "XAU", 1.0, 2700.0, vd).value(pm, 0.15)
    put = fq.PreciousVanilla("put", "XAU", 1.0, 2700.0, vd).value(pm, 0.15)
    assert call["pv"] - put["pv"] == pytest.approx((call["forward"] - 2700.0) * call["discount"], abs=1e-8)
    assert call["black"]["delta"] - put["black"]["delta"] == pytest.approx(1.0, abs=1e-10)


def test_lme_prompts():
    cal = fq.LmeCalendar()
    assert cal.cash_prompt(REF) == d(2026, 11, 11)
    assert cal.three_month_prompt(REF) == d(2027, 2, 9)
    assert cal.three_month_prompt(d(2026, 11, 13)) == d(2027, 2, 15)
    assert cal.three_month_prompt(d(2027, 7, 30)) == d(2027, 10, 29)
    assert cal.third_wednesday(2027, 3) == d(2027, 3, 17)
    assert cal.declaration_day(2027, 3) == d(2027, 3, 3)
    assert cal.classify_prompt("CA", REF, d(2026, 11, 11)) == "cash"
    assert cal.classify_prompt("CA", REF, d(2026, 11, 10)) is None
    assert cal.classify_prompt("CA", REF, d(2026, 12, 3)) == "daily"
    assert cal.classify_prompt("CA", REF, d(2027, 2, 9)) == "3m"
    kinds = {k for _, k in cal.prompts("CA", REF, 15)}
    assert kinds == {"cash", "daily", "3m", "weekly", "monthly"}
    assert not cal.is_valid_prompt("CA", REF, d(2026, 11, 10))


def test_lme_curve_and_instruments(disc):
    cal = fq.LmeCalendar()
    pr = [(dte, 9500.0 + 0.5 * i) for i, (dte, k) in enumerate(cal.prompts("CA", REF, 15)) if k in ("cash", "3m", "monthly")]
    cu = fq.LmeCurve("CA", REF, pr, cal)
    assert cu.metal == "CA" and cu.cash() == pytest.approx(9500.0)
    assert cu.is_contango()
    assert cu.cash_to_3m() == pytest.approx(cu.three_month() - cu.cash())
    assert cu.carry_bumped(10.0, cal).cash() == pytest.approx(cu.cash())
    assert cu.shifted(10.0, cal).cash() == pytest.approx(cu.cash() + 10.0)
    jun = cal.third_wednesday(2027, 6)
    fair = fq.LmeForward("CA", 25, cu.price(jun), jun).value(cu, disc, cal)
    assert fair["pv"] == pytest.approx(0.0, abs=1e-6)
    traded = cu.three_month() - cu.cash()
    carry = fq.LmeCarry("CA", 25, cu.cash_date, cu.cash(), cu.three_month_date, cu.three_month())
    assert carry.traded_carry == pytest.approx(traded)
    assert carry.value(cu, disc, cal) == pytest.approx(0.0, abs=1e-6)
    opt = fq.LmeOption("call", "CA", 25, 9600.0, 2027, 6).value(cu, 0.22, disc, cal)
    assert opt["expiry"] == d(2027, 6, 2) and opt["prompt"] == jun
    assert opt["forward"] == pytest.approx(cu.price(jun))
    with pytest.raises(fq.FerrumError):
        fq.LmeOption("call", "XAU", 25, 9600.0, 2027, 6)


# ---------------------------------------------------------------------------
# Ags
# ---------------------------------------------------------------------------


def test_ag_dates_and_listing():
    cal = fq.AgCalendar()
    assert cal.expiry("ZC", "Z26") == d(2026, 12, 14)
    assert cal.expiry("ZS", "F27") == d(2027, 1, 14)
    assert cal.expiry("ZW", "H27") == d(2027, 3, 12)
    assert cal.option_expiry("ZC", "H27") == d(2027, 2, 19)
    assert cal.expiry("KC", "H27") == d(2027, 3, 18)
    assert cal.expiry("CC", "H27") == d(2027, 3, 15)
    assert cal.expiry("OJ", "H27") == d(2027, 3, 10)
    assert cal.expiry("CT", "H27") == d(2027, 3, 5)
    assert cal.expiry("SB", "H27") == d(2027, 2, 26)
    assert cal.option_expiry("KC", "H27") == d(2027, 3, 17)
    assert fq.AgCalendar.is_good_friday(d(2027, 3, 26)) and not cal.is_business_day(d(2027, 3, 26))
    with pytest.raises(fq.FerrumError):
        cal.expiry("ZC", "J27")
    assert fq.ag_next_listed("ZC", "J27") == fq.ContractMonth(2027, 5)
    assert fq.ag_contracts()["ZC"]["listed_months"] == [3, 5, 7, 9, 12]
    assert fq.ag_lot_value("ZC", 450.0) == pytest.approx(22_500.0)


@pytest.fixture(scope="module")
def ag_curves():
    ms = months()
    zc = fq.FuturesCurve(fq.ag_hub("ZC"), REF, [(m, 440.0 + 2.0 * i) for i, m in enumerate(ms)])
    zs = fq.FuturesCurve(fq.ag_hub("ZS"), REF, [(m, 1050.0 + 1.0 * i) for i, m in enumerate(ms)])
    zm = fq.FuturesCurve(fq.ag_hub("ZM"), REF, [(m, 310.0) for m in ms])
    zl = fq.FuturesCurve(fq.ag_hub("ZL"), REF, [(m, 45.0) for m in ms])
    return zc, zs, zm, zl


def test_ag_option_and_implied_vol(ag_curves, disc):
    zc, *_ = ag_curves
    cal = fq.AgCalendar()
    opt = fq.AgOption("call", "ZC", "N27", 460.0, 10)
    v = opt.value(zc, 0.25, disc, cal)
    assert v["expiry"] == cal.option_expiry("ZC", "N27")
    assert v["pv"] == pytest.approx(v["black"]["price"] * v["discount"] * fq.ag_lot_value("ZC", 1.0) * 10)
    assert opt.implied_vol(zc, v["black"]["price"], cal) == pytest.approx(0.25, abs=1e-8)


def test_crush_and_spreads(ag_curves, disc):
    zc, zs, zm, zl = ag_curves
    cal = fq.AgCalendar()
    cr = fq.Crush("N27", 1.0).value(zs, zm, zl)
    beans = zs.price("N27") / 100.0
    assert cr["beans"] == pytest.approx(beans)
    assert cr["crush_margin"] == pytest.approx(cr["meal_value"] + cr["oil_value"] - beans)
    assert cr["oil_share"] == pytest.approx(cr["oil_value"] / (cr["meal_value"] + cr["oil_value"]))
    opt = fq.Crush("N27", 1.0).option("call", 0.0, zs, zm, zl, (0.2, 0.25, 0.3), 0.3, 0.7, disc, cal)
    assert opt["spot"]["crush_margin"] == pytest.approx(cr["crush_margin"])
    assert opt["forward_price"] >= cr["crush_margin"]
    fc = fq.full_carry("ZC", zc, "H27", "N27", 0.165, disc, cal)
    assert fc["full_carry"] == pytest.approx(fc["storage"] + fc["interest"])
    assert fc["percent_of_full_carry"] == pytest.approx(fc["spread"] / fc["full_carry"])
    with pytest.raises(fq.FerrumError):
        fq.full_carry("LE", zc, "G27", "J27", 0.1, disc, cal)
    cso = fq.AgCalendarSpread("put", "ZC", "H27", "N27", 0.0, 10).value(zc, 0.25, 0.9, disc, cal)
    assert cso["forward_spread"] == pytest.approx(zc.price("H27") - zc.price("N27"))
    assert cso["pv"] > 0


# ---------------------------------------------------------------------------
# fqview book
# ---------------------------------------------------------------------------


def test_commod_book_prices_every_line():
    mkt = sample_commod_market(REF, mc_paths=100)
    book = CommodBook(mkt, sample_commod_positions(REF))
    rows = book.rows()
    assert len(rows) == 18
    assert {r["sector"] for r in rows} == {"gas", "power", "oil", "metals", "ags"}
    for r in rows:
        assert math.isfinite(r["pv"]), r["label"]
        if r["intrinsic"] is not None:
            assert r["extrinsic"] == pytest.approx(r["pv"] - r["intrinsic"])
    # Pay-fixed gas swap gains when the curve goes up.
    swap = next(r for r in rows if r["product"] == "swap")
    assert swap["delta"] > 0
    t = book.totals(rows)
    assert t["pv"] == pytest.approx(sum(t["by_sector"].values()))
    text = book.format()
    assert "Commodities book" in text and "TOTAL" in text and "GAS" in text


def test_commod_market_shift_rebuilds_power():
    mkt = sample_commod_market(REF, mc_paths=50)
    up = mkt.shifted("PJMW:PK", 1.0)
    assert up.power["PJMW"].block_price("F27", "PK")["price"] == pytest.approx(mkt.power["PJMW"].block_price("F27", "PK")["price"] + 1.0)
    assert up.power["PJMW"].block_price("F27", "OP")["price"] == pytest.approx(mkt.power["PJMW"].block_price("F27", "OP")["price"])
