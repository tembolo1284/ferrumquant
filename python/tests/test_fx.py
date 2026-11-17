"""FX tests for the ferrumquant Python module and the fqview FX view.

Two layers:
  * self-consistency and API tests that need only ``ferrumquant``: date
    conventions, covered interest parity, forward/NDF valuation, put-call
    parity, Greeks against finite differences, strike/delta round trips,
    surface pillars reproducing their quotes, errors as ``FerrumError``;
  * cross-checks against QuantLib-Python (skipped if not installed):
    Garman-Kohlhagen prices and deltas from ``AnalyticEuropeanEngine`` on a
    ``GarmanKohlagenProcess``, and strike-from-delta / ATM strikes from
    ``BlackDeltaCalculator`` in all four delta conventions.

Run from the repo root with ``pytest python/tests`` after
``maturin develop`` in crates/fq-py.
"""

from __future__ import annotations

import math

import pytest

import ferrumquant as fq
from fqview import FxOptionPosition, FxView, sample_fx_book, sample_fx_market, sample_fx_quotes

try:
    import QuantLib as ql  # type: ignore  # noqa: E402
except ImportError:  # pragma: no cover
    ql = None

needs_ql = pytest.mark.skipif(ql is None, reason="QuantLib-Python not installed")

TRADE = "2026-10-13"
EUR_RATE, USD_RATE, JPY_RATE = 0.02, 0.04, 0.005


def flat(rate: float) -> fq.DiscountCurve:
    return fq.DiscountCurve.flat(TRADE, rate, "2040-01-01")


@pytest.fixture(scope="module")
def eurusd() -> fq.FxMarket:
    return fq.FxMarket("EURUSD", TRADE, 1.1750, flat(EUR_RATE), flat(USD_RATE))


@pytest.fixture(scope="module")
def usdjpy() -> fq.FxMarket:
    return fq.FxMarket("USDJPY", TRADE, 152.30, flat(USD_RATE), flat(JPY_RATE))


@pytest.fixture(scope="module")
def surface(eurusd) -> fq.FxVolSurface:
    return fq.FxVolSurface(eurusd, sample_fx_quotes("EURUSD"))


# ---------------------------------------------------------------------------
# Pairs and dates
# ---------------------------------------------------------------------------


class TestCurrencyPair:
    def test_market_order_and_parsing(self):
        assert str(fq.CurrencyPair("USD", "EUR")) == "EURUSD"
        assert str(fq.CurrencyPair("JPY", "USD")) == "USDJPY"
        assert str(fq.CurrencyPair("eur/usd")) == "EURUSD"
        inv = fq.CurrencyPair("USDEUR")
        assert not inv.is_market_order and str(inv.inverse()) == "EURUSD"
        with pytest.raises(fq.FerrumError):
            fq.CurrencyPair("EURXXX")
        with pytest.raises(fq.FerrumError):
            fq.CurrencyPair("EUREUR")

    def test_conventions(self):
        p = fq.CurrencyPair("EURUSD")
        assert (p.base, p.quote, p.spot_lag, p.pip) == ("EUR", "USD", 2, 0.0001)
        assert p.calendar == "TARGET+USNY" and p.settlement_calendar == "TARGET+USNY"
        assert p.premium_currency == "quote"
        assert (p.delta_convention("6M"), p.delta_convention("1Y"), p.delta_convention("2Y")) == ("spot", "spot", "forward")
        j = fq.CurrencyPair("USDJPY")
        assert (j.pip, j.premium_currency, j.delta_convention("3M"), j.delta_convention("18M")) == (0.01, "base", "spot_pa", "forward_pa")
        assert fq.CurrencyPair("USDCAD").spot_lag == 1
        assert fq.CurrencyPair("EURGBP").settlement_calendar == "TARGET+UKLN+USNY"

    def test_spot_and_forward_dates(self):
        p = fq.CurrencyPair("EURUSD")
        assert str(p.spot_date(TRADE)) == "2026-10-15"
        assert str(p.spot_date("2026-10-16")) == "2026-10-20"           # Friday trade
        assert str(p.spot_date("2026-10-09")) == "2026-10-13"           # Columbus Day on T+1 does not delay
        assert str(p.spot_date("2026-04-02")) == "2026-04-08"           # Good Friday + Easter Monday
        assert str(p.forward_date(TRADE, "1M")) == "2026-11-16"         # 15 Nov is a Sunday
        assert str(p.forward_date(TRADE, "1Y")) == "2027-10-15"
        assert str(p.forward_date("2026-04-28", "1M")) == "2026-05-29"   # end-of-month rule
        assert str(p.forward_date("2026-04-28", "2M")) == "2026-06-30"
        e, d = p.expiry_and_delivery(TRADE, "6M")
        assert (str(e), str(d)) == ("2027-04-13", "2027-04-15")
        assert p.delivery_for_expiry(e) == d
        assert str(fq.CurrencyPair("USDCAD").spot_date("2026-07-02")) == "2026-07-06"  # 3 Jul US holiday


