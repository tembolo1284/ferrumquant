"""Curve tests: DiscountCurve, OvernightIndex, SOFR bootstrap, Jacobian, CurveView.

Self-consistency tests need only ``ferrumquant``; the QuantLib section
rebuilds the same quote sheets with ``PiecewiseLogLinearDiscount`` and
compares discount factors at the pillars *and* between them (both sides
interpolate log-linearly on ACT/365F, so they should agree everywhere).
"""

from __future__ import annotations

import math

import pytest

import ferrumquant as fq
from fqview import CurveView, sample_quotes

try:
    import QuantLib as ql  # type: ignore
except ImportError:  # pragma: no cover
    ql = None

needs_ql = pytest.mark.skipif(ql is None, reason="QuantLib-Python not installed")

TRADE = fq.Date(2026, 10, 13)

OIS_ONLY = [("OIS", t, r) for t, r in [("1M", 0.0430), ("3M", 0.0425), ("6M", 0.0415), ("1Y", 0.0400), ("2Y", 0.0380),
                                         ("3Y", 0.0375), ("5Y", 0.0378), ("7Y", 0.0385), ("10Y", 0.0395), ("30Y", 0.0410)]]
MIXED = [
    ("DEPO", "1D", 0.0432),
    ("SR3", "Z26", 95.78, 0.0002),
    ("SR3", "H27", 95.95, 0.0002),
    ("SR3", "M27", 96.10, 0.0002),
    ("SR3", "U27", 96.20, 0.0002),
    ("OIS", "2Y", 0.0380),
    ("OIS", "3Y", 0.0375),
    ("OIS", "5Y", 0.0378),
    ("OIS", "10Y", 0.0395),
]


# ---------------------------------------------------------------------------
# Period
# ---------------------------------------------------------------------------


class TestPeriod:
    def test_parse_and_normalise(self):
        assert str(fq.Period("12M")) == "1Y"
        assert str(fq.Period("7D")) == "1W"
        assert fq.Period(5, "Y") == fq.Period("5Y")
        assert fq.Period("ON") == fq.Period("1D")
        assert fq.Period("3M").length == 3 and fq.Period("3M").unit == "M"
        assert fq.Period("5Y").years == 5.0
        assert fq.Period("3M").add_to("2026-10-13") == fq.Date(2027, 1, 13)
        assert repr(fq.Period("10Y")) == "Period(10Y)"
        assert len({fq.Period("1Y"), fq.Period("12M")}) == 1

    def test_invalid(self):
        with pytest.raises(fq.FerrumError):
            fq.Period("5Q")
        with pytest.raises(fq.FerrumError):
            fq.Period("")


# ---------------------------------------------------------------------------
# DiscountCurve
# ---------------------------------------------------------------------------


