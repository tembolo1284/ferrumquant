"""Swap and swaption tests: OisSwap, Swaption, RatesBlotter.

Self-consistency (par swaps are worth zero, payer-receiver parity, implied
vol round trips, ladders sum to parallel DV01) plus QuantLib cross-checks:
the flat-curve goldens re-run live, and swaps re-priced on QuantLib's own
bootstrap of the same quote sheet.
"""

from __future__ import annotations

import pytest

import ferrumquant as fq
from fqview import CurveView, RatesBlotter, SwapPosition, SwaptionPosition, sample_quotes, sample_rates_book

try:
    import QuantLib as ql  # type: ignore
except ImportError:  # pragma: no cover
    ql = None

needs_ql = pytest.mark.skipif(ql is None, reason="QuantLib-Python not installed")

BP = 1e-4
TRADE = fq.Date(2026, 10, 13)


@pytest.fixture(scope="module")
def flat():
    today = fq.Date(2026, 9, 23)
    return today, fq.OvernightIndex.sofr(), fq.DiscountCurve.flat(today, 0.04, "2066-09-23")


@pytest.fixture(scope="module")
def cv():
    return CurveView(TRADE, sample_quotes())


# ---------------------------------------------------------------------------
# OisSwap
# ---------------------------------------------------------------------------


class TestOisSwap:
    def test_construction_and_schedule(self, flat):
        today, _, _ = flat
        s = fq.OisSwap.usd_sofr_spot(today, "5Y", "pay", 1e6, 0.04)
        assert s.side == "pay" and s.notional == 1e6 and s.fixed_rate == 0.04
        assert s.effective_date == fq.Date(2026, 9, 25)  # T+2
        assert s.maturity_date == fq.Date(2031, 9, 25)
        assert s.payment_lag == 2
        periods = s.periods()
        assert len(periods) == 5
        assert periods[0]["accrual_start"] == s.effective_date
        assert periods[-1]["accrual_end"] == s.maturity_date
        assert s.final_payment_date == periods[-1]["payment_date"]
        assert periods[0]["fixed_amount"] == pytest.approx(1e6 * 0.04 * periods[0]["fixed_year_fraction"])
        d = fq.OisSwap.usd_sofr("2027-10-15", "2032-10-15", "receive", 50e6, 0.039)
        assert d.side == "receive" and d.effective_date == fq.Date(2027, 10, 15)
        assert "OisSwap(" in repr(s)

    def test_sides_and_errors(self, flat):
        today, idx, c = flat
        p = fq.OisSwap.usd_sofr_spot(today, "5Y", "payer", 1e6, 0.04)
        r = fq.OisSwap.usd_sofr_spot(today, "5Y", "rec", 1e6, 0.04)
        assert p.npv(idx, c) == pytest.approx(-r.npv(idx, c), abs=1e-9)
        with pytest.raises(fq.FerrumError):
            fq.OisSwap.usd_sofr_spot(today, "5Y", "long", 1e6, 0.04)
        with pytest.raises(fq.FerrumError):
            fq.OisSwap.usd_sofr("2032-10-15", "2027-10-15", "pay", 1e6, 0.04)

    def test_par_swap_is_worth_zero(self, flat):
        today, idx, c = flat
        s = fq.OisSwap.usd_sofr_spot(today, "10Y", "pay", 1e6, 0.0)
        par = s.par_rate(idx, c)
        at_par = fq.OisSwap.usd_sofr_spot(today, "10Y", "pay", 1e6, par)
        v = at_par.value(idx, c)
        assert v.npv == pytest.approx(0.0, abs=1e-8)
        assert v.fixed_leg_pv == pytest.approx(v.float_leg_pv, abs=1e-8)
        assert v.par_rate == pytest.approx(par, abs=1e-15)
        assert v.annuity > 0

    def test_valuation_identities(self, flat):
        today, idx, c = flat
        s = fq.OisSwap.usd_sofr_spot(today, "5Y", "pay", 1e6, 0.04)
        v = s.value(idx, c)
        assert v.npv == pytest.approx(v.float_leg_pv - v.fixed_leg_pv, abs=1e-9)
        assert v.fixed_leg_pv == pytest.approx(0.04 * v.annuity, rel=1e-12)
        assert v.par_rate == pytest.approx(v.float_leg_pv / v.annuity, rel=1e-12)
        assert v.to_dict()["npv"] == v.npv
        assert s.value(idx, c, c).npv == v.npv  # explicit discount curve
        # Cashflows reconcile with the leg PVs.
        fixed_pv = sum(cf["amount"] * c.discount(cf["payment_date"]) for cf in s.fixed_cashflows())
        float_pv = sum(cf["amount"] * c.discount(cf["payment_date"]) for cf in s.float_cashflows(idx, c))
        assert fixed_pv == pytest.approx(v.fixed_leg_pv, rel=1e-12)
        assert float_pv == pytest.approx(v.float_leg_pv, rel=1e-12)

    def test_dv01_sign_and_size(self, flat):
        today, idx, c = flat
        p = fq.OisSwap.usd_sofr_spot(today, "10Y", "pay", 1e6, 0.04)
        dv01 = p.dv01(idx, c)
        assert dv01 > 0  # payer gains when rates rise
        assert dv01 == pytest.approx((p.npv(idx, c.shifted(BP)) - p.npv(idx, c.shifted(-BP))) / 2, rel=1e-12)
        # Roughly the annuity per bp (float leg sensitivity is small near par).
        assert dv01 == pytest.approx(p.value(idx, c).annuity * BP, rel=0.05)

    def test_dual_curve(self, flat):
        today, idx, c = flat
        s = fq.OisSwap.usd_sofr_spot(today, "5Y", "pay", 1e6, 0.04)
        disc = fq.DiscountCurve.flat(today, 0.045, "2066-09-23")
        v = s.value(idx, c, disc)
        single = s.value(idx, c)
        assert v.float_leg_pv < single.float_leg_pv and v.annuity < single.annuity
        # Floating amounts come from the projection curve only.
        assert [cf["amount"] for cf in s.float_cashflows(idx, c)] == pytest.approx([cf["amount"] for cf in s.float_cashflows(idx, c)])