# ---------------------------------------------------------------------------
# Market, forwards, NDFs
# ---------------------------------------------------------------------------


class TestForwards:
    def test_covered_interest_parity(self, eurusd):
        assert str(eurusd.spot_date) == "2026-10-15"
        assert eurusd.forward(eurusd.spot_date) == pytest.approx(1.1750, abs=1e-15)
        vd, f = eurusd.forward_for_tenor("1Y")
        t = (vd - eurusd.spot_date) / 365.0
        assert f == pytest.approx(1.1750 * math.exp((USD_RATE - EUR_RATE) * t), abs=1e-12)
        assert eurusd.forward_points(vd) == pytest.approx((f - 1.1750) / 1e-4, abs=1e-9)
        assert 0.019 < eurusd.implied_rate_differential(vd) < 0.021
        assert eurusd.discount("USD", vd) == pytest.approx(math.exp(-USD_RATE * (vd - eurusd.trade_date) / 365.0), abs=1e-14)
        with pytest.raises(fq.FerrumError, match="before spot"):
            eurusd.forward("2026-10-14")
        with pytest.raises(fq.FerrumError, match="not part of"):
            eurusd.discount("JPY", vd)

    def test_construction_errors(self):
        with pytest.raises(fq.FerrumError, match="referenced at"):
            fq.FxMarket("EURUSD", TRADE, 1.175, flat(0.02), fq.DiscountCurve.flat("2026-10-14", 0.04, "2040-01-01"))
        with pytest.raises(fq.FerrumError, match="positive"):
            fq.FxMarket("EURUSD", TRADE, -1.0, flat(0.02), flat(0.04))

    def test_forward_valuation_and_parity_with_market_bumps(self, eurusd):
        vd, f = eurusd.forward_for_tenor("6M")
        at = fq.FxForward("EURUSD", "buy", 10e6, f, vd)
        assert at.npv_quote(eurusd) == pytest.approx(0.0, abs=1e-6)
        long = fq.FxForward.from_tenor("EURUSD", TRADE, "6M", "buy", 10e6, f - 0.01)
        short = fq.FxForward.from_tenor("EURUSD", TRADE, "6M", "sell", 10e6, f - 0.01)
        assert long.value_date == vd and long.notional_quote == pytest.approx(10e6 * (f - 0.01))
        v = long.value(eurusd)
        assert set(v) == {"forward", "forward_points", "npv_quote", "npv_base", "spot_delta", "forward_delta", "discount_quote"}
        assert v["npv_quote"] == pytest.approx(10e6 * 0.01 * eurusd.discount("USD", vd), abs=1e-6)
        assert v["npv_base"] == pytest.approx(v["npv_quote"] / 1.175, abs=1e-9)
        assert short.value(eurusd)["npv_quote"] == pytest.approx(-v["npv_quote"], abs=1e-9)
        h = 1e-4
        fd = (long.npv_quote(eurusd.with_spot(1.175 + h)) - long.npv_quote(eurusd.with_spot(1.175 - h))) / (2 * h)
        assert fd == pytest.approx(v["spot_delta"], abs=1e-3)
        with pytest.raises(fq.FerrumError):
            fq.FxForward("EURUSD", "sideways", 1e6, 1.2, vd)

    def test_implied_quote_curve_reprices_points(self):
        points = [("1W", 4.2), ("1M", 18.5), ("3M", 56.0), ("6M", 112.0), ("1Y", 225.0)]
        usd = fq.FxMarket.implied_quote_curve("EURUSD", TRADE, 1.175, flat(EUR_RATE), points)
        m = fq.FxMarket("EURUSD", TRADE, 1.175, flat(EUR_RATE), usd)
        for tenor, pts in points:
            vd, _ = m.forward_for_tenor(tenor)
            assert m.forward_points(vd) == pytest.approx(pts, abs=1e-9)

    def test_ndf(self):
        usd, brl = flat(0.04), flat(0.11)
        m = fq.FxMarket("USDBRL", TRADE, 5.40, usd, brl)
        ndf = fq.FxNdf.standard("USDBRL", TRADE, "3M", "buy", 10e6, 5.45)
        assert ndf.settlement_currency == "USD"
        assert ndf.settlement_date == fq.CurrencyPair("USDBRL").forward_date(TRADE, "3M")
        assert ndf.fixing_date < ndf.settlement_date
        f = m.forward(ndf.fixing_date)
        assert f > 5.40
        assert ndf.settlement_amount(f) == pytest.approx(10e6 * (f - 5.45) / f)
        v = ndf.value(m)
        assert v["npv_base"] == pytest.approx(10e6 * (f - 5.45) / f * m.discount("USD", ndf.settlement_date), abs=1e-6)
        assert v["npv_quote"] == pytest.approx(v["npv_base"] * 5.40, abs=1e-6)
        brl_settled = fq.FxNdf("USDBRL", "buy", 10e6, 5.45, ndf.fixing_date, ndf.settlement_date, "BRL")
        assert brl_settled.value(m)["npv_quote"] == pytest.approx(10e6 * (f - 5.45) * m.discount("BRL", ndf.settlement_date), abs=1e-6)
        with pytest.raises(fq.FerrumError, match="before fixing"):
            fq.FxNdf("USDBRL", "buy", 1e6, 5.4, TRADE, "2026-10-12", "USD")
        with pytest.raises(fq.FerrumError, match="not part of"):
            fq.FxNdf("USDBRL", "buy", 1e6, 5.4, TRADE, TRADE, "EUR")


