"""Bond tests for the ferrumquant Python module and the fqview blotter.

Two layers:
  * self-consistency and API tests that need only ``ferrumquant``;
  * a parameter sweep against QuantLib-Python (skipped if QuantLib is not
    installed) covering accrued, price<->yield, duration, convexity and
    DV01 over coupons x maturities x settlement dates, including odd first
    periods, end-of-month schedules and bonds in their final period.

Run from the repo root with ``pytest python/tests`` after
``maturin develop`` in crates/fq-py.
"""

from __future__ import annotations

import datetime as dt
import itertools

import pytest

import ferrumquant as fq
from fqview import TreasuryBlotter, TreasuryPosition, sample_book

try:
    import QuantLib as ql  # type: ignore  # noqa: E402
except ImportError:  # pragma: no cover
    ql = None

needs_ql = pytest.mark.skipif(ql is None, reason="QuantLib-Python not installed")


# ---------------------------------------------------------------------------
# Date
# ---------------------------------------------------------------------------


class TestDate:
    def test_construct_and_accessors(self):
        d = fq.Date(2026, 10, 16)
        assert (d.year, d.month, d.day) == (2026, 10, 16)
        assert d.weekday == "Friday"
        assert str(d) == "2026-10-16"
        assert repr(d) == "Date(2026-10-16)"
        assert d.serial == 46311  # Excel serial for 2026-10-16

    def test_accepts_many_inputs(self):
        d = fq.Date(2026, 10, 16)
        assert fq.Date.from_any("2026-10-16") == d
        assert fq.Date.from_any(dt.date(2026, 10, 16)) == d
        assert fq.Date.from_any(dt.datetime(2026, 10, 16, 9, 30)) == d
        assert fq.Date.from_any(46311) == d
        assert fq.Date.from_any(d) == d
        assert d.to_pydate() == dt.date(2026, 10, 16)

    def test_arithmetic_and_ordering(self):
        d = fq.Date(2026, 10, 16)
        assert d + 3 == fq.Date(2026, 10, 19)
        assert 3 + d == fq.Date(2026, 10, 19)
        assert d - 16 == fq.Date(2026, 9, 30)
        assert fq.Date(2026, 10, 19) - d == 3
        assert d < fq.Date(2026, 10, 17)
        assert d.add_months(4) == fq.Date(2027, 2, 16)
        assert fq.Date(2026, 1, 31).add_months(1) == fq.Date(2026, 2, 28)
        assert fq.Date(2026, 2, 28).is_end_of_month()
        assert fq.Date(2026, 2, 10).end_of_month() == fq.Date(2026, 2, 28)
        assert len({d, fq.Date(2026, 10, 16)}) == 1  # hashable

    def test_invalid(self):
        with pytest.raises(fq.FerrumError):
            fq.Date(2026, 2, 30)
        with pytest.raises(fq.FerrumError):
            fq.Date.from_any("16/10/2026")
        with pytest.raises(fq.FerrumError):
            fq.Date.from_any(3.5)


# ---------------------------------------------------------------------------
# 32nds
# ---------------------------------------------------------------------------


class TestThirtySeconds:
    @pytest.mark.parametrize(
        "quote, price",
        [
            ("99-16", 99.5),
            ("99-16+", 99.515625),
            ("99-162", 99.5078125),
            ("99-166", 99.5234375),
            ("99-00", 99.0),
            ("100-00", 100.0),
            ("99.5", 99.5),
        ],
    )
    def test_parse(self, quote, price):
        assert fq.parse_32nds(quote) == pytest.approx(price, abs=1e-12)

    def test_round_trip(self):
        for n in range(0, 32 * 8):
            price = 99.0 + n / 256.0
            assert fq.parse_32nds(fq.format_32nds(price)) == pytest.approx(price, abs=1e-12)

    def test_bad_quote(self):
        with pytest.raises(fq.FerrumError):
            fq.parse_32nds("99-32")
        with pytest.raises(fq.FerrumError):
            fq.parse_32nds("abc")


# ---------------------------------------------------------------------------
# FixedRateBond: API and self-consistency
# ---------------------------------------------------------------------------


@pytest.fixture
def ust_10y():
    return fq.FixedRateBond.us_treasury("2026-08-15", "2036-08-15", 0.04375)