class TestDiscountCurve:
    def test_flat_curve(self):
        c = fq.DiscountCurve.flat(TRADE, 0.04, "2056-10-13")
        d = fq.Date(2031, 10, 13)
        t = c.time(d)
        assert t == pytest.approx((d - TRADE) / 365.0)
        assert c.discount(d) == pytest.approx(math.exp(-0.04 * t), rel=1e-14)
        assert c.zero_rate(d) == pytest.approx(0.04, abs=1e-14)
        assert c.discount(TRADE) == 1.0
        # Simple ACT/360 forward on a flat cc curve.
        d2 = fq.Date(2032, 1, 13)
        tau = (d2 - d) / 360.0
        assert c.forward_rate(d, d2) == pytest.approx((math.exp(0.04 * (d2 - d) / 365.0) - 1) / tau, rel=1e-12)
        assert c.forward_rate(d, d2, "ACT/365F") == pytest.approx((math.exp(0.04 * (d2 - d) / 365.0) - 1) / ((d2 - d) / 365.0), rel=1e-12)

    def test_nodes_round_trip_and_interpolation(self):
        nodes = [("2027-10-13", 0.96), ("2029-10-13", 0.89), ("2036-10-13", 0.67)]
        c = fq.DiscountCurve(TRADE, nodes)
        assert c.node_count == 3
        assert [df for _, df in c.nodes()] == pytest.approx([0.96, 0.89, 0.67])
        assert c.node_date(1) == fq.Date(2029, 10, 13)
        assert c.node_zero_rate(1) == pytest.approx(-math.log(0.89) / c.time("2029-10-13"))
        # Log-linear: ln DF is linear in t between nodes.
        t1, t2 = c.time("2027-10-13"), c.time("2029-10-13")
        mid = fq.Date(2028, 10, 13)
        w = (c.time(mid) - t1) / (t2 - t1)
        assert math.log(c.discount(mid)) == pytest.approx((1 - w) * math.log(0.96) + w * math.log(0.89), rel=1e-14)
        # from_zero_rates inverts zero_rate.
        z = fq.DiscountCurve.from_zero_rates(TRADE, [(d, c.zero_rate(d)) for d, _ in c.nodes()])
        for d, df in c.nodes():
            assert z.discount(d) == pytest.approx(df, rel=1e-14)
        assert c.interpolation == "log_linear"
        assert fq.DiscountCurve(TRADE, nodes, interpolation="linear_zero").interpolation == "linear_zero"

    def test_shifts(self):
        c = fq.DiscountCurve.flat(TRADE, 0.04, "2056-10-13")
        up = c.shifted(1e-4)
        d = fq.Date(2036, 10, 13)
        assert up.zero_rate(d) == pytest.approx(0.0401, abs=1e-14)
        nodes = [("2027-10-13", 0.96), ("2029-10-13", 0.89), ("2036-10-13", 0.67)]
        c = fq.DiscountCurve(TRADE, nodes)
        b = c.with_node_shift(1, 1e-4)
        assert b.node_zero_rate(1) == pytest.approx(c.node_zero_rate(1) + 1e-4, abs=1e-15)
        assert b.node_zero_rate(0) == c.node_zero_rate(0) and b.node_zero_rate(2) == c.node_zero_rate(2)

    def test_errors(self):
        with pytest.raises(fq.FerrumError):
            fq.DiscountCurve(TRADE, [("2026-10-12", 0.99)])  # before reference
        with pytest.raises(fq.FerrumError):
            fq.DiscountCurve(TRADE, [("2027-10-13", 0.96), ("2027-10-13", 0.95)])  # not increasing
        with pytest.raises(fq.FerrumError):
            fq.DiscountCurve(TRADE, [("2027-10-13", 0.96)], interpolation="cubic")
        c = fq.DiscountCurve.flat(TRADE, 0.04, "2056-10-13")
        with pytest.raises(fq.FerrumError):
            c.discount("2026-10-12")
        with pytest.raises(fq.FerrumError):
            c.with_node_shift(5, 1e-4)
        with pytest.raises(fq.FerrumError):
            c.forward_rate("2027-01-01", "2027-04-01", day_count="ACT/999")


# ---------------------------------------------------------------------------
# OvernightIndex
# ---------------------------------------------------------------------------


class TestOvernightIndex:
    def test_fixings(self):
        idx = fq.OvernightIndex.sofr()
        assert idx.name == "SOFR"
        assert idx.fixing("2026-10-09") is None
        idx.add_fixing("2026-10-09", 0.0431)
        idx.add_fixings([("2026-10-12", 0.0432), ("2026-10-13", 0.0430)])
        assert idx.fixing("2026-10-12") == 0.0432
        assert [str(d) for d, _ in idx.fixings()] == ["2026-10-09", "2026-10-12", "2026-10-13"]
        assert "fixings=3" in repr(idx)


# ---------------------------------------------------------------------------
# Bootstrap and Jacobian: self-consistency
# ---------------------------------------------------------------------------