# ---------------------------------------------------------------------------
# Vanillas
# ---------------------------------------------------------------------------

VALUE_KEYS = {
    "forward", "discount_quote", "time_to_expiry", "vol", "pv_quote", "pv_base", "pct_base", "quote_pips",
    "spot_delta", "forward_delta", "spot_delta_pa", "forward_delta_pa", "gamma", "vega", "vanna", "volga",
}


class TestVanilla:
    def test_construction_and_premium_default(self, eurusd, usdjpy):
        o = fq.FxVanilla.from_tenor("EURUSD", TRADE, "6M", "call", 1.19, 10e6)
        assert (str(o.expiry), str(o.delivery), o.kind, o.premium_currency) == ("2027-04-13", "2027-04-15", "call", "quote")
        j = fq.FxVanilla.from_tenor("USDJPY", TRADE, "6M", "put", 150.0, 10e6)
        assert j.premium_currency == "base"
        assert fq.FxVanilla.from_tenor("USDJPY", TRADE, "6M", "put", 150.0, 10e6, premium="quote").premium_currency == "quote"
        with pytest.raises(fq.FerrumError):
            fq.FxVanilla("EURUSD", "call", 1.2, "2027-04-15", "2027-04-13", 1e6)   # delivery before expiry
        with pytest.raises(fq.FerrumError, match="not after the trade date"):
            fq.FxVanilla("EURUSD", "call", 1.2, TRADE, "2026-10-15", 1e6).value(eurusd, 0.1)
        with pytest.raises(fq.FerrumError):
            fq.FxVanilla("EURUSD", "straddle", 1.2, "2027-04-13", "2027-04-15", 1e6)

    def test_put_call_parity_against_the_forward(self, eurusd):
        c = fq.FxVanilla.from_tenor("EURUSD", TRADE, "6M", "call", 1.19, 10e6)
        p = fq.FxVanilla("EURUSD", "put", 1.19, c.expiry, c.delivery, 10e6)
        fwd = fq.FxForward("EURUSD", "buy", 10e6, 1.19, c.delivery)
        vc, vp = c.value(eurusd, 0.09), p.value(eurusd, 0.09)
        assert set(vc) == VALUE_KEYS
        assert vc["pv_quote"] - vp["pv_quote"] == pytest.approx(fwd.npv_quote(eurusd), abs=1e-6)
        assert vc["forward_delta"] - vp["forward_delta"] == pytest.approx(1.0, abs=1e-14)
        assert vc["vega"] == pytest.approx(vp["vega"], abs=1e-6)
        assert vc["gamma"] == pytest.approx(vp["gamma"], abs=1e-6)

    def test_greeks_match_finite_differences(self, eurusd):
        o = fq.FxVanilla.from_tenor("EURUSD", TRADE, "1Y", "call", 1.20, 1e6)
        vol = 0.085
        v = o.value(eurusd, vol)
        h = 1e-4
        up, dn = o.value(eurusd.with_spot(1.175 + h), vol), o.value(eurusd.with_spot(1.175 - h), vol)
        assert (up["pv_quote"] - dn["pv_quote"]) / (2 * h) == pytest.approx(v["spot_delta"] * 1e6, rel=1e-7)
        assert (up["pv_quote"] + dn["pv_quote"] - 2 * v["pv_quote"]) / h**2 == pytest.approx(v["gamma"], rel=1e-6)
        hv = 1e-5
        vu, vd = o.value(eurusd, vol + hv), o.value(eurusd, vol - hv)
        assert (vu["pv_quote"] - vd["pv_quote"]) / (2 * hv) == pytest.approx(v["vega"], rel=1e-7)
        assert (vu["pv_quote"] + vd["pv_quote"] - 2 * v["pv_quote"]) / hv**2 == pytest.approx(v["volga"], abs=5.0)
        assert (up["vega"] - dn["vega"]) / (2 * h) == pytest.approx(v["vanna"], rel=1e-5)
        s = o.slice(eurusd)
        assert v["spot_delta"] == pytest.approx(s.spot_factor() * v["forward_delta"], abs=1e-15)
        assert v["spot_delta_pa"] == pytest.approx(v["spot_delta"] - v["pct_base"], abs=1e-15)
        assert v["quote_pips"] == pytest.approx(v["pv_quote"] / 1e6 / 1e-4)
        for conv, key in (("spot", "spot_delta"), ("forward", "forward_delta"), ("spot_pa", "spot_delta_pa"), ("forward_pa", "forward_delta_pa")):
            assert o.delta(eurusd, vol, conv) == pytest.approx(v[key], abs=1e-15)

    def test_implied_vol_round_trip(self, eurusd):
        o = fq.FxVanilla.from_tenor("EURUSD", TRADE, "3M", "put", 1.15, 5e6)
        pv = o.value(eurusd, 0.0725)["pv_quote"]
        assert o.implied_vol(eurusd, pv) == pytest.approx(0.0725, abs=1e-10)

    def test_slice_delta_strike_round_trips(self, eurusd):
        s = fq.FxVanilla.from_tenor("EURUSD", TRADE, "1Y", "call", 1.20, 1.0).slice(eurusd)
        vol = 0.09
        for conv in ("spot", "forward", "spot_pa", "forward_pa"):
            for kind, delta in (("call", 0.25), ("call", 0.10), ("put", -0.25), ("put", -0.10)):
                k = s.strike_from_delta(kind, delta, vol, conv)
                assert s.delta(kind, k, vol, conv) == pytest.approx(delta, abs=1e-10)
        plain, pa = s.strike_from_delta("call", 0.25, vol, "forward"), s.strike_from_delta("call", 0.25, vol, "forward_pa")
        assert pa < plain
        k = s.atm_strike("dns", vol)
        assert s.delta("call", k, vol, "forward") + s.delta("put", k, vol, "forward") == pytest.approx(0.0, abs=1e-14)
        kpa = s.atm_strike("dns_pa", vol)
        assert s.delta("call", kpa, vol, "forward_pa") + s.delta("put", kpa, vol, "forward_pa") == pytest.approx(0.0, abs=1e-14)
        assert s.atm_strike("spot", vol) == 1.175 and s.atm_strike("forward", vol) == s.forward
        with pytest.raises(fq.FerrumError, match="not attainable"):
            s.strike_from_delta("call", 0.9, vol, "forward_pa")
        with pytest.raises(fq.FerrumError):
            s.strike_from_delta("call", 0.25, vol, "sideways")
        rebuilt = fq.FxSlice(s.spot, s.forward, s.discount_quote, s.time_to_expiry)
        assert rebuilt.strike_from_delta("put", -0.25, vol, "spot") == s.strike_from_delta("put", -0.25, vol, "spot")


