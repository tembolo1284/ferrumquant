"""Futures tests: conversion factors, Treasury basis/CTD/delivery option, SOFR futures, bond spreads, BasisScreen."""

from __future__ import annotations

import math

import pytest

import ferrumquant as fq
from fqview import BasisScreen, CurveView, Deliverable, sample_basket, sample_quotes

try:
    import QuantLib as ql  # type: ignore
except ImportError:  # pragma: no cover
    ql = None

needs_ql = pytest.mark.skipif(ql is None, reason="QuantLib-Python not installed")

SETTLE, DELIVERY, REPO = fq.Date(2027, 1, 4), fq.Date(2027, 3, 31), 0.0385


def ust(dated, maturity, cpn):
    return fq.FixedRateBond.us_treasury(dated, maturity, cpn)


@pytest.fixture(scope="module")
def zn():
    return fq.TreasuryFuture("ZN", "H27", [ust("2026-08-15", "2036-08-15", 0.0425), ust("2024-02-15", "2034-02-15", 0.035), ust("2026-08-15", "2056-08-15", 0.045)])


# ---------------------------------------------------------------------------
# Conversion factors and basket
# ---------------------------------------------------------------------------


class TestConversionFactor:
    def test_known_values(self):
        # 6% coupon prices at par at the 6% CF yield.
        assert fq.conversion_factor("ZN", 0.06, "2036-12-31", "H27") == pytest.approx(1.0, abs=2e-3)
        assert fq.conversion_factor("ZN", 0.0425, "2036-08-15", "H27") == pytest.approx(0.8771, abs=5e-5)
        assert fq.conversion_factor("ZN", 0.035, "2034-02-15", (2027, 3)) == pytest.approx(0.8628, abs=5e-5)
        # Lower coupon, lower factor; longer maturity at a sub-6% coupon, lower factor.
        assert fq.conversion_factor("ZB", 0.045, "2056-08-15", "H27") < fq.conversion_factor("ZB", 0.045, "2046-08-15", "H27") < 1.0
        assert fq.conversion_factor("TU", 0.04, "2028-12-31", "H27") == fq.conversion_factor("ZT", 0.04, "2028-12-31", "H27")

    def test_errors(self):
        with pytest.raises(fq.FerrumError):
            fq.conversion_factor("ZZ", 0.04, "2036-08-15", "H27")
        with pytest.raises(fq.FerrumError):
            fq.conversion_factor("ZN", 0.04, "2036-08-15", "K27")
        with pytest.raises(fq.FerrumError):
            fq.conversion_factor("ZN", 0.04, "2036-08-15", (2027, 4))


class TestTreasuryFuture:
    def test_basket_filters_and_dates(self, zn):
        assert len(zn) == 2  # the 30Y is not deliverable into ZN
        assert zn.contract == "ZN" and zn.delivery_month == (2027, 3)
        (b0, cf0), (b1, cf1) = zn.basket()
        assert b0.maturity == fq.Date(2036, 8, 15) and cf0 == pytest.approx(0.8771, abs=5e-5)
        assert b1.maturity == fq.Date(2034, 2, 15) and cf1 == pytest.approx(0.8628, abs=5e-5)
        assert zn.first_delivery_date() == fq.Date(2027, 3, 1)
        assert zn.last_delivery_date() == fq.Date(2027, 3, 31)
        with pytest.raises(fq.FerrumError):
            fq.TreasuryFuture("ZN", "H27", [ust("2026-08-15", "2056-08-15", 0.045)])  # empty basket
        assert "TreasuryFuture(ZN H27" in repr(zn)

    def test_basis_identities(self, zn):
        prices, F = [99.25, 96.75], 110.5
        ctd, all_ = zn.cheapest_to_deliver(prices, F, SETTLE, DELIVERY, REPO)
        assert ctd == max(range(2), key=lambda i: all_[i]["implied_repo"])
        for i, (b, (bond, cf)) in enumerate(zip(all_, zn.basket())):
            assert b["conversion_factor"] == cf
            assert b["gross_basis"] == pytest.approx(prices[i] - F * cf, abs=1e-12)
            assert b["net_basis"] == pytest.approx(b["gross_basis"] - b["carry"], abs=1e-12)
            assert b["carry"] == pytest.approx(prices[i] - b["forward_clean"], abs=1e-12)
            assert b["converted_forward"] == pytest.approx(b["forward_clean"] / cf, rel=1e-12)
            assert b["forward_clean"] == pytest.approx(zn.forward_clean_price(i, prices[i], SETTLE, DELIVERY, REPO), abs=1e-12)
            assert zn.basis(i, prices[i], F, SETTLE, DELIVERY, REPO) == b
            # Invoice at delivery = F*CF + accrued.
            assert zn.invoice_price(i, F, DELIVERY) == pytest.approx(F * cf + bond.accrued(DELIVERY), abs=1e-12)
        # Fair price = min converted forward, and at that price the CTD's net basis is zero.
        idx, fair = zn.fair_price(prices, SETTLE, DELIVERY, REPO)
        assert fair == pytest.approx(min(b["converted_forward"] for b in all_), abs=1e-12)
        at_fair = zn.basis(idx, prices[idx], fair, SETTLE, DELIVERY, REPO)
        assert at_fair["net_basis"] == pytest.approx(0.0, abs=1e-10)
        assert at_fair["implied_repo"] == pytest.approx(REPO, abs=1e-10)

    def test_delivery_option(self, zn):
        prices = [99.25, 96.75]
        r = zn.delivery_option(prices, SETTLE, DELIVERY, REPO)
        idx, fair = zn.fair_price(prices, SETTLE, DELIVERY, REPO)
        assert r.ctd_only_price == pytest.approx(fair, abs=1e-12)
        assert r.switch_option >= 0 and r.fair_price == pytest.approx(min(r.expected_converted) - r.switch_option, abs=1e-12)
        assert sum(r.ctd_probability) == pytest.approx(1.0, abs=1e-12)
        assert r.most_likely_ctd() == idx
        assert len(r.forward_yields) == 2 and all(0.03 < y < 0.06 for y in r.forward_yields)
        # Zero vol collapses to the CTD-only price; more vol, more option.
        zero = zn.delivery_option(prices, SETTLE, DELIVERY, REPO, yield_vol=0.0)
        assert zero.fair_price == pytest.approx(fair, abs=1e-9) and zero.switch_option == 0.0
        hi = zn.delivery_option(prices, SETTLE, DELIVERY, REPO, yield_vol=0.015)
        assert hi.switch_option > r.switch_option
        with pytest.raises(fq.FerrumError):
            zn.delivery_option([99.0], SETTLE, DELIVERY, REPO)
        with pytest.raises(fq.FerrumError):
            zn.delivery_option(prices, DELIVERY, SETTLE, REPO)


