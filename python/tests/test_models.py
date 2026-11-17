"""Hull-White tests: HullWhite, BermudanSwaption, ShortRatePaths, calibrate_hull_white, ModelView.

Self-consistency (parity, limits, tree vs closed form, MC vs curve) plus
QuantLib cross-checks: ZCB options against ``HullWhite.discountBondOption``
(exact), European swaptions against ``JamshidianSwaptionEngine`` on a
lag-free vanilla swap with the same dates (exact) and against
``FdHullWhiteSwaptionEngine`` on the lagged OIS (the two-day payment lag is
deterministic here and stochastic there, so about 0.1bp of notional),
Bermudans against the same FD engine.
"""

from __future__ import annotations

import math

import pytest

import ferrumquant as fq
from fqview import BermudanPosition, CurveView, ModelView, VolView, sample_bermudans, sample_cube_quotes, sample_quotes

try:
    import QuantLib as ql  # type: ignore
except ImportError:  # pragma: no cover
    ql = None

needs_ql = pytest.mark.skipif(ql is None, reason="QuantLib-Python not installed")

BP = 1e-4
TODAY = fq.Date(2026, 10, 13)
NODES = [("2027-10-13", 0.035), ("2029-10-13", 0.038), ("2031-10-13", 0.040), ("2036-10-13", 0.042), ("2056-10-13", 0.043)]


@pytest.fixture(scope="module")
def curve():
    return fq.DiscountCurve.from_zero_rates(TODAY, NODES)


@pytest.fixture(scope="module")
def model(curve):
    return fq.HullWhite(curve, 0.05, 0.01)


@pytest.fixture(scope="module")
def cv():
    return CurveView(TODAY, sample_quotes())


def swaption(kind="payer", strike=0.04, expiry="2027-10-13", tenor="10Y"):
    return fq.Swaption.usd_sofr(expiry, tenor, kind, 1e6, strike)


# ---------------------------------------------------------------------------
# HullWhite: bonds, ZCB options, caplets
# ---------------------------------------------------------------------------


class TestHullWhite:
    def test_construction(self, curve, model):
        assert model.mean_reversion == 0.05 and model.sigma == 0.01
        assert model.curve.reference_date == TODAY
        assert "HullWhite(" in repr(model)
        m2 = model.with_params(0.1, 0.02)
        assert m2.mean_reversion == 0.1 and m2.sigma == 0.02
        for a, s in ((0.0, 0.01), (0.05, -0.01), (float("nan"), 0.01)):
            with pytest.raises(fq.FerrumError):
                fq.HullWhite(curve, a, s)

    def test_bond_identities(self, curve, model):
        for t in (0.5, 3.0, 20.0):
            assert model.bond(0.0, t) == pytest.approx(curve.discount_t(t), rel=1e-15)
        assert model.bond(2.0, 2.0, 0.3) == 1.0
        # Higher factor, lower bond price.
        assert model.bond(2.0, 7.0, 0.01) < model.bond(2.0, 7.0, 0.0) < model.bond(2.0, 7.0, -0.01)
        assert model.b(1.0, 6.0) == pytest.approx((1 - math.exp(-0.05 * 5)) / 0.05)
        assert model.variance(2.0) == pytest.approx(0.01**2 * (1 - math.exp(-0.2)) / 0.1)
        assert model.time("2027-10-13") == pytest.approx(365 / 365.0)
        assert model.short_rate_mean(0.0) == pytest.approx(model.instantaneous_forward(0.0))
        assert model.short_rate_mean(5.0) > model.instantaneous_forward(5.0)
        with pytest.raises(fq.FerrumError):
            model.bond(3.0, 2.0)

    def test_zcb_option_parity(self, curve, model):
        t, s, k = 2.0, 7.0, 0.8
        call, put = model.zcb_option("call", t, s, k), model.zcb_option("put", t, s, k)
        assert call - put == pytest.approx(curve.discount_t(s) - k * curve.discount_t(t), abs=1e-15)
        assert call > 0 and put > 0
        assert model.with_params(0.05, 0.02).zcb_option("put", t, s, k) > put
        with pytest.raises(fq.FerrumError):
            model.zcb_option("straddle", t, s, k)
        with pytest.raises(fq.FerrumError):
            model.zcb_option("call", t, s, 0.0)

    def test_caplets(self, curve, model):
        s, e, p, k, tau = 1.0, 1.25, 1.25 + 2 / 365, 0.04, 91 / 360
        cap = model.compounded_caplet("cap", s, e, k, tau, pay=p)
        floor = model.compounded_caplet("floor", s, e, k, tau, pay=p)
        lag = curve.discount_t(p) / curve.discount_t(e)
        assert cap - floor == pytest.approx(lag * (curve.discount_t(s) - (1 + k * tau) * curve.discount_t(e)), abs=1e-15)
        term = model.term_caplet("cap", s, e, k, tau, pay=p)
        assert cap > term > 0
        assert term == pytest.approx(lag * (1 + k * tau) * model.zcb_option("put", s, e, 1 / (1 + k * tau)), abs=1e-15)
        # Default payment at the accrual end; dated wrapper agrees with times.
        assert model.compounded_caplet("cap", s, e, k, tau) == pytest.approx(cap / lag, rel=1e-12)
        ds, de, dp = fq.Date(2027, 10, 13), fq.Date(2028, 1, 12), fq.Date(2028, 1, 14)
        dated = model.compounded_caplet_on("cap", ds, de, k, tau, pay=dp)
        assert dated == pytest.approx(model.compounded_caplet("cap", model.time(ds), model.time(de), k, tau, pay=model.time(dp)), rel=1e-15)
        with pytest.raises(fq.FerrumError):
            model.compounded_caplet_on("cap", de, ds, k, tau)
        with pytest.raises(fq.FerrumError):
            model.compounded_caplet("collar", s, e, k, tau)