# ---------------------------------------------------------------------------
# Vol surface
# ---------------------------------------------------------------------------


class TestVolSurface:
    def test_pillars_reprice_their_quotes(self, surface):
        smiles = surface.smiles()
        assert [str(t) for t in surface.tenors] == ["1W", "1M", "3M", "6M", "1Y", "2Y"] and len(surface) == 6
        assert surface.config() == {"premium": "quote", "atm": "dns", "delta": None, "beta": 1.0}
        quotes = {q[0]: q for q in sample_fx_quotes("EURUSD")}
        for s in smiles:
            q = quotes[str(s["tenor"])]
            assert s["rmse"] < 5e-4
            pts = {round(d, 4): (k, v) for d, k, v in s["points"]}
            assert pts[0.0] == (s["atm_strike"], s["atm_vol"]) and s["atm_vol"] == q[1]
            assert pts[0.25][1] - pts[-0.25][1] == pytest.approx(q[2])
            assert 0.5 * (pts[0.25][1] + pts[-0.25][1]) - q[1] == pytest.approx(q[3])
            assert (0.10 in pts) == (len(q) == 6)
            sl = fq.FxSlice(surface.market.spot, s["forward"], s["discount_quote"], s["time_to_expiry"])
            for d, k, v in s["points"]:
                tol = 1e-7 if len(s["points"]) == 3 else 1e-3
                assert surface.vol(s["expiry"], k) == pytest.approx(v, abs=tol)
                if d:
                    assert sl.delta("call" if d > 0 else "put", k, v, s["delta_convention"]) == pytest.approx(d, abs=1e-10)
        assert smiles[-1]["delta_convention"] == "forward" and smiles[-2]["delta_convention"] == "spot"
        assert surface.smile("1Y")["atm_strike"] > surface.smile("1Y")["forward"]
        with pytest.raises(fq.FerrumError, match="no smile pillar"):
            surface.smile("5Y")

    def test_interpolation_and_delta_queries(self, surface, eurusd):
        three_m, one_y = surface.smile("3M"), surface.smile("1Y")
        e6, _ = eurusd.pair.expiry_and_delivery(TRADE, "6M")
        # Total variance between the neighbouring pillars; flat outside.
        v6 = surface.vol(e6, 1.18)
        va, vb = surface.vol(three_m["expiry"], 1.18), surface.vol(one_y["expiry"], 1.18)
        assert min(va, vb) < v6 < max(va, vb)
        assert surface.vol("2031-10-15", 1.25) == surface.vol(surface.smile("2Y")["expiry"], 1.25)
        with pytest.raises(fq.FerrumError, match="not after"):
            surface.vol(TRADE, 1.17)
        k, v = surface.vol_for_delta(one_y["expiry"], "put", -0.25)
        sl = surface.slice_at(one_y["expiry"])
        assert sl.delta("put", k, v, "spot") == pytest.approx(-0.25, abs=1e-10)
        quoted = next(kk for d, kk, _ in one_y["points"] if abs(d + 0.25) < 1e-12)
        assert k == pytest.approx(quoted, abs=2e-3)
        # Pricing off the surface is pricing with the surface vol.
        o = fq.FxVanilla("EURUSD", "put", k, one_y["expiry"], one_y["delivery"], 1e6)
        assert surface.value(o)["pv_quote"] == pytest.approx(o.value(eurusd, surface.vol(one_y["expiry"], k))["pv_quote"], abs=1e-9)
        bumped = surface.with_parallel_bump(0.01)
        for a, b in zip(surface.smiles(), bumped.smiles()):
            assert b["atm_vol"] - a["atm_vol"] == pytest.approx(0.01, abs=1e-15)

    def test_quote_shapes_and_conventions(self, eurusd, usdjpy):
        as_dicts = [{"tenor": "1M", "atm": 0.072, "rr25": -0.004, "bf25": 0.0015}, {"tenor": "1Y", "atm": 0.08, "rr25": -0.009, "bf25": 0.0025, "rr10": -0.0165, "bf10": 0.008}]
        as_tuples = [("1M", 0.072, -0.004, 0.0015), ("1Y", 0.08, -0.009, 0.0025, -0.0165, 0.008)]
        a, b = fq.FxVolSurface(eurusd, as_dicts), fq.FxVolSurface(eurusd, as_tuples)
        assert a.smile("1Y")["points"] == b.smile("1Y")["points"]
        with pytest.raises(fq.FerrumError, match="missing 'atm'"):
            fq.FxVolSurface(eurusd, [{"tenor": "1M", "rr25": 0.0, "bf25": 0.0}])
        with pytest.raises(fq.FerrumError, match="at least one expiry"):
            fq.FxVolSurface(eurusd, [])
        with pytest.raises(fq.FerrumError, match="butterfly"):
            fq.FxVolSurface(eurusd, [{"tenor": "1M", "atm": 0.072, "rr25": -0.004, "bf25": 0.0015, "rr10": -0.01}])
        # USDJPY: premium-adjusted conventions by default, overridable.
        j = fq.FxVolSurface(usdjpy, sample_fx_quotes("USDJPY"))
        assert j.config() == {"premium": "base", "atm": "dns_pa", "delta": None, "beta": 1.0}
        assert j.smile("1Y")["delta_convention"] == "spot_pa" and j.smile("1Y")["atm_strike"] < j.smile("1Y")["forward"]
        q = fq.FxVolSurface(usdjpy, sample_fx_quotes("USDJPY"), premium="quote", delta="forward", beta=0.9)
        assert q.config() == {"premium": "quote", "atm": "dns", "delta": "forward", "beta": 0.9}
        assert all(s["delta_convention"] == "forward" for s in q.smiles())


