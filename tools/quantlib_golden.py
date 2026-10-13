#!/usr/bin/env python3
"""Reference values for crates/fq-rates/tests/golden_quantlib.rs.

Run with QuantLib-Python (pip install QuantLib) and paste the printed
numbers into the Rust test. Generated with QuantLib 1.43.

Bonds use Unadjusted payment dates so QuantLib discounts to the coupon
dates, matching the Street convention; QuantLib's default would discount
to the business-day-adjusted payment dates.
"""
import QuantLib as ql


def D(y, m, d):
    return ql.Date(d, m, y)


CAL = ql.UnitedStates(ql.UnitedStates.GovernmentBond)


def ust(dated, maturity, coupon, eom=False):
    sched = ql.Schedule(dated, maturity, ql.Period(ql.Semiannual), CAL, ql.Unadjusted, ql.Unadjusted,
                        ql.DateGeneration.Backward, eom)
    dc = ql.ActualActual(ql.ActualActual.ISMA, sched)
    return ql.FixedRateBond(1, 100.0, sched, [coupon], dc, ql.Unadjusted, 100.0, dated), dc


def clean(bond, dc, y, settle, comp=ql.Compounded):
    return ql.BondFunctions.cleanPrice(bond, ql.InterestRate(y, dc, comp, ql.Semiannual), settle)


def bonds():
    ql.Settings.instance().evaluationDate = D(2026, 9, 21)
    b, dc = ust(D(2026, 8, 15), D(2036, 8, 15), 0.0425)
    s = D(2026, 9, 22)
    y = ql.BondFunctions.bondYield(b, ql.BondPrice(99.0, ql.BondPrice.Clean), dc, ql.Compounded, ql.Semiannual, s, 1e-14, 200)
    r = ql.InterestRate(y, dc, ql.Compounded, ql.Semiannual)
    print("UST 4.25% 2036 settle 2026-09-22")
    print("  yield@99     %.15f" % y)
    print("  accrued      %.15f" % ql.BondFunctions.accruedAmount(b, s))
    print("  mod duration %.15f" % ql.BondFunctions.duration(b, r, ql.Duration.Modified, s))
    print("  mac duration %.15f" % ql.BondFunctions.duration(b, r, ql.Duration.Macaulay, s))
    print("  convexity    %.15f" % ql.BondFunctions.convexity(b, r, s))
    print("  clean@4.375%% %.15f" % clean(b, dc, 0.04375, s))

    b, dc = ust(D(2026, 10, 15), D(2031, 8, 15), 0.04)
    s = D(2026, 11, 16)
    print("Short first coupon 4% 2031, settle 2026-11-16")
    print("  accrued      %.15f" % ql.BondFunctions.accruedAmount(b, s))
    print("  first coupon %.15f" % b.cashflows()[0].amount())
    print("  clean@4.1%%   %.15f" % clean(b, dc, 0.041, s))

    b, dc = ust(D(2025, 2, 15), D(2027, 2, 15), 0.04)
    print("Final period 4% 2027, settle 2026-11-16")
    print("  clean@3.9%%   %.15f" % clean(b, dc, 0.039, D(2026, 11, 16), ql.SimpleThenCompounded))

    b, dc = ust(D(2026, 8, 31), D(2028, 8, 31), 0.04125, eom=True)
    s = D(2026, 10, 15)
    print("EOM 4.125% 2028, settle 2026-10-15")
    print("  coupon dates", [str(c.date()) for c in b.cashflows()[:-1]])
    print("  accrued      %.15f" % ql.BondFunctions.accruedAmount(b, s))
    print("  clean@4.2%%   %.15f" % clean(b, dc, 0.042, s))