# ---------------------------------------------------------------------------
# European swaptions
# ---------------------------------------------------------------------------


class TestEuropeanSwaption:
    def test_parity_and_implied_vol(self, curve, model):
        idx = fq.OvernightIndex.sofr()
        for k in (0.03, 0.04, 0.05):
            p = model.european_swaption(swaption("payer", k))
            r = model.european_swaption(swaption("receiver", k))
            fwd = swaption("payer", k).underlying.npv(idx, curve)
            assert p.pv - r.pv == pytest.approx(fwd, abs=1e-6)
            assert p.exercise_boundary == r.exercise_boundary
            assert p.implied_normal_vol is not None and r.implied_normal_vol == pytest.approx(p.implied_normal_vol, abs=1e-9)
            # The implied vol reprices through the fq-rates Bachelier pricer.
            market = swaption("payer", k).price(idx, curve, normal_vol=p.implied_normal_vol)
            assert market.pv == pytest.approx(p.pv, abs=1e-6)
            assert market.forward_rate == pytest.approx(p.forward_rate, abs=1e-13)
        assert "HwSwaptionPricing(" in repr(p)
        d = p.to_dict()
        assert d["pv"] == p.pv and d["implied_normal_vol"] == p.implied_normal_vol
        assert p.strike == 0.05 and p.expiry_time == pytest.approx(1.0)

    def test_forward_and_annuity(self, curve, model):
        sw = swaption()
        f, a = sw.forward_and_annuity(fq.OvernightIndex.sofr(), curve)
        f2, a2 = model.forward_and_annuity(sw.underlying, sw.expiry)
        assert f2 == pytest.approx(f, abs=1e-13) and a2 == pytest.approx(a, abs=1e-6)
        # Default from-date is today: the whole swap.
        assert model.forward_and_annuity(sw.underlying) == (f2, a2)

    def test_vol_scales_with_sigma_and_expired_raises(self, model):
        v1 = model.european_swaption(swaption()).implied_normal_vol
        v2 = model.with_params(0.05, 0.02).european_swaption(swaption()).implied_normal_vol
        assert v2 / v1 == pytest.approx(2.0, abs=0.02)
        assert 0.0075 < v1 < 0.0095
        with pytest.raises(fq.FerrumError):
            model.european_swaption(swaption(expiry="2026-10-13"))


# ---------------------------------------------------------------------------
# Bermudans
# ---------------------------------------------------------------------------