# ---------------------------------------------------------------------------
# fqview
# ---------------------------------------------------------------------------


class TestFxView:
    def test_views_render_and_book_adds_up(self):
        for pair in ("EURUSD", "USDJPY"):
            view = FxView(sample_fx_market(pair, TRADE), sample_fx_quotes(pair))
            fwd = view.format_forwards()
            assert pair in fwd and "1Y" in fwd
            smiles = view.format_smiles()
            assert "25dC" in smiles
            rows = view.book_rows(sample_fx_book(pair))
            assert len(rows) == len(sample_fx_book(pair))
            total = view.format_book(sample_fx_book(pair)).splitlines()[-1]
            assert total.startswith("TOTAL")
        # A delta-struck option really has that delta on the pillar.
        m = sample_fx_market("EURUSD", TRADE)
        view = FxView(m, sample_fx_quotes("EURUSD"))
        o = view.build(FxOptionPosition("x", "1Y", "call", "25dC", 1e6))
        assert o.delta(m, view.surface.vol(o.expiry, o.strike), "spot") == pytest.approx(0.25, abs=1e-10)
        # ATM is the delta-neutral strike at the smile's own vol there (the 6M pillar is a
        # 5-point least-squares fit, so it sits within a few vol bp of the quoted ATM strike).
        atm = view.build(FxOptionPosition("y", "6M", "put", "ATM", 1e6))
        sl = view.surface.slice_at(atm.expiry)
        assert atm.strike == pytest.approx(sl.atm_strike("dns", view.surface.vol(atm.expiry, atm.strike)), rel=1e-12)
        assert atm.strike == pytest.approx(view.surface.smile("6M")["atm_strike"], rel=1e-4)
        # Short and long of the same option cancel.
        a = view.book_rows([FxOptionPosition("l", "3M", "put", 1.15, 5e6), FxOptionPosition("s", "3M", "put", 1.15, -5e6)])
        assert a[0]["pv_quote"] + a[1]["pv_quote"] == pytest.approx(0.0, abs=1e-9)
        assert a[0]["spot_delta"] + a[1]["spot_delta"] == pytest.approx(0.0, abs=1e-9)


