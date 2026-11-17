"""Vol tests: SwaptionVolCube, CapFloor, VolView.

The cube is checked for self-consistency (nodes reprice their quotes, smiles
skew with rho, interpolation lands between nodes, cube pricing equals
direct pricing at the cube's vol). Caps/floors are cross-checked against
QuantLib's BachelierCapFloorEngine on an OvernightLeg.
"""

from __future__ import annotations

import pytest

import ferrumquant as fq
from fqview import CapPosition, CurveView, VolView, sample_caps, sample_cube_quotes, sample_quotes

try:
    import QuantLib as ql  # type: ignore
except ImportError:  # pragma: no cover
    ql = None

needs_ql = pytest.mark.skipif(ql is None, reason="QuantLib-Python not installed")

BP = 1e-4
TODAY = fq.Date(2026, 10, 13)


@pytest.fixture(scope="module")
def flat():
    return fq.OvernightIndex.sofr(), fq.DiscountCurve.flat(TODAY, 0.04, "2040-01-01")


@pytest.fixture(scope="module")
def cv():
    return CurveView(TODAY, sample_quotes())


def small_quotes():
    """2x3 grid with explicit smiles; (5Y, 10Y) has no smile and uses the default."""
    q = {}
    for e in ("1Y", "5Y"):
        for t in ("2Y", "5Y", "10Y"):
            v = 0.0080 if e == "1Y" else 0.0072
            q[(e, t)] = {"forward": 0.038, "atm_vol": v, "smile": [(-0.01, v * 1.12), (-0.005, v * 1.05), (0.005, v * 0.99), (0.01, v * 1.01)]}
    del q[("5Y", "10Y")]["smile"]
    return q


# ---------------------------------------------------------------------------
# SwaptionVolCube
# ---------------------------------------------------------------------------


class TestVolCube:
    def test_build_and_nodes(self):
        cube = fq.SwaptionVolCube(TODAY, ["1Y", "5Y"], ["2Y", "5Y", "10Y"], small_quotes(), default_smile=(-0.2, 0.4))
        assert cube.reference_date == TODAY
        assert [str(p) for p in cube.expiries] == ["1Y", "5Y"] and [str(p) for p in cube.tenors] == ["2Y", "5Y", "10Y"]
        n = cube.node(0, 1)
        assert n["expiry"] == fq.Period("1Y") and n["tenor"] == fq.Period("5Y")
        assert n["atm_vol"] == 0.0080 and n["forward"] == 0.038
        assert n["rho"] < 0 and n["nu"] > 0 and n["rmse"] < 2e-4
        d = cube.node(1, 2)  # default smile
        assert d["rho"] == pytest.approx(-0.2) and d["nu"] == pytest.approx(0.4) and d["rmse"] == 0.0
        with pytest.raises(fq.FerrumError):
            cube.node(2, 0)
        # A sub-grid of the quotes is fine.
        assert len(fq.SwaptionVolCube(TODAY, ["1Y", "5Y"], ["2Y"], small_quotes()).tenors) == 1
        bad = small_quotes()
        del bad[("1Y", "2Y")]
        with pytest.raises(fq.FerrumError):
            fq.SwaptionVolCube(TODAY, ["1Y", "5Y"], ["2Y", "5Y", "10Y"], bad)
        with pytest.raises(fq.FerrumError):
            fq.SwaptionVolCube(TODAY, ["1Y", "5Y"], ["2Y", "5Y", "10Y"], small_quotes())  # (5Y,10Y) has no smile and no default
        assert "SwaptionVolCube(" in repr(cube)

    def test_nodes_reprice_and_smile_skews(self):
        cube = fq.SwaptionVolCube(TODAY, ["1Y", "5Y"], ["2Y", "5Y", "10Y"], small_quotes(), default_smile=(-0.2, 0.4))
        e1 = fq.Period("1Y").add_to(TODAY)
        f = 0.038
        assert cube.normal_vol(e1, "5Y", f, f) == pytest.approx(0.0080, abs=1e-12)
        lo, hi = cube.normal_vol(e1, "5Y", f, f - 0.01), cube.normal_vol(e1, "5Y", f, f + 0.01)
        assert lo == pytest.approx(0.0080 * 1.12, rel=0.02) and hi == pytest.approx(0.0080 * 1.01, rel=0.02)
        assert lo > hi  # negative rho: low strikes richer
        # Grid-time ATM query and SABR params at a node.
        t1 = cube.node_weights(e1, "5Y")
        assert sum(w for _, _, w in t1) == pytest.approx(1.0)
        assert cube.atm_vol((e1 - TODAY) / 365.0, (fq.Period("5Y").add_to(TODAY) - TODAY) / 365.0) == pytest.approx(0.0080, abs=1e-12)
        a, r, n = cube.sabr_at((e1 - TODAY) / 365.0, (fq.Period("5Y").add_to(TODAY) - TODAY) / 365.0)
        assert a > 0 and -1 < r < 0 and n > 0

    def test_interpolation_between_nodes(self):
        cube = fq.SwaptionVolCube(TODAY, ["1Y", "5Y"], ["2Y", "5Y", "10Y"], small_quotes(), default_smile=(-0.2, 0.4))
        e3 = fq.Period("3Y").add_to(TODAY)
        mid = cube.normal_vol(e3, "5Y", 0.038, 0.038)
        assert 0.0072 < mid < 0.0080
        mid7 = cube.normal_vol(e3, "7Y", 0.038, 0.038)
        assert 0.0072 < mid7 < 0.0080
        # Beyond the grid: flat extrapolation.
        e10 = fq.Period("10Y").add_to(TODAY)
        assert cube.normal_vol(e10, "5Y", 0.038, 0.038) == pytest.approx(0.0072, abs=1e-12)

    def test_cube_pricing_and_vega_bump(self, flat):
        idx, c = flat
        cube = fq.SwaptionVolCube(TODAY, ["1Y", "5Y"], ["2Y", "5Y", "10Y"], small_quotes(), default_smile=(-0.2, 0.4))
        sw = fq.Swaption.usd_sofr("2027-10-13", "10Y", "payer", 1e6, 0.045)
        p = cube.price(sw, idx, c)
        f, _ = sw.forward_and_annuity(idx, c)
        vol = cube.normal_vol("2027-10-13", "10Y", f, 0.045)
        assert p.pv == pytest.approx(sw.price(idx, c, normal_vol=vol).pv, rel=1e-12)
        bumped = cube.with_atm_bump(0, 2, 1 * BP)  # (1Y, 10Y) node
        p_up = bumped.price(sw, idx, c)
        assert p_up.pv > p.pv
        assert (p_up.pv - p.pv) == pytest.approx(p.vega * BP, rel=0.05)
        # A node the query does not touch leaves the price alone.
        assert cube.with_atm_bump(1, 0, 1 * BP).price(sw, idx, c).pv == pytest.approx(p.pv, rel=1e-12)


