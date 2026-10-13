//! Bond spreads to the swap (SOFR) curve.
//!
//! All three measure how a bond's price compares with the same cashflows
//! valued off the OIS curve, from settlement forward:
//!
//! - **Z-spread**: the constant continuously-compounded spread `z` added to
//!   the zero curve such that `Σ CF_k DF(settle, p_k) e^{-z t_k} = P_dirty`.
//! - **I-spread**: Street yield minus the par OIS rate from settlement to
//!   maturity. A quick quote-screen number; note the yield is semiannual
//!   bond-basis and the swap rate annual ACT/360, so this is a convention
//!   mismatch by design.
//! - **Asset swap spread** (par-par): the spread over SOFR on a par floater
//!   that makes buying the bond at par and swapping its coupons fair:
//!   `s = (PV_curve − P_dirty) / (100 × A)`, with `A` the floating-leg
//!   annuity per unit notional on the OIS schedule from settlement.
//!
//! Discount factors are taken relative to the settlement date,
//! `DF(settle, d) = DF(d) / DF(settle)`, so results do not depend on the
//! gap between the curve reference date and settlement.

use thiserror::Error;

use fq_core::math::{SolverConfig, SolverError, bracket, brent};
use fq_core::time::Date;

use crate::bond::{BondError, FixedRateBond};
use crate::curve::{CurveError, DiscountCurve};
use crate::index::OvernightIndex;
use crate::swap::{OisSwap, SwapError, SwapSide};