class TestBootstrap:
    def test_reprices_inputs_and_orders_nodes(self):
        res = fq.bootstrap_sofr(TRADE, fq.OvernightIndex.sofr(), MIXED)
        assert len(res) == len(MIXED)
        assert res.keys == ["DEPO 1D", "SR3 Z26", "SR3 H27", "SR3 M27", "SR3 U27", "OIS 2Y", "OIS 3Y", "OIS 5Y", "OIS 10Y"]
        assert all(abs(e) < 1e-12 for e in res.repricing_errors())
        dates = [d for _, d, _ in res.nodes()]
        assert dates == sorted(dates)
        dfs = [df for _, _, df in res.nodes()]
        assert dfs == sorted(dfs, reverse=True)
        assert res.curve.nodes() == [(d, df) for _, d, df in res.nodes()]
        rows = res.table()
        assert rows[-1]["key"] == "OIS 10Y" and rows[-1]["pillar"] == fq.Date(2036, 10, 17)

    def test_future_convexity_default_and_sr3_pillars(self):
        a = fq.bootstrap_sofr(TRADE, fq.OvernightIndex.sofr(), [("DEPO", "1D", 0.0432), ("SR3", "Z26", 95.78)])
        b = fq.bootstrap_sofr(TRADE, fq.OvernightIndex.sofr(), [("DEPO", "1D", 0.0432), ("SR3", "Z26", 95.78, 0.0)])
        assert a.nodes()[-1] == b.nodes()[-1]
        # Z26 accrues from the Dec-2026 IMM date to the Mar-2027 IMM date.
        assert a.nodes()[-1][1] == fq.Date(2027, 3, 17)

    def test_bad_instruments(self):
        idx = fq.OvernightIndex.sofr()
        with pytest.raises(fq.FerrumError):
            fq.bootstrap_sofr(TRADE, idx, [("FRA", "3M", 0.04)])
        with pytest.raises(fq.FerrumError):
            fq.bootstrap_sofr(TRADE, idx, [("OIS", "5Y")])
        with pytest.raises(fq.FerrumError):
            fq.bootstrap_sofr(TRADE, idx, ["OIS 5Y 0.04"])
        with pytest.raises(fq.FerrumError):
            fq.bootstrap_sofr(TRADE, idx, [])
        with pytest.raises(fq.FerrumError):
            fq.bootstrap_sofr(TRADE, idx, [("OIS", "5Y", 0.04), ("OIS", "60M", 0.04)])  # duplicate node

    def test_jacobian_matches_finite_difference(self):
        res, jac = fq.bootstrap_sofr_with_jacobian(TRADE, fq.OvernightIndex.sofr(), MIXED, bump=1e-4)
        n = len(MIXED)
        assert jac.node_count == n and jac.keys == res.keys and jac.bump == 1e-4
        m = jac.matrix
        # Bump the 5Y OIS quote by hand and compare zero-rate changes.
        j = res.keys.index("OIS 5Y")
        h = 1e-5
        up = list(MIXED)
        up[j] = ("OIS", "5Y", 0.0378 + h)
        dn = list(MIXED)
        dn[j] = ("OIS", "5Y", 0.0378 - h)
        cu = fq.bootstrap_sofr(TRADE, fq.OvernightIndex.sofr(), up).curve
        cd = fq.bootstrap_sofr(TRADE, fq.OvernightIndex.sofr(), dn).curve
        for i in range(n):
            fd = (cu.node_zero_rate(i) - cd.node_zero_rate(i)) / (2 * h)
            assert m[i][j] == pytest.approx(fd, abs=2e-4), (i, j)
        # Triangular structure: a quote cannot move nodes before its own pillar.
        for i in range(n):
            for jj in range(i + 1, n):
                assert abs(m[i][jj]) < 1e-9
        # Diagonal is positive and near 1 for the swaps (par ~ zero at the pillar).
        for i in range(5, n):
            assert 0.9 < m[i][i] < 1.1

    def test_zero_bucket_ladder_and_par_ladder(self):
        res, jac = fq.bootstrap_sofr_with_jacobian(TRADE, fq.OvernightIndex.sofr(), MIXED)
        c = res.curve
        pillar = res.nodes()[-2][1]  # 5Y
        notional = 1e6
        ladder = fq.zero_bucket_ladder(c, lambda cv: cv.discount(pillar) * notional)
        # A zero-coupon claim at a pillar has risk only in that bucket: -t * DF * N.
        t, df = c.time(pillar), c.discount(pillar)
        for i, v in enumerate(ladder):
            if i == len(ladder) - 2:
                assert v == pytest.approx(-t * df * notional, rel=1e-6)
            else:
                assert abs(v) < 1e-6 * notional
        par = jac.to_par_ladder(ladder)
        assert len(par) == len(ladder)
        # A par 5Y swap's PV01 to its own quote equals its annuity; check the sign and
        # that later quotes carry no risk.
        assert par[-2] < 0 and abs(par[-1]) < 1e-6 * notional
        with pytest.raises(fq.FerrumError):
            jac.to_par_ladder(ladder[:-1])

    def test_zero_bucket_ladder_propagates_python_errors(self):
        c = fq.bootstrap_sofr(TRADE, fq.OvernightIndex.sofr(), MIXED).curve

        def boom(_):
            raise ValueError("pricer failed")

        with pytest.raises(ValueError, match="pricer failed"):
            fq.zero_bucket_ladder(c, boom)