class TestBermudan:
    def test_construction(self, model):
        b = fq.BermudanSwaption.usd_sofr_coterminal("2027-10-13", "5Y", "payer", 1e6, 0.04)
        assert len(b.exercise_dates) == 5 and b.exercise_dates[0] == fq.Date(2027, 10, 13)
        assert b.kind == "payer" and b.strike == 0.04
        assert b.underlying.maturity_date == fq.Date(2032, 10, 15)
        assert b.exercise_dates[1] < b.underlying.periods()[1]["accrual_start"]
        assert "BermudanSwaption(" in repr(b)
        swap = b.underlying
        with pytest.raises(fq.FerrumError):
            fq.BermudanSwaption([], "payer", swap)
        with pytest.raises(fq.FerrumError):
            fq.BermudanSwaption(["2027-10-13"], "receiver", swap)
        with pytest.raises(fq.FerrumError):
            fq.BermudanSwaption(["2028-10-13", "2027-10-13"], "payer", swap)
        with pytest.raises(fq.FerrumError):
            model.bermudan_swaption(fq.BermudanSwaption(["2026-10-13"], "payer", swap))
        with pytest.raises(fq.FerrumError):
            model.bermudan_swaption(b, steps_per_year=0)

    def test_tree_european_converges_to_closed_form(self, model):
        for kind in ("payer", "receiver"):
            sw = swaption(kind)
            closed = model.european_swaption(sw).pv
            single = fq.BermudanSwaption([sw.expiry], kind, sw.underlying)
            coarse = model.bermudan_swaption(single, 24)
            fine = model.bermudan_swaption(single, 200)
            assert abs(coarse - closed) / closed < 0.01
            assert abs(fine - closed) / closed < 2e-3
            assert abs(fine - closed) < abs(coarse - closed)

    def test_bermudan_bounds(self, model):
        b = fq.BermudanSwaption.usd_sofr_coterminal("2027-10-13", "5Y", "payer", 1e6, 0.04)
        berm = model.bermudan_swaption(b, 100)
        for ex in b.exercise_dates:
            assert berm >= model.bermudan_swaption(fq.BermudanSwaption([ex], "payer", b.underlying), 100) - 1e-9
        first = model.european_swaption(fq.Swaption(b.exercise_dates[0], "payer", b.underlying)).pv
        assert first < berm < 1.5 * first
        assert model.with_params(0.05, 0.015).bermudan_swaption(b, 100) > berm


# ---------------------------------------------------------------------------
# Simulation
# ---------------------------------------------------------------------------


class TestSimulation:
    def test_shape_and_validation(self, model):
        sim = model.simulate([0.5, 1.0, 2.0], paths=100, seed=1)
        assert sim.n_paths == 100 and sim.n_steps == 3 and sim.times() == [0.5, 1.0, 2.0]
        assert sim.factor(0, 2) == pytest.approx(-sim.factor(1, 2), abs=1e-15)  # antithetic
        assert len(sim.factors(1)) == 100 and sim.factors(1)[7] == sim.factor(7, 1)
        assert sim.short_rate(3, 0) == pytest.approx(sim.factor(3, 0) + model.short_rate_mean(0.5), rel=1e-12)
        assert sim.short_rates(0)[3] == sim.short_rate(3, 0)
        assert sim.discounts(2)[0] == sim.discount_to(0, 2)
        assert sim.bond(0, 1, 1.0) == 1.0
        assert "ShortRatePaths(" in repr(sim)
        # Reproducible.
        assert model.simulate([0.5, 1.0, 2.0], paths=100, seed=1).factors(2) == sim.factors(2)
        for bad in ([], [1.0, 0.5], [0.0]):
            with pytest.raises(fq.FerrumError):
                model.simulate(bad)
        with pytest.raises(fq.FerrumError):
            model.simulate([1.0], paths=3, antithetic=True)
        with pytest.raises(fq.FerrumError):
            sim.factor(100, 0)
        with pytest.raises(fq.FerrumError):
            sim.discounts(3)
        with pytest.raises(fq.FerrumError):
            sim.bond(0, 2, 1.0)

    def test_bank_account_reprices_curve(self, curve, model):
        sim = model.simulate([1.0, 5.0, 10.0], paths=40_000, seed=3)
        for k, t in enumerate(sim.times()):
            mean, se = sim.estimate(lambda p, k=k: sim.discount_to(p, k))
            assert abs(mean - curve.discount_t(t)) < 4 * max(se, 1e-6)
            mean, se = sim.discounted_bond(k, t + 7.0)
            assert abs(mean - curve.discount_t(t + 7.0)) < 4 * max(se, 1e-6)
        with pytest.raises(fq.FerrumError):
            sim.discounted_bond(1, 2.0)

    def test_estimate_propagates_errors(self, model):
        sim = model.simulate([1.0], paths=10, seed=1)

        def boom(_):
            raise ValueError("payoff failed")

        with pytest.raises(ValueError, match="payoff failed"):
            sim.estimate(boom)

    def test_swaption_by_simulation(self, model):
        sw = swaption()
        closed = model.european_swaption(sw)
        t = model.time(sw.expiry)
        sim = model.simulate([t], paths=60_000, seed=5)
        periods = [p for p in sw.underlying.periods() if p["accrual_start"] >= sw.expiry]
        curve = model.curve

        def payoff(p):
            x = sim.factor(p, 0)
            v = 0.0
            for q in periods:
                s, e, pay = (model.time(q[k]) for k in ("accrual_start", "accrual_end", "payment_date"))
                lag = curve.discount_t(pay) / curve.discount_t(e)
                v += 1e6 * lag * (model.bond(t, s, x) - model.bond(t, e, x)) - q["fixed_amount"] * model.bond(t, pay, x)
            return sim.discount_to(p, 0) * max(v, 0.0)

        mean, se = sim.estimate(payoff)
        assert abs(mean - closed.pv) < 3 * se and se < 0.02 * closed.pv