#[derive(Debug, Error, Clone, PartialEq)]
pub enum SpreadError {
    #[error(transparent)]
    Bond(#[from] BondError),
    #[error(transparent)]
    Curve(#[from] CurveError),
    #[error(transparent)]
    Swap(#[from] SwapError),
    #[error(transparent)]
    Solver(#[from] SolverError),
    #[error("settlement {settle} is before the curve reference date {reference}")]
    SettleBeforeCurve { settle: Date, reference: Date },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BondSpreads {
    pub clean_price: f64,
    pub dirty_price: f64,
    /// Street yield (semiannual bond basis).
    pub yield_rate: f64,
    /// Dirty PV per `face` of the bond's cashflows on the curve, from settlement.
    pub curve_pv: f64,
    pub z_spread: f64,
    pub i_spread: f64,
    pub asset_swap_spread: f64,
}

fn check_settle(curve: &DiscountCurve, settle: Date) -> Result<(), SpreadError> {
    if settle < curve.reference_date() {
        return Err(SpreadError::SettleBeforeCurve { settle, reference: curve.reference_date() });
    }
    Ok(())
}

/// Dirty PV per `face` at `settle` with a continuously-compounded spread `z`
/// over the curve: `Σ_{p > settle} CF DF(settle, p) e^{-z t}`, `t` in
/// ACT/365F years from settlement.
pub fn npv_with_spread(bond: &FixedRateBond, curve: &DiscountCurve, settle: Date, z: f64) -> Result<f64, SpreadError> {
    check_settle(curve, settle)?;
    let df_settle = curve.discount(settle)?;
    let mut pv = 0.0;
    for c in bond.coupons().iter().filter(|c| c.payment_date > settle) {
        let t = f64::from(c.payment_date - settle) / 365.0;
        pv += c.amount * curve.discount(c.payment_date)? / df_settle * (-z * t).exp();
    }
    let r = bond.redemption_date();
    if r > settle {
        let t = f64::from(r - settle) / 365.0;
        pv += bond.face() * curve.discount(r)? / df_settle * (-z * t).exp();
    }
    Ok(pv)
}

/// Z-spread (continuously compounded, rate units) that reprices `clean_price`.
pub fn z_spread(bond: &FixedRateBond, clean_price: f64, settle: Date, curve: &DiscountCurve) -> Result<f64, SpreadError> {
    check_settle(curve, settle)?;
    let target = clean_price + bond.accrued(settle)?;
    let mut err = None;
    let mut f = |z: f64| match npv_with_spread(bond, curve, settle, z) {
        Ok(pv) => pv - target,
        Err(e) => {
            err = Some(e);
            f64::NAN
        }
    };
    let (lo, hi) = bracket(&mut f, -0.02, 0.02, 20)?;
    let z = brent(&mut f, lo, hi, SolverConfig { tolerance: 1e-14, max_iterations: 200 })?;
    match err {
        Some(e) => Err(e),
        None => Ok(z),
    }
}

/// Par OIS rate from `settle` to the bond's maturity on the curve.
pub fn matched_maturity_swap_rate(bond: &FixedRateBond, settle: Date, index: &OvernightIndex, curve: &DiscountCurve) -> Result<f64, SpreadError> {
    check_settle(curve, settle)?;
    let swap = OisSwap::usd_sofr(settle, bond.maturity(), SwapSide::PayFixed, 1.0, 0.0)?;
    Ok(swap.value(index, curve, curve)?.par_rate)
}

/// I-spread: Street yield minus the matched-maturity par OIS rate.
pub fn i_spread(bond: &FixedRateBond, clean_price: f64, settle: Date, index: &OvernightIndex, curve: &DiscountCurve) -> Result<f64, SpreadError> {
    let y = bond.yield_from_clean(clean_price, settle)?;
    Ok(y - matched_maturity_swap_rate(bond, settle, index, curve)?)
}

/// Par-par asset swap spread (rate units) over SOFR, annual ACT/360 on the
/// OIS schedule from `settle` to maturity.
pub fn asset_swap_spread(bond: &FixedRateBond, clean_price: f64, settle: Date, curve: &DiscountCurve) -> Result<f64, SpreadError> {
    check_settle(curve, settle)?;
    let dirty = clean_price + bond.accrued(settle)?;
    let curve_pv = npv_with_spread(bond, curve, settle, 0.0)?;
    let df_settle = curve.discount(settle)?;
    let swap = OisSwap::usd_sofr(settle, bond.maturity(), SwapSide::PayFixed, 1.0, 0.0)?;
    let annuity: f64 = swap
        .periods()
        .iter()
        .map(|p| curve.discount(p.payment_date).map(|df| p.fixed_year_fraction * df / df_settle))
        .sum::<Result<f64, CurveError>>()?;
    Ok((curve_pv - dirty) / (bond.face() * annuity))
}

/// All spread measures in one pass.
pub fn spreads(bond: &FixedRateBond, clean_price: f64, settle: Date, index: &OvernightIndex, curve: &DiscountCurve) -> Result<BondSpreads, SpreadError> {
    let dirty_price = clean_price + bond.accrued(settle)?;
    Ok(BondSpreads {
        clean_price,
        dirty_price,
        yield_rate: bond.yield_from_clean(clean_price, settle)?,
        curve_pv: npv_with_spread(bond, curve, settle, 0.0)?,
        z_spread: z_spread(bond, clean_price, settle, curve)?,
        i_spread: i_spread(bond, clean_price, settle, index, curve)?,
        asset_swap_spread: asset_swap_spread(bond, clean_price, settle, curve)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_abs_diff_eq;

    type R = Result<(), SpreadError>;

    fn d(y: i32, m: u32, dd: u32) -> Date {
        Date::from_ymd(y, m, dd).expect("valid test date")
    }

    fn setup() -> (FixedRateBond, Date, OvernightIndex, DiscountCurve) {
        let bond = FixedRateBond::us_treasury(d(2026, 8, 15), d(2036, 8, 15), 0.0425, None).expect("bond");
        let today = d(2026, 10, 13);
        let curve = DiscountCurve::flat(today, 0.04, d(2040, 1, 1)).expect("curve");
        (bond, today, OvernightIndex::sofr(), curve)
    }

    #[test]
    fn zero_spread_when_priced_off_the_curve() -> R {
        let (bond, today, index, curve) = setup();
        let settle = bond.settlement_date(today)?;
        let clean = npv_with_spread(&bond, &curve, settle, 0.0)? - bond.accrued(settle)?;
        assert_abs_diff_eq!(z_spread(&bond, clean, settle, &curve)?, 0.0, epsilon = 1e-12);
        assert_abs_diff_eq!(asset_swap_spread(&bond, clean, settle, &curve)?, 0.0, epsilon = 1e-12);
        let s = spreads(&bond, clean, settle, &index, &curve)?;
        assert_abs_diff_eq!(s.curve_pv, s.dirty_price, epsilon = 1e-10);
        Ok(())
    }

    #[test]
    fn z_spread_round_trips_and_is_independent_of_curve_reference() -> R {
        let (bond, today, _, curve) = setup();
        let settle = bond.settlement_date(today)?;
        let z_true = 0.0035;
        let clean = npv_with_spread(&bond, &curve, settle, z_true)? - bond.accrued(settle)?;
        assert_abs_diff_eq!(z_spread(&bond, clean, settle, &curve)?, z_true, epsilon = 1e-12);
        // Same curve shape referenced a week earlier: identical spread.
        let earlier = DiscountCurve::flat(d(2026, 10, 6), 0.04, d(2040, 1, 1))?;
        assert_abs_diff_eq!(z_spread(&bond, clean, settle, &earlier)?, z_true, epsilon = 1e-12);
        // Negative spreads solve too (rich bond).
        let rich = npv_with_spread(&bond, &curve, settle, -0.002)? - bond.accrued(settle)?;
        assert_abs_diff_eq!(z_spread(&bond, rich, settle, &curve)?, -0.002, epsilon = 1e-12);
        Ok(())
    }

    #[test]
    fn cheaper_bond_has_higher_spreads() -> R {
        let (bond, today, index, curve) = setup();
        let settle = bond.settlement_date(today)?;
        let par_clean = npv_with_spread(&bond, &curve, settle, 0.0)? - bond.accrued(settle)?;
        let a = spreads(&bond, par_clean, settle, &index, &curve)?;
        let b = spreads(&bond, par_clean - 2.0, settle, &index, &curve)?;
        assert!(b.z_spread > a.z_spread && b.z_spread > 0.002);
        assert!(b.asset_swap_spread > a.asset_swap_spread && b.asset_swap_spread > 0.002);
        assert!(b.i_spread > a.i_spread);
        assert!(b.yield_rate > a.yield_rate);
        // Z and ASW agree to first order for a bond near par on a flat curve.
        assert_abs_diff_eq!(b.z_spread, b.asset_swap_spread, epsilon = 2e-4);
        Ok(())
    }

    #[test]
    fn i_spread_is_yield_minus_matched_swap_rate() -> R {
        let (bond, today, index, curve) = setup();
        let settle = bond.settlement_date(today)?;
        let clean = 99.0;
        let y = bond.yield_from_clean(clean, settle)?;
        let par = matched_maturity_swap_rate(&bond, settle, &index, &curve)?;
        assert!(par > 0.040 && par < 0.041, "{par}");
        assert_abs_diff_eq!(i_spread(&bond, clean, settle, &index, &curve)?, y - par, epsilon = 1e-15);
        Ok(())
    }

    #[test]
    fn settlement_before_curve_is_rejected() -> R {
        let (bond, _, _, curve) = setup();
        let r = z_spread(&bond, 99.0, d(2026, 10, 1), &curve);
        assert!(matches!(r, Err(SpreadError::SettleBeforeCurve { .. })));
        Ok(())
    }
}