# ---------------------------------------------------------------------------
# QuantLib cross-checks
# ---------------------------------------------------------------------------


def _ql_date(d) -> "ql.Date":
    return ql.Date(d.day, d.month, d.year)


@pytest.fixture(scope="module")
def ql_setup(eurusd):
    """QuantLib's forward runs from its evaluation date while ours runs from
    the spot date; with flat curves the two agree once QuantLib's spot is set
    to ``S * exp(-(r_q - r_b) * t_spot)``."""
    today = _ql_date(fq.Date.from_any(TRADE))
    ql.Settings.instance().evaluationDate = today
    dc = ql.Actual365Fixed()
    t_spot = (eurusd.spot_date - eurusd.trade_date) / 365.0
    spot = ql.QuoteHandle(ql.SimpleQuote(eurusd.spot * math.exp(-(USD_RATE - EUR_RATE) * t_spot)))
    dom = ql.YieldTermStructureHandle(ql.FlatForward(today, USD_RATE, dc))
    fgn = ql.YieldTermStructureHandle(ql.FlatForward(today, EUR_RATE, dc))
    return today, spot, dom, fgn, dc


@needs_ql
class TestAgainstQuantLib:
    """Expiry = delivery in the price tests so both sides discount the payoff
    to the same date; the delta tests feed ``BlackDeltaCalculator`` our own
    forward and spot factor."""

    @pytest.mark.parametrize("days", [30, 91, 182, 365, 730])
    @pytest.mark.parametrize("kind,strike,vol", [("call", 1.1750, 0.08), ("put", 1.1750, 0.08), ("call", 1.2500, 0.10), ("put", 1.1000, 0.12), ("call", 1.0500, 0.15)])
    def test_garman_kohlhagen_price_and_delta(self, eurusd, ql_setup, days, kind, strike, vol):
        today, spot, dom, fgn, dc = ql_setup
        expiry = eurusd.trade_date + days
        volh = ql.BlackVolTermStructureHandle(ql.BlackConstantVol(today, ql.NullCalendar(), vol, dc))
        process = ql.GarmanKohlagenProcess(spot, fgn, dom, volh)
        opt = ql.VanillaOption(ql.PlainVanillaPayoff(ql.Option.Call if kind == "call" else ql.Option.Put, strike), ql.EuropeanExercise(_ql_date(expiry)))
        opt.setPricingEngine(ql.AnalyticEuropeanEngine(process))
        ours = fq.FxVanilla("EURUSD", kind, strike, expiry, expiry, 1.0).value(eurusd, vol)
        assert ours["pv_quote"] == pytest.approx(opt.NPV(), abs=2e-10)
        assert ours["vega"] == pytest.approx(opt.vega(), abs=1e-9)
        # dPV/dS = dPV/dS_ql * dS_ql/dS with S_ql = S * exp(-(r_q - r_b) t_spot).
        t_spot = (eurusd.spot_date - eurusd.trade_date) / 365.0
        assert ours["spot_delta"] == pytest.approx(opt.delta() * math.exp(-(USD_RATE - EUR_RATE) * t_spot), abs=1e-10)
        assert ours["gamma"] == pytest.approx(opt.gamma() * math.exp(-2 * (USD_RATE - EUR_RATE) * t_spot), rel=1e-9)

    @pytest.mark.parametrize("tenor", ["1M", "6M", "1Y", "2Y"])
    @pytest.mark.parametrize("conv,ql_conv", [("spot", "Spot"), ("forward", "Fwd"), ("spot_pa", "PaSpot"), ("forward_pa", "PaFwd")])
    @pytest.mark.parametrize("kind,delta", [("call", 0.25), ("call", 0.10), ("put", -0.25), ("put", -0.10)])
    def test_strike_from_delta_matches_black_delta_calculator(self, eurusd, tenor, conv, ql_conv, kind, delta):
        vol = 0.09
        s = fq.FxVanilla.from_tenor("EURUSD", TRADE, tenor, "call", 1.2, 1.0).slice(eurusd)
        # QuantLib's forward is spot * fDiscount / dDiscount; feeding our spot factor as
        # the foreign discount reproduces our forward and our spot-delta scaling exactly.
        calc = ql.BlackDeltaCalculator(
            ql.Option.Call if kind == "call" else ql.Option.Put,
            getattr(ql.DeltaVolQuote, ql_conv),
            s.spot,
            s.discount_quote,
            s.spot_factor(),
            vol * math.sqrt(s.time_to_expiry),
        )
        k = s.strike_from_delta(kind, delta, vol, conv)
        assert k == pytest.approx(calc.strikeFromDelta(delta), rel=1e-9)
        assert s.delta(kind, k, vol, conv) == pytest.approx(calc.deltaFromStrike(k), abs=1e-12)

    @pytest.mark.parametrize("tenor", ["1M", "1Y"])
    def test_atm_strikes_match_black_delta_calculator(self, eurusd, tenor):
        vol = 0.085
        s = fq.FxVanilla.from_tenor("EURUSD", TRADE, tenor, "call", 1.2, 1.0).slice(eurusd)
        sd = vol * math.sqrt(s.time_to_expiry)
        plain = ql.BlackDeltaCalculator(ql.Option.Call, ql.DeltaVolQuote.Fwd, s.spot, s.discount_quote, s.spot_factor(), sd)
        assert s.atm_strike("spot", vol) == pytest.approx(plain.atmStrike(ql.DeltaVolQuote.AtmSpot), rel=1e-12)
        assert s.atm_strike("forward", vol) == pytest.approx(plain.atmStrike(ql.DeltaVolQuote.AtmFwd), rel=1e-12)
        assert s.atm_strike("dns", vol) == pytest.approx(plain.atmStrike(ql.DeltaVolQuote.AtmDeltaNeutral), rel=1e-12)
        pa = ql.BlackDeltaCalculator(ql.Option.Call, ql.DeltaVolQuote.PaFwd, s.spot, s.discount_quote, s.spot_factor(), sd)
        assert s.atm_strike("dns_pa", vol) == pytest.approx(pa.atmStrike(ql.DeltaVolQuote.AtmDeltaNeutral), rel=1e-12)
