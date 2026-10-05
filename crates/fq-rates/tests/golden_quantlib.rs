//! Golden tests against QuantLib-Python 1.43.
//!
//! Reference values come from `tools/quantlib_golden.py`; regenerate and
//! update here when a convention changes. Bonds compare against QuantLib
//! bonds built with Unadjusted payment dates (Street convention); swaps and
//! swaptions against `MakeOIS` with a 2-day payment lag on a flat 4%
//! continuously-compounded ACT/365F curve; the bootstrap against
//! `PiecewiseLogLinearDiscount` with `Pillar.LastRelevantDate` helpers.

use approx::assert_abs_diff_eq;

use fq_core::time::Date;
use fq_rates::bond::FixedRateBond;
use fq_rates::curve::{BootstrapConfig, BootstrapInstrument, DiscountCurve, bootstrap_sofr};
use fq_rates::index::OvernightIndex;
use fq_rates::swap::{OisSwap, SwapSide, Tenor};
use fq_rates::swaption::{Swaption, SwaptionType, VolQuote};

type R = Result<(), Box<dyn std::error::Error>>;

fn d(y: i32, m: u32, dd: u32) -> Date {
    Date::from_ymd(y, m, dd).expect("valid test date")
}

// ---------------------------------------------------------------------------
// Bonds
// ---------------------------------------------------------------------------

#[test]
fn golden_ust_yield_and_risk() -> R {
    let b = FixedRateBond::us_treasury(d(2026, 8, 15), d(2036, 8, 15), 0.0425, None)?;
    let settle = d(2026, 9, 22);

    assert_abs_diff_eq!(b.accrued(settle)?, 0.438_858_695_652_167, epsilon = 1e-12);

    let y = b.yield_from_clean(99.0, settle)?;
    assert_abs_diff_eq!(y, 0.043_751_048_565_529, epsilon = 1e-12);

    let a = b.analytics(y, settle)?;
    assert_abs_diff_eq!(a.clean_price, 99.0, epsilon = 1e-10);
    assert_abs_diff_eq!(a.modified_duration, 7.961_510_749_763_204, epsilon = 1e-9);
    assert_abs_diff_eq!(a.macaulay_duration, 8.135_672_971_497_140, epsilon = 1e-9);
    assert_abs_diff_eq!(a.convexity, 75.645_351_446_427_483, epsilon = 1e-8);

    assert_abs_diff_eq!(b.clean_price(0.04375, settle)?, 99.000_830_136_207_782, epsilon = 1e-10);
    Ok(())
}

#[test]
fn golden_ust_short_first_coupon() -> R {
    let b = FixedRateBond::us_treasury(d(2026, 10, 15), d(2031, 8, 15), 0.04, None)?;
    let settle = d(2026, 11, 16);
    assert_abs_diff_eq!(b.coupons()[0].amount, 1.336_956_521_739_130, epsilon = 1e-12);
    assert_abs_diff_eq!(b.accrued(settle)?, 0.347_826_086_956_515, epsilon = 1e-12);
    assert_abs_diff_eq!(b.clean_price(0.041, settle)?, 99.574_118_525_022_584, epsilon = 1e-10);
    Ok(())
}

#[test]
fn golden_ust_final_period_simple_yield() -> R {
    let b = FixedRateBond::us_treasury(d(2025, 2, 15), d(2027, 2, 15), 0.04, None)?;
    assert_abs_diff_eq!(b.clean_price(0.039, d(2026, 11, 16))?, 100.014_836_330_909_361, epsilon = 1e-10);
    Ok(())
}

#[test]
fn golden_ust_end_of_month() -> R {
    let b = FixedRateBond::us_treasury(d(2026, 8, 31), d(2028, 8, 31), 0.04125, None)?;
    let ends: Vec<Date> = b.coupons().iter().map(|c| c.accrual_end).collect();
    assert_eq!(ends, [d(2027, 2, 28), d(2027, 8, 31), d(2028, 2, 29), d(2028, 8, 31)]);
    let settle = d(2026, 10, 15);
    assert_abs_diff_eq!(b.accrued(settle)?, 0.512_776_243_093_915, epsilon = 1e-12);
    assert_abs_diff_eq!(b.clean_price(0.042, settle)?, 99.862_071_986_471_193, epsilon = 1e-10);
    Ok(())
}

// ---------------------------------------------------------------------------
// OIS swaps
// ---------------------------------------------------------------------------

fn flat_market() -> (Date, OvernightIndex, DiscountCurve) {
    let today = d(2026, 9, 23);
    let curve = DiscountCurve::flat(today, 0.04, d(2066, 9, 23)).expect("flat curve");
    (today, OvernightIndex::sofr(), curve)
}

#[test]
fn golden_ois_par_rates() -> R {
    let (today, index, curve) = flat_market();
    for (years, par) in [(1, 0.040_256_163_371_180), (5, 0.040_252_227_824_074), (10, 0.040_252_432_335_201)] {
        let s = OisSwap::usd_sofr_spot(today, Tenor::years(years), SwapSide::PayFixed, 1.0, 0.0)?;
        assert_abs_diff_eq!(s.value(&index, &curve, &curve)?.par_rate, par, epsilon = 1e-13);
    }
    Ok(())
}