class TestFixedRateBond:
    def test_basics(self, ust_10y):
        b = ust_10y
        assert b.coupon_rate == 0.04375
        assert b.face == 100.0
        assert b.maturity == fq.Date(2036, 8, 15)
        assert b.dated_date == fq.Date(2026, 8, 15)
        cps = b.coupons()
        assert len(cps) == 20
        assert cps[0]["accrual_start"] == fq.Date(2026, 8, 15)
        assert cps[0]["accrual_end"] == fq.Date(2027, 2, 15)
        assert cps[0]["payment_date"] == fq.Date(2027, 2, 16)  # 15 Feb 2027 is Presidents' Day
        assert all(c["amount"] == pytest.approx(2.1875) for c in cps)

    def test_settlement_is_t_plus_one_on_bond_calendar(self, ust_10y):
        b = ust_10y
        assert b.settlement_date("2026-10-16") == fq.Date(2026, 10, 19)  # Friday -> Monday
        assert b.settlement_date("2026-11-10") == fq.Date(2026, 11, 12)  # Veterans Day skipped
        assert b.settlement_date(dt.date(2026, 10, 14)) == fq.Date(2026, 10, 15)

    def test_price_yield_round_trip(self, ust_10y):
        b = ust_10y
        settle = fq.Date(2026, 10, 19)
        for y in (0.0, 0.01, 0.04375, 0.06, 0.12):
            p = b.clean_price(y, settle)
            assert b.yield_from_clean(p, settle) == pytest.approx(y, abs=1e-12)

    def test_par_bond_prices_at_par_on_a_coupon_date(self):
        b = fq.FixedRateBond.us_treasury("2026-08-15", "2036-08-15", 0.04)
        assert b.clean_price(0.04, "2027-02-15") == pytest.approx(100.0, abs=1e-10)
        assert b.accrued("2027-02-15") == pytest.approx(0.0, abs=1e-12)

    def test_analytics_internally_consistent(self, ust_10y):
        b = ust_10y
        settle = fq.Date(2026, 10, 19)
        a = b.analytics(0.0445, settle)
        assert a.dirty_price == pytest.approx(a.clean_price + a.accrued, abs=1e-12)
        assert a.modified_duration == pytest.approx(a.macaulay_duration / (1 + a.yield_rate / 2), rel=1e-12)
        assert a.dv01 == pytest.approx(a.modified_duration * a.dirty_price * 1e-4, rel=1e-12)
        # Finite-difference check of DV01 and convexity.
        up, dn = b.dirty_price(0.0445 + 1e-4, settle), b.dirty_price(0.0445 - 1e-4, settle)
        assert (dn - up) / 2 == pytest.approx(a.dv01, rel=1e-6)
        fd_convexity = (up + dn - 2 * a.dirty_price) / (1e-4**2) / a.dirty_price
        assert fd_convexity == pytest.approx(a.convexity, rel=1e-5)
        assert a.to_dict()["dv01"] == a.dv01
        assert "YieldAnalytics(" in repr(a)

    def test_analytics_from_price_matches_analytics(self, ust_10y):
        b = ust_10y
        settle = "2026-10-19"
        a1 = b.analytics_from_price(99.515625, settle)
        a2 = b.analytics(a1.yield_rate, settle)
        assert a1.clean_price == pytest.approx(99.515625, abs=1e-10)
        assert a1.to_dict() == pytest.approx(a2.to_dict())

    def test_errors(self, ust_10y):
        with pytest.raises(fq.FerrumError):
            fq.FixedRateBond.us_treasury("2036-08-15", "2026-08-15", 0.04)
        with pytest.raises(fq.FerrumError):
            ust_10y.clean_price(0.04, "2026-08-14")  # before dated date
        with pytest.raises(fq.FerrumError):
            ust_10y.clean_price(0.04, "2036-08-15")  # on/after maturity


# ---------------------------------------------------------------------------
# Blotter
# ---------------------------------------------------------------------------