# ---------------------------------------------------------------------------
# SOFR futures
# ---------------------------------------------------------------------------


class TestSofrFuture:
    def test_contract(self):
        f = fq.SofrFuture("Z26", 95.78, 0.0002)
        assert f.contract == "Z26" and f.price == 95.78 and f.convexity_adjustment == 0.0002
        assert f.start == fq.Date(2026, 12, 16) and f.end == fq.Date(2027, 3, 17)  # third Wednesdays
        assert f.futures_rate == pytest.approx(0.0422, abs=1e-15)
        assert f.implied_forward == pytest.approx(0.0420, abs=1e-15)
        assert fq.SofrFuture("H27", 96.0).convexity_adjustment == 0.0
        with pytest.raises(fq.FerrumError):
            fq.SofrFuture("K27", 96.0)
        assert "SofrFuture(Z26" in repr(f)

    def test_reprices_on_bootstrapped_curve(self):
        cv = CurveView(fq.Date(2026, 10, 13), sample_quotes())
        for code, price in (("Z26", 95.78), ("H27", 95.95), ("M27", 96.10), ("U27", 96.20)):
            f = fq.SofrFuture(code, price, 0.0002)
            assert f.fair_price(cv.index, cv.curve) == pytest.approx(price, abs=1e-10)
            assert f.curve_rate(cv.index, cv.curve) == pytest.approx(f.implied_forward, abs=1e-12)
        # A contract not in the strip: fair price between its neighbours' rates, with convexity.
        z27 = fq.SofrFuture("Z27", 96.3, 0.0002)
        assert 95 < z27.fair_price(cv.index, cv.curve) < 97

    def test_flat_curve(self):
        idx, c = fq.OvernightIndex.sofr(), fq.DiscountCurve.flat("2026-10-13", 0.04, "2030-01-01")
        f = fq.SofrFuture("Z26", 96.0, 0.0)
        rate = f.curve_rate(idx, c)
        # Compounded SOFR on a flat 4% cc curve over ~91 days, ACT/360.
        days = f.end - f.start
        assert rate == pytest.approx((math.exp(0.04 * days / 365.0) - 1) * 360.0 / days, rel=1e-10)
        assert f.fair_price(idx, c) == pytest.approx(100 * (1 - rate), rel=1e-12)


# ---------------------------------------------------------------------------
# Bond spreads on the curve
# ---------------------------------------------------------------------------


class TestBondSpreads:
    def test_spreads(self):
        cv = CurveView(fq.Date(2026, 10, 13), sample_quotes())
        b = ust("2026-08-15", "2036-08-15", 0.04375)
        settle = b.settlement_date(cv.trade_date)
        s = b.spreads(99.515625, settle, cv.index, cv.curve)
        a = b.analytics_from_price(99.515625, settle)
        assert s.clean_price == 99.515625 and s.dirty_price == pytest.approx(a.dirty_price, abs=1e-12)
        assert s.yield_rate == pytest.approx(a.yield_rate, abs=1e-15)
        # I-spread = yield - matched-maturity OIS rate.
        mm = fq.OisSwap.usd_sofr(settle, b.maturity, "pay", 1.0, 0.0).par_rate(cv.index, cv.curve)
        assert s.i_spread == pytest.approx(a.yield_rate - mm, abs=1e-15)
        # Z-spread reprices the dirty price on the shifted curve.
        shifted = cv.curve.shifted(s.z_spread)
        assert b.npv(shifted) / shifted.discount(settle) == pytest.approx(s.dirty_price, abs=1e-9)
        assert b.npv(cv.curve) / cv.curve.discount(settle) == pytest.approx(s.curve_pv, abs=1e-9)
        # Cheap to swaps: positive spreads in the same neighbourhood.
        assert s.z_spread > 0 and abs(s.z_spread - s.asset_swap_spread) < 0.001 and abs(s.z_spread - s.i_spread) < 0.001
        assert s.to_dict()["z_spread"] == s.z_spread and "BondSpreads(" in repr(s)
        # Curve DV01 is negative for a long bond (PV falls as the curve rises).
        assert b.curve_dv01(cv.curve) < 0
        with pytest.raises(fq.FerrumError):
            b.spreads(99.5, "2026-10-01", cv.index, cv.curve)  # before the curve