def swaps_and_swaptions():
    today = D(2026, 9, 23)
    ql.Settings.instance().evaluationDate = today
    curve = ql.YieldTermStructureHandle(ql.FlatForward(today, 0.04, ql.Actual365Fixed(), ql.Continuous))
    sofr = ql.Sofr(curve)
    print("OIS on flat 4% cc curve, ref 2026-09-23")
    for yrs in (1, 5, 10):
        ois = ql.MakeOIS(ql.Period(yrs, ql.Years), sofr, 0.0, ql.Period(0, ql.Days), paymentLag=2, settlementDays=2,
                         discountingTermStructure=curve)
        print("  %2dY par %.15f" % (yrs, ois.fairRate()))
    ois = ql.MakeOIS(ql.Period(5, ql.Years), sofr, 0.04, ql.Period(0, ql.Days), paymentLag=2, settlementDays=2,
                     discountingTermStructure=curve, nominal=1e6, swapType=ql.Swap.Payer)
    print("  5Y payer @4%% 1e6: NPV %.12f fixed PV %.12f float PV %.12f" % (ois.NPV(), -ois.fixedLegNPV(), ois.overnightLegNPV()))
    print("  fixed amounts", ["%.12f" % cf.amount() for cf in ois.fixedLeg()])
    print("  float amounts", ["%.12f" % cf.amount() for cf in ois.overnightLeg()])

    expiry = D(2027, 9, 23)
    eff = CAL.advance(expiry, 2, ql.Days)

    def fwd_swap(side):
        return ql.MakeOIS(ql.Period(10, ql.Years), sofr, 0.04, ql.Period(0, ql.Days), paymentLag=2,
                          discountingTermStructure=curve, nominal=1e6, swapType=side, effectiveDate=eff)

    normal = ql.SwaptionVolatilityStructureHandle(
        ql.ConstantSwaptionVolatility(today, CAL, ql.ModifiedFollowing, 0.008, ql.Actual365Fixed(), ql.Normal))
    shifted = ql.SwaptionVolatilityStructureHandle(
        ql.ConstantSwaptionVolatility(today, CAL, ql.ModifiedFollowing, 0.25, ql.Actual365Fixed(), ql.ShiftedLognormal, 0.02))
    payer = ql.Swaption(fwd_swap(ql.Swap.Payer), ql.EuropeanExercise(expiry))
    receiver = ql.Swaption(fwd_swap(ql.Swap.Receiver), ql.EuropeanExercise(expiry))
    payer.setPricingEngine(ql.BachelierSwaptionEngine(curve, normal))
    receiver.setPricingEngine(ql.BachelierSwaptionEngine(curve, normal))
    print("1Yx10Y swaption, strike 4%, 1e6, normal vol 80bp")
    print("  payer NPV %.12f annuity %.12f vega %.12f delta %.12f" % (payer.NPV(), payer.annuity(), payer.vega(), payer.delta()))
    print("  receiver NPV %.12f" % receiver.NPV())
    payer.setPricingEngine(ql.BlackSwaptionEngine(curve, shifted))
    print("  payer shifted Black (25%%, shift 2%%) NPV %.12f" % payer.NPV())


def bootstrap():
    today = D(2026, 9, 23)
    ql.Settings.instance().evaluationDate = today
    quotes = [("1M", 0.0430), ("3M", 0.0425), ("6M", 0.0415), ("1Y", 0.0400), ("2Y", 0.0380),
              ("3Y", 0.0375), ("5Y", 0.0378), ("7Y", 0.0385), ("10Y", 0.0395), ("30Y", 0.0410)]
    helpers = [ql.OISRateHelper(2, ql.Period(t), ql.QuoteHandle(ql.SimpleQuote(r)), ql.Sofr(), ql.YieldTermStructureHandle(),
                                False, 2, ql.ModifiedFollowing, ql.Annual, CAL, ql.Period(0, ql.Days), 0.0,
                                ql.Pillar.LastRelevantDate) for t, r in quotes]
    curve = ql.PiecewiseLogLinearDiscount(today, helpers, ql.Actual365Fixed())
    print("SOFR bootstrap nodes (date, DF)")
    for h in helpers:
        print("  %s %.15f" % (h.pillarDate().ISO(), curve.discount(h.pillarDate())))


def mixed_bootstrap():
    today = D(2026, 10, 13)
    ql.Settings.instance().evaluationDate = today
    helpers = [ql.DepositRateHelper(ql.QuoteHandle(ql.SimpleQuote(0.0432)), ql.Period(1, ql.Days), 0, CAL,
                                    ql.Following, False, ql.Actual360())]
    for (m, y, p) in [(12, 2026, 95.78), (3, 2027, 95.95), (6, 2027, 96.10), (9, 2027, 96.20)]:
        helpers.append(ql.SofrFutureRateHelper(p, m, y, ql.Quarterly, ql.QuoteHandle(ql.SimpleQuote(0.0002))))
    for t, r in [("2Y", 0.0380), ("3Y", 0.0375), ("5Y", 0.0378), ("10Y", 0.0395)]:
        helpers.append(ql.OISRateHelper(2, ql.Period(t), ql.QuoteHandle(ql.SimpleQuote(r)), ql.Sofr(),
                                        ql.YieldTermStructureHandle(), False, 2, ql.ModifiedFollowing, ql.Annual,
                                        CAL, ql.Period(0, ql.Days), 0.0, ql.Pillar.LastRelevantDate))
    curve = ql.PiecewiseLogLinearDiscount(today, helpers, ql.Actual365Fixed())
    print("Mixed deposit/SR3/OIS bootstrap nodes (date, DF), ref 2026-10-13")
    for h in helpers:
        print("  %s %.15f" % (h.pillarDate().ISO(), curve.discount(h.pillarDate())))


if __name__ == "__main__":
    print("QuantLib", ql.__version__)
    bonds()
    swaps_and_swaptions()
    bootstrap()
    mixed_bootstrap()