# ---------------------------------------------------------------------------
# CurveView
# ---------------------------------------------------------------------------


class TestCurveView:
    def test_rows_and_ladder(self):
        cv = CurveView(TRADE, sample_quotes())
        rows = cv.rows()
        assert [r["key"] for r in rows][:2] == ["DEPO 1D", "SR3 Z26"]
        assert rows[1]["quote"] == 95.78 and rows[1]["kind"] == "SR3"
        assert all(abs(r["repricing_error"]) < 1e-12 for r in rows)
        assert all(0.03 < r["fwd_3m"] < 0.05 for r in rows)
        pillar = next(r["pillar"] for r in rows if r["key"] == "OIS 5Y")
        ladder = cv.par_ladder(lambda c: c.discount(pillar) * 1e6)
        by_key = {r["key"]: r for r in ladder}
        assert by_key["OIS 5Y"]["par_dv01"] < 0
        assert by_key["OIS 10Y"]["par_dv01"] == pytest.approx(0.0, abs=1e-6)
        assert abs(by_key["OIS 7Y"]["zero_dv01"]) < 1e-6
        # Parallel DV01 of the zero-coupon claim equals the zero ladder's sum.
        assert cv.parallel_dv01(lambda c: c.discount(pillar) * 1e6) == pytest.approx(sum(r["zero_dv01"] for r in ladder), rel=1e-6)

    def test_format(self):
        cv = CurveView(TRADE, sample_quotes())
        text = cv.format()
        assert "SOFR curve" in text and "OIS 30Y" in text
        assert text.count("\n") == len(sample_quotes()) + 2
        lad = cv.format_ladder(cv.par_ladder(lambda c: c.discount("2031-10-17")), "x")
        assert "TOTAL" in lad

    def test_to_dataframe_if_pandas(self):
        pd = pytest.importorskip("pandas")
        assert isinstance(CurveView(TRADE, sample_quotes()).to_dataframe(), pd.DataFrame)


# ---------------------------------------------------------------------------
# QuantLib
# ---------------------------------------------------------------------------


def _ql_date(d: fq.Date):
    return ql.Date(d.day, d.month, d.year)


def _ql_curve(trade: fq.Date, quotes):
    ql.Settings.instance().evaluationDate = _ql_date(trade)
    cal = ql.UnitedStates(ql.UnitedStates.GovernmentBond)
    helpers = []
    for q in quotes:
        kind = q[0].upper()
        if kind == "DEPO":
            helpers.append(ql.DepositRateHelper(ql.QuoteHandle(ql.SimpleQuote(q[2])), ql.Period(q[1]), 0, cal, ql.Following, False, ql.Actual360()))
        elif kind == "SR3":
            code, price = q[1], q[2]
            convexity = q[3] if len(q) > 3 else 0.0
            month = {"H": 3, "M": 6, "U": 9, "Z": 12}[code[0]]
            year = 2000 + int(code[1:])
            helpers.append(ql.SofrFutureRateHelper(price, month, year, ql.Quarterly, ql.QuoteHandle(ql.SimpleQuote(convexity))))
        else:
            helpers.append(
                ql.OISRateHelper(2, ql.Period(q[1]), ql.QuoteHandle(ql.SimpleQuote(q[2])), ql.Sofr(), ql.YieldTermStructureHandle(),
                                 False, 2, ql.ModifiedFollowing, ql.Annual, cal, ql.Period(0, ql.Days), 0.0, ql.Pillar.LastRelevantDate)
            )
    curve = ql.PiecewiseLogLinearDiscount(_ql_date(trade), helpers, ql.Actual365Fixed())
    curve.enableExtrapolation()
    return curve, helpers