# ---------------------------------------------------------------------------
# CapFloor
# ---------------------------------------------------------------------------


class TestCapFloor:
    def test_structure(self, flat):
        cap = fq.CapFloor.usd_sofr(TODAY, "2Y", "cap", 1e6, 0.04)
        assert cap.kind == "cap" and cap.notional == 1e6 and cap.strike == 0.04
        periods = cap.periods()
        assert len(periods) == 2
        assert periods[0]["accrual_start"] == fq.Date(2026, 10, 15)
        assert periods[0]["fixing_date"] == periods[0]["accrual_end"] - 1 or periods[0]["fixing_date"] < periods[0]["accrual_end"]
        assert periods[1]["payment_date"] > periods[1]["accrual_end"]
        with pytest.raises(fq.FerrumError):
            fq.CapFloor.usd_sofr(TODAY, "2Y", "collar", 1e6, 0.04)
        assert "CapFloor(cap" in repr(cap)

    def test_pricing_identities(self, flat):
        idx, c = flat
        cap = fq.CapFloor.usd_sofr(TODAY, "5Y", "cap", 1e6, 0.035)
        floor = fq.CapFloor.usd_sofr(TODAY, "5Y", "floor", 1e6, 0.035)
        pc, pf = cap.price(idx, c, flat_vol=0.008), floor.price(idx, c, flat_vol=0.008)
        assert len(pc) == 5 and pc.pv == pytest.approx(sum(r["pv"] for r in pc.caplets()), rel=1e-12)
        assert pc.vega == pytest.approx(sum(r["vega"] for r in pc.caplets()), rel=1e-12)
        # Cap - floor = swap: sum of (F - K) * tau * DF * N over the periods.
        parity = sum((r["forward"] - 0.035) * p["year_fraction"] * c.discount(p["payment_date"]) * 1e6 for r, p in zip(pc.caplets(), cap.periods()))
        assert pc.pv - pf.pv == pytest.approx(parity, rel=1e-10)
        assert pc.vega == pytest.approx(pf.vega, rel=1e-12)
        # Strip pricing with equal vols equals flat; npv shortcut agrees.
        assert cap.price(idx, c, vol_strip=[0.008] * 5).pv == pytest.approx(pc.pv, rel=1e-14)
        assert cap.npv(idx, c, flat_vol=0.008) == pytest.approx(pc.pv, rel=1e-14)
        assert cap.implied_flat_vol(idx, c, pc.pv) == pytest.approx(0.008, abs=1e-10)
        with pytest.raises(fq.FerrumError):
            cap.price(idx, c)
        with pytest.raises(fq.FerrumError):
            cap.price(idx, c, vol_strip=[0.008] * 4)

    def test_golden_values(self, flat):
        idx, c = flat
        cap = fq.CapFloor.usd_sofr(TODAY, "2Y", "cap", 1e6, 0.04)
        r = cap.price(idx, c, flat_vol=0.008)
        assert r.pv == pytest.approx(7_604.427570057423, abs=1e-6)
        assert r.caplets()[0]["pv"] == pytest.approx(3_235.255560113149, abs=1e-6)
        assert fq.CapFloor.usd_sofr(TODAY, "2Y", "floor", 1e6, 0.04).npv(idx, c, flat_vol=0.008) == pytest.approx(7_118.469242685865, abs=1e-6)


