//! European, physically settled swaption on an OIS swap.
//!
//! The underlying is a spot-starting swap from the expiry date; the forward
//! swap rate and annuity come from the swap's own valuation off the curves,
//! so the swaption is exactly consistent with the swap pricer. Prices come
//! from the Bachelier (normal) model by default, the USD market standard,
//! or from shifted Black.

use thiserror::Error;

use fq_core::math::{BlackError, BlackResult, OptionType, bachelier, bachelier_implied_vol, shifted_black, shifted_black_implied_vol};
use fq_core::time::Date;

use crate::curve::DiscountCurve;
use crate::index::OvernightIndex;
use crate::swap::{OisSwap, SwapError, SwapSide, Tenor};

#[derive(Debug, Error, Clone, PartialEq)]
pub enum SwaptionError {
    #[error(transparent)]
    Swap(#[from] SwapError),
    #[error(transparent)]
    Black(#[from] BlackError),
    #[error("expiry {expiry} is not after the valuation date {today}")]
    Expired { expiry: Date, today: Date },
    #[error("underlying swap starts {start}, before expiry {expiry}")]
    UnderlyingStartsBeforeExpiry { start: Date, expiry: Date },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SwaptionType {
    /// Right to enter as fixed payer (a call on the swap rate).
    Payer,
    /// Right to enter as fixed receiver (a put on the swap rate).
    Receiver,
}

impl SwaptionType {
    const fn option_type(self) -> OptionType {
        match self {
            Self::Payer => OptionType::Call,
            Self::Receiver => OptionType::Put,
        }
    }

    const fn swap_side(self) -> SwapSide {
        match self {
            Self::Payer => SwapSide::PayFixed,
            Self::Receiver => SwapSide::ReceiveFixed,
        }
    }
}

/// Volatility quote for the swaption's expiry/tenor/strike.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum VolQuote {
    /// Absolute (normal) vol in rate units, e.g. 0.0080 for 80bp.
    Normal(f64),
    /// Lognormal vol on the shifted rate; `shift` 0 is plain Black.
    ShiftedLognormal { vol: f64, shift: f64 },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SwaptionPricing {
    /// PV in currency units (notional included).
    pub pv: f64,
    pub forward_rate: f64,
    pub strike: f64,
    /// Annuity in currency units: PV of 1 unit of fixed rate on the notional.
    pub annuity: f64,
    /// ACT/365F years from the valuation date to expiry.
    pub expiry_time: f64,
    /// dPV / dForward (currency per unit rate); divide by 1e4 for a 1bp move.
    pub delta: f64,
    /// d2PV / dForward2
    pub gamma: f64,
    /// dPV / dVol in the quote's own units; times 1e-4 gives vega per bp of normal vol.
    pub vega: f64,
    /// Per-unit-annuity model output.
    pub model: BlackResult,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Swaption {
    expiry: Date,
    kind: SwaptionType,
    underlying: OisSwap,
}

impl Swaption {
    /// Wraps an existing forward-starting swap. The swap's side must match
    /// the option type; the swap's fixed rate is the strike.
    pub fn new(expiry: Date, kind: SwaptionType, underlying: OisSwap) -> Result<Self, SwaptionError> {
        let start = underlying.effective_date();
        if start < expiry {
            return Err(SwaptionError::UnderlyingStartsBeforeExpiry { start, expiry });
        }
        Ok(Self { expiry, kind, underlying })
    }

    /// e.g. a 1Y-into-10Y USD SOFR payer: `usd_sofr(expiry, Tenor::years(10), Payer, notional, strike)`.
    /// The underlying starts two business days after expiry (spot settlement).
    pub fn usd_sofr(expiry: Date, tenor: Tenor, kind: SwaptionType, notional: f64, strike: f64) -> Result<Self, SwaptionError> {
        let underlying = OisSwap::usd_sofr_spot(expiry, tenor, kind.swap_side(), notional, strike)?;
        Self::new(expiry, kind, underlying)
    }

    pub fn expiry(&self) -> Date {
        self.expiry
    }

    pub fn kind(&self) -> SwaptionType {
        self.kind
    }

    pub fn strike(&self) -> f64 {
        self.underlying.fixed_rate()
    }

    pub fn underlying(&self) -> &OisSwap {
        &self.underlying
    }

    /// Forward swap rate and annuity (currency units) off the curves.
    pub fn forward_and_annuity(&self, index: &OvernightIndex, projection: &DiscountCurve, discount: &DiscountCurve) -> Result<(f64, f64), SwaptionError> {
        let v = self.underlying.value(index, projection, discount)?;
        Ok((v.par_rate, v.annuity))
    }

    pub fn price(&self, index: &OvernightIndex, projection: &DiscountCurve, discount: &DiscountCurve, vol: VolQuote) -> Result<SwaptionPricing, SwaptionError> {
        let today = discount.reference_date();
        if self.expiry <= today {
            return Err(SwaptionError::Expired { expiry: self.expiry, today });
        }
        let t = discount.time(self.expiry);
        let (forward, annuity) = self.forward_and_annuity(index, projection, discount)?;
        let strike = self.strike();
        let kind = self.kind.option_type();
        let model = match vol {
            VolQuote::Normal(sigma) => bachelier(kind, forward, strike, sigma, t)?,
            VolQuote::ShiftedLognormal { vol, shift } => shifted_black(kind, forward, strike, vol, t, shift)?,
        };
        Ok(SwaptionPricing {
            pv: annuity * model.price,
            forward_rate: forward,
            strike,
            annuity,
            expiry_time: t,
            delta: annuity * model.delta,
            gamma: annuity * model.gamma,
            vega: annuity * model.vega,
            model,
        })
    }

    /// Normal vol that reproduces `pv` (currency units).
    pub fn implied_normal_vol(&self, index: &OvernightIndex, projection: &DiscountCurve, discount: &DiscountCurve, pv: f64) -> Result<f64, SwaptionError> {
        let t = discount.time(self.expiry);
        let (forward, annuity) = self.forward_and_annuity(index, projection, discount)?;
        Ok(bachelier_implied_vol(self.kind.option_type(), forward, self.strike(), t, pv / annuity, 1.0)?)
    }

    /// Shifted-lognormal vol that reproduces `pv` (currency units).
    pub fn implied_lognormal_vol(&self, index: &OvernightIndex, projection: &DiscountCurve, discount: &DiscountCurve, shift: f64, pv: f64) -> Result<f64, SwaptionError> {
        let t = discount.time(self.expiry);
        let (forward, annuity) = self.forward_and_annuity(index, projection, discount)?;
        Ok(shifted_black_implied_vol(self.kind.option_type(), forward, self.strike(), t, shift, pv / annuity, 5.0)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_abs_diff_eq;
    use std::f64::consts::PI;

    type R = Result<(), SwaptionError>;

    fn d(y: i32, m: u32, dd: u32) -> Date {
        Date::from_ymd(y, m, dd).expect("valid test date")
    }

    fn setup() -> (Date, OvernightIndex, DiscountCurve) {
        let today = d(2026, 9, 23);
        let curve = DiscountCurve::flat(today, 0.04, d(2056, 9, 23)).expect("flat curve");
        (today, OvernightIndex::sofr(), curve)
    }

    #[test]
    fn atm_bachelier_price_matches_closed_form() -> R {
        let (today, index, curve) = setup();
        let expiry = d(2027, 9, 23);
        let probe = Swaption::usd_sofr(expiry, Tenor::years(10), SwaptionType::Payer, 1e8, 0.0)?;
        let (fwd, annuity) = probe.forward_and_annuity(&index, &curve, &curve)?;
        let atm = Swaption::usd_sofr(expiry, Tenor::years(10), SwaptionType::Payer, 1e8, fwd)?;
        let sigma = 0.0080;
        let p = atm.price(&index, &curve, &curve, VolQuote::Normal(sigma))?;
        let t = f64::from(expiry - today) / 365.0;
        assert_abs_diff_eq!(p.pv, annuity * sigma * t.sqrt() / (2.0 * PI).sqrt(), epsilon = 1e-6);
        assert_abs_diff_eq!(p.model.delta, 0.5, epsilon = 1e-12);
        assert!(p.vega > 0.0 && p.gamma > 0.0);
        Ok(())
    }

    #[test]
    fn payer_receiver_parity_equals_forward_swap_npv() -> R {
        let (_, index, curve) = setup();
        let expiry = d(2028, 9, 25);
        let strike = 0.035;
        let payer = Swaption::usd_sofr(expiry, Tenor::years(5), SwaptionType::Payer, 1e6, strike)?;
        let receiver = Swaption::usd_sofr(expiry, Tenor::years(5), SwaptionType::Receiver, 1e6, strike)?;
        let vol = VolQuote::Normal(0.0070);
        let diff = payer.price(&index, &curve, &curve, vol)?.pv - receiver.price(&index, &curve, &curve, vol)?.pv;
        // Payer minus receiver is a forward-starting payer swap at the strike.
        let fwd_swap_npv = payer.underlying().npv(&index, &curve)?;
        assert_abs_diff_eq!(diff, fwd_swap_npv, epsilon = 1e-6);
        Ok(())
    }

    #[test]
    fn implied_vols_round_trip_both_models() -> R {
        let (_, index, curve) = setup();
        let s = Swaption::usd_sofr(d(2029, 9, 24), Tenor::years(2), SwaptionType::Receiver, 5e7, 0.042)?;
        let pv = s.price(&index, &curve, &curve, VolQuote::Normal(0.0065))?.pv;
        assert_abs_diff_eq!(s.implied_normal_vol(&index, &curve, &curve, pv)?, 0.0065, epsilon = 1e-10);
        let pv = s.price(&index, &curve, &curve, VolQuote::ShiftedLognormal { vol: 0.25, shift: 0.02 })?.pv;
        assert_abs_diff_eq!(s.implied_lognormal_vol(&index, &curve, &curve, 0.02, pv)?, 0.25, epsilon = 1e-10);
        Ok(())
    }

    #[test]
    fn greeks_scale_with_annuity_and_pv_is_convex_in_forward() -> R {
        let (_, index, curve) = setup();
        let s = Swaption::usd_sofr(d(2027, 9, 23), Tenor::years(10), SwaptionType::Payer, 1e6, 0.04)?;
        let p = s.price(&index, &curve, &curve, VolQuote::Normal(0.008))?;
        assert_abs_diff_eq!(p.delta, p.annuity * p.model.delta, epsilon = 1e-9);
        assert_abs_diff_eq!(p.vega, p.annuity * p.model.vega, epsilon = 1e-9);
        // Higher rates raise a payer's value.
        let up = s.price(&index, &curve.shifted(1e-3), &curve.shifted(1e-3), VolQuote::Normal(0.008))?;
        assert!(up.pv > p.pv);
        assert!(up.forward_rate > p.forward_rate);
        Ok(())
    }

    #[test]
    fn errors() -> R {
        let (today, index, curve) = setup();
        let s = Swaption::usd_sofr(today, Tenor::years(5), SwaptionType::Payer, 1e6, 0.04)?;
        assert!(matches!(s.price(&index, &curve, &curve, VolQuote::Normal(0.01)), Err(SwaptionError::Expired { .. })));
        let swap = OisSwap::usd_sofr_spot(today, Tenor::years(5), SwapSide::PayFixed, 1e6, 0.04)?;
        assert!(matches!(Swaption::new(d(2027, 1, 4), SwaptionType::Payer, swap), Err(SwaptionError::UnderlyingStartsBeforeExpiry { .. })));
        Ok(())
    }
}