class TestBlotter:
    def test_rows_and_totals(self):
        blotter = TreasuryBlotter("2026-10-16", sample_book())
        rows = blotter.rows()
        assert len(rows) == len(sample_book())
        for r in rows:
            assert r["settle"] == fq.Date(2026, 10, 19)
            assert r["dirty_price"] == pytest.approx(r["clean_price"] + r["accrued"])
            assert r["position_dv01"] == pytest.approx(r["dv01_per_100"] * r["notional"] / 100)
            assert fq.parse_32nds(r["price_32"]) == pytest.approx(r["clean_price"], abs=1e-9)
        short = next(r for r in rows if r["notional"] < 0)
        assert short["position_dv01"] < 0 and short["market_value"] < 0
        t = blotter.totals(rows)
        assert t["position_dv01"] == pytest.approx(sum(r["position_dv01"] for r in rows))
        assert t["notional"] == pytest.approx(sum(p.notional for p in sample_book()))
        # Yields should rise along this sample curve's belly and long end.
        ys = [r["yield"] for r in rows if r["notional"] > 0]
        assert ys == sorted(ys)

    def test_format_has_header_and_total(self):
        text = TreasuryBlotter("2026-10-16", sample_book()).format()
        assert "Security" in text and "TOTAL" in text
        assert text.count("\n") == len(sample_book()) + 4

    def test_position_accepts_decimal_price_and_odd_first_coupon(self):
        p = TreasuryPosition("odd", "2026-10-15", "2031-08-15", 0.04, 99.25, 1e6, first_coupon="2027-02-15")
        rows = TreasuryBlotter("2026-11-13", [p]).rows()
        assert rows[0]["clean_price"] == pytest.approx(99.25, abs=1e-10)

    def test_to_dataframe_if_pandas(self):
        pd = pytest.importorskip("pandas")
        df = TreasuryBlotter("2026-10-16", sample_book()).to_dataframe()
        assert isinstance(df, pd.DataFrame) and "dv01_per_100" in df.columns


# ---------------------------------------------------------------------------
# QuantLib sweep
# ---------------------------------------------------------------------------


def _ql_date(d: fq.Date):
    return ql.Date(d.day, d.month, d.year)


def _ql_ust(dated: fq.Date, maturity: fq.Date, coupon: float, eom: bool, first_coupon=None):
    cal = ql.UnitedStates(ql.UnitedStates.GovernmentBond)
    if first_coupon is None:
        sched = ql.Schedule(
            _ql_date(dated), _ql_date(maturity), ql.Period(ql.Semiannual), cal,
            ql.Unadjusted, ql.Unadjusted, ql.DateGeneration.Backward, eom,
        )
    else:
        sched = ql.Schedule(
            _ql_date(dated), _ql_date(maturity), ql.Period(ql.Semiannual), cal,
            ql.Unadjusted, ql.Unadjusted, ql.DateGeneration.Backward, eom, _ql_date(first_coupon),
        )
    dc = ql.ActualActual(ql.ActualActual.ISMA, sched)
    bond = ql.FixedRateBond(1, 100.0, sched, [coupon], dc, ql.Unadjusted, 100.0, _ql_date(dated))
    return bond, dc, sched


def _ql_compounding(sched, settle: fq.Date):
    """Street convention: simple in the final coupon period, else compounded."""
    penultimate = sched.dates()[-2]
    return ql.SimpleThenCompounded if _ql_date(settle) >= penultimate else ql.Compounded


# A sweep that covers short, belly and long maturities, high and low coupons,
# settlement right after a coupon, mid-period, just before a coupon, and in
# the final period.
COUPONS = (0.00125, 0.02, 0.04375, 0.07)
MATURITY_YEARS = (2, 5, 10, 30)
SETTLE_OFFSETS = (1, 45, 181, 364)  # days after the dated date