# ---------------------------------------------------------------------------
# Swaption
# ---------------------------------------------------------------------------


class TestSwaption:
    def test_construction(self, flat):
        _, _, _ = flat
        sw = fq.Swaption.usd_sofr("2027-09-23", "10Y", "payer", 1e6, 0.04)
        assert sw.kind == "payer" and sw.strike == 0.04 and sw.expiry == fq.Date(2027, 9, 23)
        assert sw.underlying.effective_date == fq.Date(2027, 9, 27)
        assert sw.underlying.side == "pay"
        fwd = fq.OisSwap.usd_sofr("2027-10-15", "2037-10-15", "receive", 1e6, 0.04)
        assert fq.Swaption("2027-10-13", "receiver", fwd).kind == "receiver"
        with pytest.raises(fq.FerrumError):
            fq.Swaption("2027-10-13", "payer", fwd)  # side mismatch
        with pytest.raises(fq.FerrumError):
            fq.Swaption("2027-10-20", "receiver", fwd)  # starts before expiry
        with pytest.raises(fq.FerrumError):
            fq.Swaption.usd_sofr("2027-09-23", "10Y", "straddle", 1e6, 0.04)
        assert "Swaption(" in repr(sw)

    def test_price_and_greeks(self, flat):
        _, idx, c = flat
        sw = fq.Swaption.usd_sofr("2027-09-23", "10Y", "payer", 1e6, 0.04)
        p = sw.price(idx, c, normal_vol=0.008)
        f, a = sw.forward_and_annuity(idx, c)
        assert p.forward_rate == f and p.annuity == a and p.strike == 0.04
        assert p.expiry_time == pytest.approx(c.time("2027-09-23"))
        assert p.pv == pytest.approx(p.model_price * p.annuity, rel=1e-12)
        assert 0 < p.delta < p.annuity and p.gamma > 0 and p.vega > 0
        # Finite-difference vega.
        up = sw.price(idx, c, normal_vol=0.008 + 1e-6).pv
        dn = sw.price(idx, c, normal_vol=0.008 - 1e-6).pv
        assert (up - dn) / 2e-6 == pytest.approx(p.vega, rel=1e-6)
        assert p.to_dict()["vega"] == p.vega

    def test_payer_receiver_parity(self, flat):
        _, idx, c = flat
        payer = fq.Swaption.usd_sofr("2027-09-23", "10Y", "payer", 1e6, 0.04)
        receiver = fq.Swaption.usd_sofr("2027-09-23", "10Y", "receiver", 1e6, 0.04)
        p, r = payer.price(idx, c, normal_vol=0.008), receiver.price(idx, c, normal_vol=0.008)
        assert p.pv - r.pv == pytest.approx(p.annuity * (p.forward_rate - 0.04), rel=1e-10)
        assert p.vega == pytest.approx(r.vega, rel=1e-12)
        assert p.delta - r.delta == pytest.approx(p.annuity, rel=1e-10)

    def test_implied_vols_round_trip(self, flat):
        _, idx, c = flat
        sw = fq.Swaption.usd_sofr("2027-09-23", "10Y", "payer", 1e6, 0.04)
        pv_n = sw.price(idx, c, normal_vol=0.008).pv
        assert sw.implied_normal_vol(idx, c, pv_n) == pytest.approx(0.008, abs=1e-12)
        pv_l = sw.price(idx, c, lognormal_vol=0.25, shift=0.02).pv
        assert sw.implied_lognormal_vol(idx, c, pv_l, shift=0.02) == pytest.approx(0.25, abs=1e-10)
        with pytest.raises(fq.FerrumError):
            sw.price(idx, c)
        with pytest.raises(fq.FerrumError):
            sw.price(idx, c, normal_vol=0.008, lognormal_vol=0.25)
        with pytest.raises(fq.FerrumError):
            sw.implied_normal_vol(idx, c, -1.0)

    def test_expired(self, flat):
        _, idx, c = flat
        sw = fq.Swaption.usd_sofr("2026-09-23", "10Y", "payer", 1e6, 0.04)
        with pytest.raises(fq.FerrumError):
            sw.price(idx, c, normal_vol=0.008)