# ---------------------------------------------------------------------------
# VolView
# ---------------------------------------------------------------------------


class TestVolView:
    def test_cube_and_smile(self, cv):
        vv = VolView(cv, sample_cube_quotes(cv))
        assert vv.expiries == ["3M", "1Y", "2Y", "5Y", "10Y"] and vv.tenors == ["1Y", "2Y", "5Y", "10Y", "30Y"]
        assert vv.cube.node(1, 3)["atm_vol"] == pytest.approx(90 * BP)
        # Forward from the curve equals the swaption's own forward.
        f = vv.forward("1Y", "10Y")
        sw = fq.Swaption.usd_sofr(fq.Period("1Y").add_to(cv.trade_date), "10Y", "payer", 1.0, 0.0)
        assert f == pytest.approx(sw.forward_and_annuity(cv.index, cv.curve)[0], abs=1e-15)
        smile = vv.smile("1Y", "10Y")
        atm = next(r for r in smile if r["offset_bp"] == 0)
        assert atm["vol"] == pytest.approx(90 * BP, abs=1e-12)
        assert smile[0]["vol"] > atm["vol"]  # low-strike skew
        text = vv.format_cube()
        assert "ATM bp" in text and "SABR" in text
        assert "1Yx10Y smile" in vv.format_smile("1Y", "10Y")

    def test_caps(self, cv):
        vv = VolView(cv, sample_cube_quotes(cv))
        rows = vv.cap_rows(sample_caps())
        by = {r["label"]: r for r in rows}
        assert by["2Y cap 4%"]["caplets"] == 2 and by["10Y cap 5%"]["caplets"] == 10
        assert by["5Y floor 3%"]["npv"] < 0 and by["5Y floor 3%"]["vega"] < 0  # short
        assert by["5Y floor 3%"]["dv01"] > 0  # short a floor gains when rates rise
        for r in rows:
            assert r["implied_vol"] == pytest.approx(r["vol"], abs=1e-9)
        text = vv.format_caps(sample_caps())
        assert "Caps/floors" in text and text.count("\n") == len(rows) + 4
        one = vv.cap_rows([CapPosition("x", "3Y", "cap", 1e6, 0.04, 0.008)])
        assert one[0]["caplets"] == 3


# ---------------------------------------------------------------------------
# QuantLib
# ---------------------------------------------------------------------------


@needs_ql
class TestAgainstQuantLib:
    @pytest.mark.parametrize("years, strike", [(2, 0.04), (5, 0.035), (10, 0.045)])
    def test_bachelier_cap_floor(self, flat, years, strike):
        idx, c = flat
        today = ql.Date(13, 10, 2026)
        ql.Settings.instance().evaluationDate = today
        qc = ql.YieldTermStructureHandle(ql.FlatForward(today, 0.04, ql.Actual365Fixed(), ql.Continuous))
        cal = ql.UnitedStates(ql.UnitedStates.GovernmentBond)
        eff = cal.advance(today, 2, ql.Days)
        sched = ql.Schedule(eff, eff + ql.Period(years, ql.Years), ql.Period(ql.Annual), cal, ql.ModifiedFollowing, ql.ModifiedFollowing, ql.DateGeneration.Backward, False)
        leg = ql.OvernightLeg([1e6], sched, ql.Sofr(qc), ql.Actual360(), ql.ModifiedFollowing, [1.0], [0.0], False, ql.RateAveraging.Compound, cal, 2)
        vol = ql.OptionletVolatilityStructureHandle(ql.ConstantOptionletVolatility(today, cal, ql.ModifiedFollowing, 0.008, ql.Actual365Fixed(), ql.Normal))
        qcap, qfloor = ql.Cap(leg, [strike]), ql.Floor(leg, [strike])
        qcap.setPricingEngine(ql.BachelierCapFloorEngine(qc, vol))
        qfloor.setPricingEngine(ql.BachelierCapFloorEngine(qc, vol))
        cap = fq.CapFloor.usd_sofr(TODAY, f"{years}Y", "cap", 1e6, strike)
        floor = fq.CapFloor.usd_sofr(TODAY, f"{years}Y", "floor", 1e6, strike)
        pc = cap.price(idx, c, flat_vol=0.008)
        assert pc.pv == pytest.approx(qcap.NPV(), abs=1e-6)
        assert floor.npv(idx, c, flat_vol=0.008) == pytest.approx(qfloor.NPV(), abs=1e-6)
        assert [str(r["payment_date"]) for r in pc.caplets()] == [cf.date().ISO() for cf in leg]
        assert [r["forward"] for r in pc.caplets()] == pytest.approx([ql.as_floating_rate_coupon(cf).rate() for cf in leg], abs=1e-12)