# ---------------------------------------------------------------------------
# BasisScreen
# ---------------------------------------------------------------------------


class TestBasisScreen:
    def test_screen(self):
        screen = BasisScreen("ZN", "H27", futures_price=112.0, repo=REPO, trade_date=SETTLE - 1, deliverables=sample_basket())
        assert len(screen.deliverables) == 4  # the 30Y dropped
        assert screen.settle == SETTLE and screen.delivery_date == DELIVERY
        rows = screen.rows()
        assert sum(r["is_ctd"] for r in rows) == 1
        ctd = next(r for r in rows if r["is_ctd"])
        assert ctd["implied_repo"] == max(r["implied_repo"] for r in rows)
        assert sum(r["ctd_probability"] for r in rows) == pytest.approx(1.0, abs=1e-12)
        for r in rows:
            assert r["net_basis_32"] == pytest.approx(r["gross_basis_32"] - r["carry_32"], abs=1e-9)
            assert fq.parse_32nds(r["price_32"]) == pytest.approx(r["price"], abs=1e-9)
        s = screen.summary()
        assert s["ctd"] == ctd["label"]
        assert s["fair_price_with_option"] <= s["fair_price_ctd_only"]
        # Switch option is measured against the expected converted prices (convexity
        # effect excluded), so it is not simply ctd_only - fair.
        opt = screen.future.delivery_option(screen.prices(), SETTLE, DELIVERY, REPO)
        assert s["switch_option_32"] == pytest.approx(opt.switch_option * 32, abs=1e-12)
        assert s["rich_cheap_32"] == pytest.approx((112.0 - opt.fair_price) * 32, abs=1e-9)
        text = screen.format()
        assert "ZN H27 basis" in text and "CTD" in text and text.count("\n") == len(rows) + 4

    def test_explicit_delivery_date_and_dataframe(self):
        screen = BasisScreen("ZN", "H27", 112.0, REPO, SETTLE - 1, sample_basket(), delivery_date="2027-03-01")
        assert screen.delivery_date == fq.Date(2027, 3, 1)
        pd = pytest.importorskip("pandas")
        assert isinstance(screen.to_dataframe(), pd.DataFrame)


# ---------------------------------------------------------------------------
# QuantLib: forward clean price via carry
# ---------------------------------------------------------------------------


@needs_ql
class TestAgainstQuantLib:
    def test_forward_clean_price_matches_cash_and_carry(self, zn):
        """Forward clean = (dirty - PV of coupons at repo) grown at repo, less accrued at delivery."""
        bond, cf = zn.basket()[1]  # 3.5% Feb-34: a coupon falls on 2027-02-15 inside the window
        clean = 96.75
        fwd = zn.forward_clean_price(1, clean, SETTLE, DELIVERY, REPO)
        dirty = clean + bond.accrued(SETTLE)
        tau = (DELIVERY - SETTLE) / 360.0
        coupons = [c for c in bond.coupons() if SETTLE < c["payment_date"] <= DELIVERY]
        assert len(coupons) == 1
        c = coupons[0]
        reinvest = (DELIVERY - c["payment_date"]) / 360.0
        expected = dirty * (1 + REPO * tau) - c["amount"] * (1 + REPO * reinvest) - bond.accrued(DELIVERY)
        assert fwd == pytest.approx(expected, abs=1e-9)
        # And QuantLib agrees on the accrued amounts used.
        sched = ql.Schedule(ql.Date(15, 2, 2024), ql.Date(15, 2, 2034), ql.Period(ql.Semiannual), ql.UnitedStates(ql.UnitedStates.GovernmentBond),
                            ql.Unadjusted, ql.Unadjusted, ql.DateGeneration.Backward, False)
        qb = ql.FixedRateBond(1, 100.0, sched, [0.035], ql.ActualActual(ql.ActualActual.ISMA, sched), ql.Unadjusted, 100.0, ql.Date(15, 2, 2024))
        ql.Settings.instance().evaluationDate = ql.Date(4, 1, 2027)
        assert bond.accrued(SETTLE) == pytest.approx(ql.BondFunctions.accruedAmount(qb, ql.Date(4, 1, 2027)), abs=1e-10)
        assert bond.accrued(DELIVERY) == pytest.approx(ql.BondFunctions.accruedAmount(qb, ql.Date(31, 3, 2027)), abs=1e-10)