# ---------------------------------------------------------------------------
# Calibration
# ---------------------------------------------------------------------------


class TestCalibration:
    def atm_grid(self, curve, truth):
        out = []
        for e, t in ((1, "5Y"), (2, "5Y"), (5, "5Y"), (1, "10Y"), (5, "10Y"), (10, "10Y")):
            expiry = fq.Date(2026 + e, 10, 13)
            probe = fq.Swaption.usd_sofr(expiry, t, "payer", 1e6, 0.04)
            f, _ = truth.forward_and_annuity(probe.underlying, expiry)
            sw = fq.Swaption.usd_sofr(expiry, t, "payer", 1e6, f)
            out.append((sw, truth.european_swaption(sw).implied_normal_vol))
        return out

    def test_recovers_generating_parameters(self, curve):
        truth = fq.HullWhite(curve, 0.08, 0.012)
        quotes = self.atm_grid(curve, truth)
        r = fq.calibrate_hull_white(curve, quotes, mode="sigma", a=0.08, sigma=0.02)
        assert r.converged and r.rmse < 1e-7
        assert r.model.sigma == pytest.approx(0.012, abs=1e-7) and r.model.mean_reversion == 0.08
        fits = r.fits()
        assert len(fits) == 6 and all(f["model_vol"] == pytest.approx(f["market_vol"], abs=1e-7) for f in fits)
        both = fq.calibrate_hull_white(curve, quotes, mode="both", a=0.03, sigma=0.008)
        assert both.model.mean_reversion == pytest.approx(0.08, abs=1e-4)
        assert both.model.sigma == pytest.approx(0.012, abs=1e-6)
        assert "HwCalibration(" in repr(both) and both.iterations > 0

    def test_invalid(self, curve):
        with pytest.raises(fq.FerrumError):
            fq.calibrate_hull_white(curve, [])
        with pytest.raises(fq.FerrumError):
            fq.calibrate_hull_white(curve, [(swaption(), -0.01)])
        with pytest.raises(fq.FerrumError):
            fq.calibrate_hull_white(curve, [(swaption(), 0.008)], mode="all")
        with pytest.raises(fq.FerrumError):
            fq.calibrate_hull_white(curve, [(swaption(expiry="2026-10-13"), 0.008)])


# ---------------------------------------------------------------------------
# ModelView
# ---------------------------------------------------------------------------


class TestModelView:
    def test_fit_and_views(self, cv):
        vv = VolView(cv, sample_cube_quotes(cv))
        mv = ModelView(cv, vv, mean_reversion=0.05)
        assert mv.calibration.converged
        assert mv.model.mean_reversion == 0.05 and 0.005 < mv.model.sigma < 0.02
        rows = mv.fit_rows()
        assert len(rows) == len(mv.expiries) * len(mv.tenors) == 20
        # A single-sigma model cannot fit a vol surface that decays with tenor; the
        # fit sits in the middle and the rmse is a few bp.
        assert 2 * BP < mv.calibration.rmse < 20 * BP
        assert abs(sum(r["error"] for r in rows)) < 5 * BP * len(rows)
        text = mv.format_fit()
        assert "Hull-White fit" in text and "Err bp" in text
        brows = mv.bermudan_rows(sample_bermudans(), steps_per_year=50)
        by = {r["label"]: r for r in brows}
        for r in brows:
            assert r["premium"] * (1 if r["notional"] > 0 else -1) >= -1e-6
        assert by["2Yx5Y berm rec"]["npv"] < 0 and by["2Yx5Y berm rec"]["vega"] < 0  # short
        assert by["1Yx5Y berm pay"]["dv01"] > 0 and by["5Yx5Y berm rec"]["dv01"] < 0
        assert by["1Yx5Y berm pay"]["exercises"] == 5
        text = mv.format_bermudans(sample_bermudans(), steps_per_year=50)
        assert "Bermudan swaptions" in text and text.count("\n") == len(brows) + 4
        prow = mv.path_rows(horizons=(1.0, 5.0), paths=4000)
        for r in prow:
            assert abs(r["mc_df"] - r["curve_df"]) < 4 * max(r["mc_df_se"], 1e-6)
            assert r["mean_short_rate"] == pytest.approx(r["model_short_rate"], abs=1e-12)
        assert "Hull-White paths" in mv.format_paths(horizons=(1.0,), paths=2000)

    def test_to_dataframe_if_pandas(self, cv):
        pd = pytest.importorskip("pandas")
        mv = ModelView(cv, VolView(cv, sample_cube_quotes(cv)), expiries=("1Y",), tenors=("5Y", "10Y"))
        assert isinstance(mv.to_dataframe(), pd.DataFrame)