@needs_ql
class TestAgainstQuantLib:
    @pytest.mark.parametrize("quotes", [OIS_ONLY, MIXED, sample_quotes()], ids=["ois", "mixed", "sample"])
    def test_pillars_and_interpolated_dfs(self, quotes):
        res = fq.bootstrap_sofr(TRADE, fq.OvernightIndex.sofr(), quotes)
        qc, helpers = _ql_curve(TRADE, quotes)
        assert len(helpers) == len(res)
        for h, (_, pillar, df) in zip(helpers, res.nodes()):
            assert h.pillarDate().ISO() == str(pillar)
            assert df == pytest.approx(qc.discount(h.pillarDate()), abs=1e-10)
        # Between pillars: every 37 days out to the last node.
        last = res.nodes()[-1][1]
        d = TRADE + 37
        while d < last:
            assert res.curve.discount(d) == pytest.approx(qc.discount(_ql_date(d)), abs=1e-10), str(d)
            d = d + 37

    def test_zero_rates_match(self):
        res = fq.bootstrap_sofr(TRADE, fq.OvernightIndex.sofr(), MIXED)
        qc, _ = _ql_curve(TRADE, MIXED)
        for d in ("2027-06-01", "2029-01-15", "2033-10-13"):
            ours = res.curve.zero_rate(d)
            theirs = qc.zeroRate(_ql_date(fq.Date.from_any(d)), ql.Actual365Fixed(), ql.Continuous).rate()
            assert ours == pytest.approx(theirs, abs=1e-12)

    def test_forward_rates_match(self):
        res = fq.bootstrap_sofr(TRADE, fq.OvernightIndex.sofr(), MIXED)
        qc, _ = _ql_curve(TRADE, MIXED)
        for d1, d2 in (("2027-01-13", "2027-04-13"), ("2028-06-15", "2028-12-15"), ("2031-10-17", "2032-10-17")):
            a, b = fq.Date.from_any(d1), fq.Date.from_any(d2)
            theirs = qc.forwardRate(_ql_date(a), _ql_date(b), ql.Actual360(), ql.Simple).rate()
            assert res.curve.forward_rate(a, b) == pytest.approx(theirs, abs=1e-12)


# ---------------------------------------------------------------------------
# Review regressions (2026-11-05): invalid inputs must raise, not return NaN
# ---------------------------------------------------------------------------


class TestInvalidInputsRaise:
    def test_linear_zero_extrapolates_flat(self):
        c = fq.DiscountCurve.from_zero_rates(TRADE, [("2027-10-13", 0.03), ("2028-10-13", 0.05)], interpolation="linear_zero")
        assert c.zero_rate("2029-10-13") == pytest.approx(0.05, abs=1e-12)
        assert c.zero_rate("2046-10-13") == pytest.approx(0.05, abs=1e-12)

    def test_bump_validation(self):
        res = fq.bootstrap_sofr(TRADE, fq.OvernightIndex.sofr(), MIXED)
        for bad in (0.0, float("nan"), float("inf")):
            with pytest.raises(fq.FerrumError):
                fq.zero_bucket_ladder(res.curve, lambda c: 1.0, bump=bad)
            with pytest.raises(fq.FerrumError):
                fq.bootstrap_sofr_with_jacobian(TRADE, fq.OvernightIndex.sofr(), MIXED, bump=bad)
        with pytest.raises(fq.FerrumError):
            res.curve.with_node_shift(0, float("nan"))

    def test_bootstrap_keeps_the_real_error(self):
        # SR3 U26 is already running on the trade date; no fixings were loaded.
        with pytest.raises(fq.FerrumError, match="SR3 U26.*fixing"):
            fq.bootstrap_sofr(TRADE, fq.OvernightIndex.sofr(), [("DEPO", "1D", 0.043), ("SR3", "U26", 95.7)])

    def test_swaption_nan_inputs_raise(self):
        idx, c = fq.OvernightIndex.sofr(), fq.DiscountCurve.flat(TRADE, 0.04, "2040-01-01")
        sw = fq.Swaption.usd_sofr("2027-10-13", "5Y", "payer", 1e6, 0.04)
        with pytest.raises(fq.FerrumError):
            sw.price(idx, c, normal_vol=float("nan"))
        with pytest.raises(fq.FerrumError):
            fq.Swaption.usd_sofr("2027-10-13", "5Y", "payer", 1e6, float("nan")).price(idx, c, normal_vol=0.008)
        with pytest.raises(fq.FerrumError):
            sw.price(idx, c, lognormal_vol=0.2, shift=float("inf"))