# ---------------------------------------------------------------------------
# RatesBlotter
# ---------------------------------------------------------------------------


class TestRatesBlotter:
    def test_rows(self, cv):
        book = RatesBlotter(cv, sample_rates_book())
        rows = book.rows()
        assert len(rows) == len(sample_rates_book())
        by = {r["label"]: r for r in rows}
        # Spot swaps struck at the bootstrap quote are par swaps.
        for label in ("2Y pay", "5Y rec", "10Y pay"):
            assert by[label]["npv"] == pytest.approx(0.0, abs=1e-6)
            assert by[label]["par_rate"] == pytest.approx(by[label]["rate"], abs=1e-12)
        assert by["2Y pay"]["dv01"] > 0 and by["5Y rec"]["dv01"] < 0
        assert by["30Y rec"]["npv"] > 0  # receiving above par
        assert by["1Yx10Y payer"]["vega"] > 0 and by["5Yx5Y payer"]["vega"] < 0  # short position
        assert by["5Yx5Y payer"]["npv"] < 0 and by["5Yx5Y payer"]["pv01"] < 0
        assert by["2Yx2Y rec"]["dv01"] < 0  # receiver loses when rates rise
        assert by["2Y pay"]["type"] == "OIS" and by["1Yx10Y payer"]["type"] == "SWPN"
        t = book.totals(rows)
        assert t["npv"] == pytest.approx(sum(r["npv"] for r in rows))
        assert book.npv() == pytest.approx(t["npv"], rel=1e-12)

    def test_ladder_sums_to_parallel_dv01(self, cv):
        book = RatesBlotter(cv, sample_rates_book())
        ladder = book.ladder()
        assert [r["key"] for r in ladder] == cv.result.keys
        zero_total = sum(r["zero_dv01"] for r in ladder)
        assert zero_total == pytest.approx(book.totals()["dv01"], rel=1e-5)
        # A single 5Y par swap: zero risk concentrated at 5Y, par risk ~ its PV01 at 5Y only.
        one = RatesBlotter(cv, [SwapPosition("5Y", "5Y", "pay", 100e6, 0.0378)])
        lad = {r["key"]: r for r in one.ladder()}
        row = one.rows()[0]
        assert lad["OIS 5Y"]["par_dv01"] == pytest.approx(row["pv01"], rel=1e-3)
        for k in ("OIS 7Y", "OIS 10Y", "OIS 20Y", "OIS 30Y"):
            assert abs(lad[k]["par_dv01"]) < 1e-3 * row["pv01"]
        for k in ("DEPO 1D", "SR3 Z26", "OIS 2Y", "OIS 3Y"):
            assert abs(lad[k]["par_dv01"]) < 1e-3 * row["pv01"]

    def test_format(self, cv):
        book = RatesBlotter(cv, sample_rates_book())
        text = book.format()
        assert "Rates blotter" in text and "TOTAL" in text
        assert text.count("\n") == len(sample_rates_book()) + 4

    def test_position_validation(self, cv):
        with pytest.raises(ValueError):
            RatesBlotter(cv, [SwapPosition("bad", None, "pay", 1e6, 0.04)])

    def test_to_dataframe_if_pandas(self, cv):
        pd = pytest.importorskip("pandas")
        assert isinstance(RatesBlotter(cv, sample_rates_book()).to_dataframe(), pd.DataFrame)


