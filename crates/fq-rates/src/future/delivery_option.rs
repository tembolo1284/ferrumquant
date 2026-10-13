//! The short's delivery (switch) option in a Treasury future.
//!
//! At delivery the short chooses the bond that minimises `P_i / CF_i`, so
//! the future settles to `min_i P_i(y) / CF_i`, not to any one bond. Under
//! a parallel shift `Δ ~ N(0, σ² T)` of the basket's forward yields the fair
//! futures price is `E[min_i P_i(y_i + Δ) / CF_i]`, and the switch option
//! is `min_i E[P_i(y_i + Δ) / CF_i] − E[min_i ...] ≥ 0` (Jensen). Splitting
//! it this way keeps the single-bond convexity effect out of the option
//! value: with one deliverable the option is exactly zero.
//!
//! This is the standard "parallel-shift / normal yield" model; the timing
//! and end-of-month options are not included.

use thiserror::Error;

use fq_core::math::norm_pdf;
use fq_core::time::Date;

use crate::bond::BondError;
use crate::future::{FutureError, TreasuryFuture};

#[derive(Debug, Error, Clone, PartialEq)]
pub enum DeliveryOptionError {
    #[error(transparent)]
    Future(#[from] FutureError),
    #[error(transparent)]
    Bond(#[from] BondError),
    #[error("delivery {delivery} must be after settlement {settle}")]
    InvalidWindow { settle: Date, delivery: Date },
    #[error("yield volatility {0} must be non-negative")]
    NegativeVol(f64),
}

/// Inputs for the parallel-shift model.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DeliveryOptionConfig {
    /// Annualised normal yield volatility in rate units (e.g. 0.009 for 90bp).
    pub yield_vol: f64,
    /// Integration half-width in standard deviations.
    pub std_devs: f64,
    /// Number of integration points (odd keeps a node at zero shift).
    pub points: usize,
}

impl Default for DeliveryOptionConfig {
    fn default() -> Self {
        Self { yield_vol: 0.009, std_devs: 6.0, points: 241 }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct DeliveryOptionResult {
    /// `min_i forward_clean_i / CF_i`: the CTD-only price at zero shift.
    pub ctd_only_price: f64,
    /// `E[P_i(Δ) / CF_i]` per deliverable.
    pub expected_converted: Vec<f64>,
    /// `E[min_i P_i(Δ) / CF_i]`: fair futures price including the switch option.
    pub fair_price: f64,
    /// `min_i expected_converted_i − fair_price`, in price points (≥ 0).
    pub switch_option: f64,
    /// Probability each deliverable is cheapest at delivery.
    pub ctd_probability: Vec<f64>,
    /// Forward yield of each deliverable at delivery (zero shift).
    pub forward_yields: Vec<f64>,
}

impl DeliveryOptionResult {
    pub fn most_likely_ctd(&self) -> usize {
        self.ctd_probability
            .iter()
            .enumerate()
            .fold((0, f64::NEG_INFINITY), |(bi, bv), (i, &v)| if v > bv { (i, v) } else { (bi, bv) })
            .0
    }
}

/// Value the switch option for `future` given today's clean prices.
pub fn delivery_option(
    future: &TreasuryFuture,
    clean_prices: &[f64],
    settle: Date,
    delivery_date: Date,
    repo: f64,
    config: DeliveryOptionConfig,
) -> Result<DeliveryOptionResult, DeliveryOptionError> {
    if delivery_date <= settle {
        return Err(DeliveryOptionError::InvalidWindow { settle, delivery: delivery_date });
    }
    if config.yield_vol < 0.0 {
        return Err(DeliveryOptionError::NegativeVol(config.yield_vol));
    }
    let basket = future.basket();
    if clean_prices.len() != basket.len() {
        return Err(FutureError::LengthMismatch { prices: clean_prices.len(), basket: basket.len() }.into());
    }
    let n = basket.len();

    // Forward clean prices and the yields that reproduce them at delivery.
    let mut forward_yields = Vec::with_capacity(n);
    let mut converted_at_zero = Vec::with_capacity(n);
    for (i, &clean) in clean_prices.iter().enumerate() {
        let fwd_clean = future.forward_clean_price(i, clean, settle, delivery_date, repo)?;
        forward_yields.push(basket[i].bond.yield_from_clean(fwd_clean, delivery_date)?);
        converted_at_zero.push(fwd_clean / basket[i].conversion_factor);
    }
    let ctd_only_price = converted_at_zero.iter().copied().fold(f64::INFINITY, f64::min);

    // Integrate over the yield shift with a normal density on a uniform grid.
    let t = f64::from(delivery_date - settle) / 365.0;
    let sd = config.yield_vol * t.sqrt();
    let points = config.points.max(1);
    let (shifts, weights): (Vec<f64>, Vec<f64>) = if sd == 0.0 || points == 1 {
        (vec![0.0], vec![1.0])
    } else {
        let step = 2.0 * config.std_devs / (points - 1) as f64;
        let xs: Vec<f64> = (0..points).map(|k| -config.std_devs + step * k as f64).collect();
        let raw: Vec<f64> = xs.iter().map(|&x| norm_pdf(x) * step).collect();
        let total: f64 = raw.iter().sum();
        (xs.iter().map(|&x| x * sd).collect(), raw.iter().map(|w| w / total).collect())
    };

    let mut expected_converted = vec![0.0; n];
    let mut fair_price = 0.0;
    let mut ctd_probability = vec![0.0; n];
    let mut converted = vec![0.0; n];
    for (&shift, &w) in shifts.iter().zip(&weights) {
        for ((b, &fy), (c, e)) in basket.iter().zip(&forward_yields).zip(converted.iter_mut().zip(expected_converted.iter_mut())) {
            let price = b.bond.clean_price(fy + shift, delivery_date)?;
            *c = price / b.conversion_factor;
            *e += w * *c;
        }
        let (ctd, min) = converted
            .iter()
            .enumerate()
            .fold((0, f64::INFINITY), |(bi, bv), (i, &v)| if v < bv { (i, v) } else { (bi, bv) });
        fair_price += w * min;
        ctd_probability[ctd] += w;
    }

    let min_expected = expected_converted.iter().copied().fold(f64::INFINITY, f64::min);
    Ok(DeliveryOptionResult {
        ctd_only_price,
        expected_converted,
        fair_price,
        switch_option: (min_expected - fair_price).max(0.0),
        ctd_probability,
        forward_yields,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_abs_diff_eq;

    use crate::bond::FixedRateBond;
    use crate::future::{DeliveryMonth, TreasuryContract};

    type R = Result<(), Box<dyn std::error::Error>>;

    fn d(y: i32, m: u32, dd: u32) -> Date {
        Date::from_ymd(y, m, dd).expect("valid test date")
    }

    fn ust(dated: Date, maturity: Date, cpn: f64) -> FixedRateBond {
        FixedRateBond::us_treasury(dated, maturity, cpn, None).expect("valid test bond")
    }

    /// ZN March 2027: a low-coupon short bond and a high-coupon long bond, so
    /// the CTD switches as yields move.
    fn zn_h27() -> Result<TreasuryFuture, FutureError> {
        TreasuryFuture::new(
            TreasuryContract::TenYear,
            DeliveryMonth::new(2027, 3)?,
            [ust(d(2024, 2, 15), d(2034, 2, 15), 0.035), ust(d(2026, 8, 15), d(2036, 8, 15), 0.0425)],
        )
    }

    const SETTLE: (i32, u32, u32) = (2027, 1, 4);
    const DELIVERY: (i32, u32, u32) = (2027, 3, 31);

    #[test]
    fn zero_vol_reduces_to_ctd_only() -> R {
        let f = zn_h27()?;
        let (s, dd) = (d(SETTLE.0, SETTLE.1, SETTLE.2), d(DELIVERY.0, DELIVERY.1, DELIVERY.2));
        let prices = [96.75, 99.25];
        let cfg = DeliveryOptionConfig { yield_vol: 0.0, ..Default::default() };
        let r = delivery_option(&f, &prices, s, dd, 0.0385, cfg)?;
        let (ctd_idx, fair) = f.fair_price(&prices, s, dd, 0.0385)?;
        assert_abs_diff_eq!(r.fair_price, fair, epsilon = 1e-9);
        assert_abs_diff_eq!(r.ctd_only_price, fair, epsilon = 1e-9);
        assert_abs_diff_eq!(r.switch_option, 0.0, epsilon = 1e-12);
        assert_eq!(r.most_likely_ctd(), ctd_idx);
        assert_abs_diff_eq!(r.ctd_probability[ctd_idx], 1.0, epsilon = 1e-12);
        Ok(())
    }

    #[test]
    fn positive_vol_gives_a_positive_option_and_lower_price() -> R {
        let f = zn_h27()?;
        let (s, dd) = (d(SETTLE.0, SETTLE.1, SETTLE.2), d(DELIVERY.0, DELIVERY.1, DELIVERY.2));
        let prices = [96.75, 99.25];
        let r = delivery_option(&f, &prices, s, dd, 0.0385, DeliveryOptionConfig::default())?;
        assert!(r.switch_option > 0.0, "option {}", r.switch_option);
        assert!(r.switch_option < 0.5, "option {} too large for a 3-month 90bp vol", r.switch_option);
        let min_expected = r.expected_converted.iter().copied().fold(f64::INFINITY, f64::min);
        assert_abs_diff_eq!(r.fair_price, min_expected - r.switch_option, epsilon = 1e-12);
        assert_abs_diff_eq!(r.ctd_probability.iter().sum::<f64>(), 1.0, epsilon = 1e-12);
        assert!(r.ctd_probability.iter().all(|&p| p > 0.0), "both bonds should be CTD somewhere: {:?}", r.ctd_probability);
        // More vol, more option.
        let hi = delivery_option(&f, &prices, s, dd, 0.0385, DeliveryOptionConfig { yield_vol: 0.015, ..Default::default() })?;
        assert!(hi.switch_option > r.switch_option);
        Ok(())
    }

    #[test]
    fn single_bond_basket_has_no_switch_option() -> R {
        let f = TreasuryFuture::new(TreasuryContract::TenYear, DeliveryMonth::new(2027, 3)?, [ust(d(2026, 8, 15), d(2036, 8, 15), 0.0425)])?;
        let (s, dd) = (d(SETTLE.0, SETTLE.1, SETTLE.2), d(DELIVERY.0, DELIVERY.1, DELIVERY.2));
        let r = delivery_option(&f, &[99.25], s, dd, 0.0385, DeliveryOptionConfig::default())?;
        assert_abs_diff_eq!(r.switch_option, 0.0, epsilon = 1e-12);
        assert_abs_diff_eq!(r.ctd_probability[0], 1.0, epsilon = 1e-12);
        // Convexity: the expected converted price exceeds the zero-shift one.
        assert!(r.expected_converted[0] > r.ctd_only_price);
        Ok(())
    }

    #[test]
    fn errors() -> R {
        let f = zn_h27()?;
        let (s, dd) = (d(SETTLE.0, SETTLE.1, SETTLE.2), d(DELIVERY.0, DELIVERY.1, DELIVERY.2));
        assert!(matches!(delivery_option(&f, &[99.0], s, dd, 0.04, DeliveryOptionConfig::default()), Err(DeliveryOptionError::Future(FutureError::LengthMismatch { .. }))));
        assert!(matches!(delivery_option(&f, &[96.75, 99.25], dd, s, 0.04, DeliveryOptionConfig::default()), Err(DeliveryOptionError::InvalidWindow { .. })));
        let bad = DeliveryOptionConfig { yield_vol: -0.01, ..Default::default() };
        assert!(matches!(delivery_option(&f, &[96.75, 99.25], s, dd, 0.04, bad), Err(DeliveryOptionError::NegativeVol(_))));
        Ok(())
    }
}