@needs_ql
class TestAgainstQuantLib:
    @pytest.mark.parametrize("coupon, years, offset", list(itertools.product(COUPONS, MATURITY_YEARS, SETTLE_OFFSETS)))
    def test_sweep(self, coupon, years, offset):
        dated = fq.Date(2026, 8, 15)
        maturity = dated.add_years(years)
        settle = dated + offset
        b = fq.FixedRateBond.us_treasury(dated, maturity, coupon)
        qb, dc, sched = _ql_ust(dated, maturity, coupon, eom=False)
        ql.Settings.instance().evaluationDate = _ql_date(settle)
        s = _ql_date(settle)
        comp = _ql_compounding(sched, settle)

        # Accrued and clean price at a given yield.
        y = coupon + 0.005
        assert b.accrued(settle) == pytest.approx(ql.BondFunctions.accruedAmount(qb, s), abs=1e-10)
        rate = ql.InterestRate(y, dc, comp, ql.Semiannual)
        ql_clean = ql.BondFunctions.cleanPrice(qb, rate, s)
        assert b.clean_price(y, settle) == pytest.approx(ql_clean, abs=1e-9)

        # Yield from price, both directions.
        ql_y = ql.BondFunctions.bondYield(qb, ql.BondPrice(ql_clean, ql.BondPrice.Clean), dc, comp, ql.Semiannual, s, 1e-14, 200)
        assert b.yield_from_clean(ql_clean, settle) == pytest.approx(ql_y, abs=1e-10)

        # Risk at that yield.
        a = b.analytics(y, settle)
        assert a.modified_duration == pytest.approx(ql.BondFunctions.duration(qb, rate, ql.Duration.Modified, s), abs=1e-9)
        assert a.macaulay_duration == pytest.approx(ql.BondFunctions.duration(qb, rate, ql.Duration.Macaulay, s), abs=1e-9)
        assert a.convexity == pytest.approx(ql.BondFunctions.convexity(qb, rate, s), abs=1e-7)
        ql_dirty = ql_clean + ql.BondFunctions.accruedAmount(qb, s)
        assert a.dirty_price == pytest.approx(ql_dirty, abs=1e-9)
        assert a.dv01 == pytest.approx(ql.BondFunctions.duration(qb, rate, ql.Duration.Modified, s) * ql_dirty * 1e-4, abs=1e-10)

    def test_odd_first_coupon(self):
        dated, maturity, first = fq.Date(2026, 10, 15), fq.Date(2031, 8, 15), fq.Date(2027, 2, 15)
        b = fq.FixedRateBond.us_treasury(dated, maturity, 0.04, first)
        qb, dc, sched = _ql_ust(dated, maturity, 0.04, eom=False, first_coupon=first)
        settle = fq.Date(2026, 11, 16)
        ql.Settings.instance().evaluationDate = _ql_date(settle)
        s = _ql_date(settle)
        assert b.coupons()[0]["amount"] == pytest.approx(qb.cashflows()[0].amount(), abs=1e-12)
        assert b.accrued(settle) == pytest.approx(ql.BondFunctions.accruedAmount(qb, s), abs=1e-10)
        rate = ql.InterestRate(0.041, dc, ql.Compounded, ql.Semiannual)
        assert b.clean_price(0.041, settle) == pytest.approx(ql.BondFunctions.cleanPrice(qb, rate, s), abs=1e-9)

    def test_end_of_month_schedule(self):
        dated, maturity = fq.Date(2026, 8, 31), fq.Date(2028, 8, 31)
        b = fq.FixedRateBond.us_treasury(dated, maturity, 0.04125)
        qb, dc, sched = _ql_ust(dated, maturity, 0.04125, eom=True)
        ours = [c["accrual_end"] for c in b.coupons()]
        theirs = [fq.Date(d.year(), d.month(), d.dayOfMonth()) for d in list(sched.dates())[1:]]
        assert ours == theirs
        settle = fq.Date(2026, 10, 15)
        ql.Settings.instance().evaluationDate = _ql_date(settle)
        s = _ql_date(settle)
        rate = ql.InterestRate(0.042, dc, ql.Compounded, ql.Semiannual)
        assert b.accrued(settle) == pytest.approx(ql.BondFunctions.accruedAmount(qb, s), abs=1e-10)
        assert b.clean_price(0.042, settle) == pytest.approx(ql.BondFunctions.cleanPrice(qb, rate, s), abs=1e-9)

    def test_sample_book_against_quantlib(self):
        """Every line of the blotter re-priced in QuantLib."""
        trade = fq.Date(2026, 10, 16)
        rows = TreasuryBlotter(trade, sample_book()).rows()
        for p, r in zip(sample_book(), rows):
            dated, maturity = fq.Date.from_any(p.dated), fq.Date.from_any(p.maturity)
            qb, dc, sched = _ql_ust(dated, maturity, p.coupon, eom=maturity.is_end_of_month())
            s = _ql_date(r["settle"])
            ql.Settings.instance().evaluationDate = s
            comp = _ql_compounding(sched, r["settle"])
            ql_y = ql.BondFunctions.bondYield(qb, ql.BondPrice(r["clean_price"], ql.BondPrice.Clean), dc, comp, ql.Semiannual, s, 1e-14, 200)
            assert r["yield"] == pytest.approx(ql_y, abs=1e-10), p.label
            rate = ql.InterestRate(ql_y, dc, comp, ql.Semiannual)
            assert r["mod_duration"] == pytest.approx(ql.BondFunctions.duration(qb, rate, ql.Duration.Modified, s), abs=1e-9), p.label
            assert r["accrued"] == pytest.approx(ql.BondFunctions.accruedAmount(qb, s), abs=1e-10), p.label