# ---------------------------------------------------------------------------
# QuantLib
# ---------------------------------------------------------------------------


def _ql_date(d: fq.Date):
    return ql.Date(d.day, d.month, d.year)


CAL = None if ql is None else ql.UnitedStates(ql.UnitedStates.GovernmentBond)


def _ql_flat(today: fq.Date, rate: float):
    ql.Settings.instance().evaluationDate = _ql_date(today)
    return ql.YieldTermStructureHandle(ql.FlatForward(_ql_date(today), rate, ql.Actual365Fixed(), ql.Continuous))


def _ql_ois(curve, tenor: str, side: str, notional: float, rate: float, effective=None):
    kw = dict(paymentLag=2, settlementDays=2, discountingTermStructure=curve, nominal=notional,
              swapType=ql.Swap.Payer if side == "pay" else ql.Swap.Receiver)
    if effective is not None:
        kw.pop("settlementDays")
        kw["effectiveDate"] = _ql_date(effective)
    return ql.MakeOIS(ql.Period(tenor), ql.Sofr(curve), rate, ql.Period(0, ql.Days), **kw)


def _ql_bootstrap(trade: fq.Date, quotes):
    ql.Settings.instance().evaluationDate = _ql_date(trade)
    helpers = []
    for q in quotes:
        kind = q[0].upper()
        if kind == "DEPO":
            helpers.append(ql.DepositRateHelper(ql.QuoteHandle(ql.SimpleQuote(q[2])), ql.Period(q[1]), 0, CAL, ql.Following, False, ql.Actual360()))
        elif kind == "SR3":
            code, price = q[1], q[2]
            convexity = q[3] if len(q) > 3 else 0.0
            helpers.append(ql.SofrFutureRateHelper(price, {"H": 3, "M": 6, "U": 9, "Z": 12}[code[0]], 2000 + int(code[1:]), ql.Quarterly,
                                                   ql.QuoteHandle(ql.SimpleQuote(convexity))))
        else:
            helpers.append(ql.OISRateHelper(2, ql.Period(q[1]), ql.QuoteHandle(ql.SimpleQuote(q[2])), ql.Sofr(), ql.YieldTermStructureHandle(),
                                            False, 2, ql.ModifiedFollowing, ql.Annual, CAL, ql.Period(0, ql.Days), 0.0, ql.Pillar.LastRelevantDate))
    curve = ql.PiecewiseLogLinearDiscount(_ql_date(trade), helpers, ql.Actual365Fixed())
    curve.enableExtrapolation()
    return ql.YieldTermStructureHandle(curve)