#[test]
fn golden_ois_5y_payer_legs() -> R {
    let (today, index, curve) = flat_market();
    let s = OisSwap::usd_sofr_spot(today, Tenor::years(5), SwapSide::PayFixed, 1e6, 0.04)?;
    let v = s.value(&index, &curve, &curve)?;
    assert_abs_diff_eq!(v.npv, 1_135.884_550_944_087, epsilon = 1e-6);
    assert_abs_diff_eq!(v.fixed_leg_pv, 180_136.280_383_079_546, epsilon = 1e-6);
    assert_abs_diff_eq!(v.float_leg_pv, 181_272.164_934_023_633, epsilon = 1e-6);

    let fixed: Vec<f64> = s.fixed_cashflows()?.iter().map(|c| c.amount).collect();
    let expected_fixed = [40_777.777_777_777_803, 40_444.444_444_444_503, 40_555.555_555_555_453, 40_555.555_555_555_453, 40_555.555_555_555_453];
    let float: Vec<f64> = s.float_cashflows(&index, &curve)?.iter().map(|c| c.amount).collect();
    let expected_float = [41_038.922_103_397_766, 40_696.718_987_396_576, 40_810.774_192_388_213, 40_810.774_192_388_213, 40_810.774_192_388_213];
    for (got, want) in fixed.iter().zip(expected_fixed) {
        assert_abs_diff_eq!(*got, want, epsilon = 1e-7);
    }
    for (got, want) in float.iter().zip(expected_float) {
        assert_abs_diff_eq!(*got, want, epsilon = 1e-7);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Swaptions
// ---------------------------------------------------------------------------

#[test]
fn golden_swaption_1y10y() -> R {
    let (_, index, curve) = flat_market();
    let expiry = d(2027, 9, 23);
    let payer = Swaption::usd_sofr(expiry, Tenor::years(10), SwaptionType::Payer, 1e6, 0.04)?;
    let receiver = Swaption::usd_sofr(expiry, Tenor::years(10), SwaptionType::Receiver, 1e6, 0.04)?;

    let p = payer.price(&index, &curve, &curve, VolQuote::Normal(0.008))?;
    assert_abs_diff_eq!(p.pv, 26_125.418_997_300_498, epsilon = 1e-6);
    assert_abs_diff_eq!(p.annuity, 7_870_420.164_031_203_836, epsilon = 1e-5);
    assert_abs_diff_eq!(p.vega, 3_138_278.215_498_735_663, epsilon = 1e-5);
    assert_abs_diff_eq!(p.delta, 4_034_345.597_871_636_972, epsilon = 1e-5);

    let r = receiver.price(&index, &curve, &curve, VolQuote::Normal(0.008))?;
    assert_abs_diff_eq!(r.pv, 24_137.121_491_618_138, epsilon = 1e-6);

    let sb = payer.price(&index, &curve, &curve, VolQuote::ShiftedLognormal { vol: 0.25, shift: 0.02 })?;
    assert_abs_diff_eq!(sb.pv, 48_074.944_051_775_950, epsilon = 1e-6);
    Ok(())
}

// ---------------------------------------------------------------------------
// Bootstrap
// ---------------------------------------------------------------------------

#[test]
fn golden_sofr_bootstrap_nodes() -> R {
    let today = d(2026, 9, 23);
    let quotes = [
        ("1M", 0.0430), ("3M", 0.0425), ("6M", 0.0415), ("1Y", 0.0400), ("2Y", 0.0380),
        ("3Y", 0.0375), ("5Y", 0.0378), ("7Y", 0.0385), ("10Y", 0.0395), ("30Y", 0.0410),
    ];
    let instruments: Vec<_> = quotes.iter().map(|(t, r)| BootstrapInstrument::ois(t.parse().expect("tenor"), *r)).collect();
    let res = bootstrap_sofr(today, &OvernightIndex::sofr(), &instruments, BootstrapConfig::default())?;

    let expected = [
        (d(2026, 10, 28), 0.995_835_859_403_227),
        (d(2026, 12, 30), 0.988_558_566_208_579),
        (d(2027, 3, 30), 0.978_787_333_939_823),
        (d(2027, 9, 29), 0.960_391_656_571_123),
        (d(2028, 9, 27), 0.926_752_884_721_165),
        (d(2029, 9, 27), 0.893_727_952_791_532),
        (d(2031, 9, 29), 0.827_988_405_446_439),
        (d(2033, 9, 28), 0.763_942_250_620_560),
        (d(2036, 9, 29), 0.673_230_136_979_227),
        (d(2056, 9, 27), 0.290_190_167_335_056),
    ];
    assert_eq!(res.nodes.len(), expected.len());
    for ((_, date, df), (want_date, want_df)) in res.nodes.iter().zip(expected) {
        assert_eq!(*date, want_date);
        assert_abs_diff_eq!(*df, want_df, epsilon = 1e-10);
    }
    Ok(())
}