# ---------------------------------------------------------------------------
# QuantLib
# ---------------------------------------------------------------------------


def _ql_curve(curve):
    today = ql.Date(13, 10, 2026)
    ql.Settings.instance().evaluationDate = today
    dates = [today] + [ql.Date(13, 10, y) for y in (2027, 2029, 2031, 2036, 2056)]
    dfs = [1.0] + [curve.discount(d) for d, _ in NODES]
    return ql.YieldTermStructureHandle(ql.DiscountCurve(dates, dfs, ql.Actual365Fixed()))


@needs_ql
class TestAgainstQuantLib:
    def test_zcb_options(self, curve, model):
        hw = ql.HullWhite(_ql_curve(curve), 0.05, 0.01)
        for t, s, k in ((2.0, 7.0, 0.8), (1.0, 3.0, 0.92), (5.0, 30.0, 0.3)):
            assert model.zcb_option("call", t, s, k) == pytest.approx(hw.discountBondOption(ql.Option.Call, k, t, s), rel=1e-12)
            assert model.zcb_option("put", t, s, k) == pytest.approx(hw.discountBondOption(ql.Option.Put, k, t, s), rel=1e-12)

    @pytest.mark.parametrize("strike", [0.03, 0.04, 0.05])
    def test_european_swaption_vs_fd_engine(self, curve, model, strike):
        qc = _ql_curve(curve)
        hw = ql.HullWhite(qc, 0.05, 0.01)
        cal = ql.UnitedStates(ql.UnitedStates.GovernmentBond)
        exp = ql.Date(13, 10, 2027)
        eff = cal.advance(exp, 2, ql.Days)
        for kind, qkind in (("payer", ql.Swap.Payer), ("receiver", ql.Swap.Receiver)):
            ois = ql.MakeOIS(ql.Period(10, ql.Years), ql.Sofr(qc), strike, ql.Period(0, ql.Days), paymentLag=2, discountingTermStructure=qc,
                             nominal=1e6, swapType=qkind, effectiveDate=eff)
            swo = ql.Swaption(ois, ql.EuropeanExercise(exp))
            swo.setPricingEngine(ql.FdHullWhiteSwaptionEngine(hw, 200, 400))
            ours = model.european_swaption(swaption(kind, strike))
            assert ours.forward_rate == pytest.approx(ois.fairRate(), abs=1e-12)
            # QuantLib's FD engine carries the two-day payment lag stochastically; we
            # treat it as a deterministic factor, worth under 10 currency units
            # (0.1bp of notional) across these strikes.
            assert ours.pv == pytest.approx(swo.NPV(), abs=15.0), (kind, strike)

    def test_bermudan_vs_fd_engine(self, curve, model):
        qc = _ql_curve(curve)
        hw = ql.HullWhite(qc, 0.05, 0.01)
        cal = ql.UnitedStates(ql.UnitedStates.GovernmentBond)
        b = fq.BermudanSwaption.usd_sofr_coterminal("2027-10-13", "5Y", "payer", 1e6, 0.04)
        eff = cal.advance(ql.Date(13, 10, 2027), 2, ql.Days)
        ois = ql.MakeOIS(ql.Period(5, ql.Years), ql.Sofr(qc), 0.04, ql.Period(0, ql.Days), paymentLag=2, discountingTermStructure=qc,
                         nominal=1e6, swapType=ql.Swap.Payer, effectiveDate=eff)
        assert [str(p["accrual_start"]) for p in b.underlying.periods()] == [cf.accrualStartDate().ISO() for cf in map(ql.as_coupon, ois.fixedLeg())]
        swo = ql.Swaption(ois, ql.BermudanExercise([ql.Date(d.day, d.month, d.year) for d in b.exercise_dates]))
        swo.setPricingEngine(ql.FdHullWhiteSwaptionEngine(hw, 400, 800))
        assert model.bermudan_swaption(b, 400) == pytest.approx(swo.NPV(), rel=5e-4)