@needs_ql
class TestAgainstQuantLib:
    def test_flat_curve_par_rates_and_legs(self, flat):
        today, idx, c = flat
        qc = _ql_flat(today, 0.04)
        for yrs in (1, 2, 5, 10, 30):
            s = fq.OisSwap.usd_sofr_spot(today, f"{yrs}Y", "pay", 1e6, 0.0)
            assert s.par_rate(idx, c) == pytest.approx(_ql_ois(qc, f"{yrs}Y", "pay", 1e6, 0.0).fairRate(), abs=1e-13)
        s = fq.OisSwap.usd_sofr_spot(today, "5Y", "pay", 1e6, 0.04)
        q = _ql_ois(qc, "5Y", "pay", 1e6, 0.04)
        v = s.value(idx, c)
        assert v.npv == pytest.approx(q.NPV(), abs=1e-6)
        assert v.fixed_leg_pv == pytest.approx(-q.fixedLegNPV(), abs=1e-6)
        assert v.float_leg_pv == pytest.approx(q.overnightLegNPV(), abs=1e-6)
        assert [cf["amount"] for cf in s.fixed_cashflows()] == pytest.approx([cf.amount() for cf in q.fixedLeg()], abs=1e-7)
        assert [cf["amount"] for cf in s.float_cashflows(idx, c)] == pytest.approx([cf.amount() for cf in q.overnightLeg()], abs=1e-7)
        assert [str(cf["payment_date"]) for cf in s.fixed_cashflows()] == [cf.date().ISO() for cf in q.fixedLeg()]

    def test_swaps_on_bootstrapped_curve(self, cv):
        qc = _ql_bootstrap(TRADE, sample_quotes())
        for tenor, side, rate in (("2Y", "pay", 0.0380), ("5Y", "receive", 0.0378), ("10Y", "pay", 0.0395), ("7Y", "pay", 0.0400), ("30Y", "receive", 0.0425)):
            s = fq.OisSwap.usd_sofr_spot(TRADE, tenor, side, 100e6, rate)
            q = _ql_ois(qc, tenor, side, 100e6, rate)
            assert s.npv(cv.index, cv.curve) == pytest.approx(q.NPV(), abs=2e-3), tenor
            assert s.par_rate(cv.index, cv.curve) == pytest.approx(q.fairRate(), abs=1e-11), tenor
        # Forward-starting swap.
        s = fq.OisSwap.usd_sofr("2027-10-15", "2032-10-15", "pay", 50e6, 0.039)
        q = _ql_ois(qc, "5Y", "pay", 50e6, 0.039, effective=fq.Date(2027, 10, 15))
        assert str(s.maturity_date) == q.maturityDate().ISO()
        assert s.npv(cv.index, cv.curve) == pytest.approx(q.NPV(), abs=2e-3)

    def test_swaption_goldens_live(self, flat):
        today, idx, c = flat
        qc = _ql_flat(today, 0.04)
        expiry = fq.Date(2027, 9, 23)
        eff = CAL.advance(_ql_date(expiry), 2, ql.Days)

        def fwd(side):
            return ql.MakeOIS(ql.Period(10, ql.Years), ql.Sofr(qc), 0.04, ql.Period(0, ql.Days), paymentLag=2,
                              discountingTermStructure=qc, nominal=1e6, swapType=side, effectiveDate=eff)

        normal = ql.SwaptionVolatilityStructureHandle(ql.ConstantSwaptionVolatility(_ql_date(today), CAL, ql.ModifiedFollowing, 0.008, ql.Actual365Fixed(), ql.Normal))
        shifted = ql.SwaptionVolatilityStructureHandle(ql.ConstantSwaptionVolatility(_ql_date(today), CAL, ql.ModifiedFollowing, 0.25, ql.Actual365Fixed(), ql.ShiftedLognormal, 0.02))
        qp = ql.Swaption(fwd(ql.Swap.Payer), ql.EuropeanExercise(_ql_date(expiry)))
        qr = ql.Swaption(fwd(ql.Swap.Receiver), ql.EuropeanExercise(_ql_date(expiry)))
        qp.setPricingEngine(ql.BachelierSwaptionEngine(qc, normal))
        qr.setPricingEngine(ql.BachelierSwaptionEngine(qc, normal))

        payer = fq.Swaption.usd_sofr(expiry, "10Y", "payer", 1e6, 0.04)
        receiver = fq.Swaption.usd_sofr(expiry, "10Y", "receiver", 1e6, 0.04)
        p = payer.price(idx, c, normal_vol=0.008)
        assert p.pv == pytest.approx(qp.NPV(), abs=1e-6)
        assert p.annuity == pytest.approx(qp.annuity(), abs=1e-5)
        assert p.vega == pytest.approx(qp.vega(), abs=1e-5)
        assert p.delta == pytest.approx(qp.delta(), abs=1e-5)
        assert receiver.price(idx, c, normal_vol=0.008).pv == pytest.approx(qr.NPV(), abs=1e-6)
        qp.setPricingEngine(ql.BlackSwaptionEngine(qc, shifted))
        assert payer.price(idx, c, lognormal_vol=0.25, shift=0.02).pv == pytest.approx(qp.NPV(), abs=1e-6)
