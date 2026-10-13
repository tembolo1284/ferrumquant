//! CME three-month SOFR futures (SR3).
//!
//! Each contract settles to `100 - R` where `R` is SOFR compounded daily
//! (ACT/360) over the reference quarter from the contract month's IMM date
//! to the next IMM date. The futures rate `100 - P` exceeds the forward rate
//! by a convexity adjustment that depends on a rate-volatility model; it is
//! taken as an input here (in rate units) so the bootstrap can use whichever
//! model the desk prefers.

use thiserror::Error;

use fq_core::time::{BusinessDayConvention, Calendar, Date, DateError, ImmError, ImmMonth};

use crate::curve::DiscountCurve;
use crate::index::{CompoundingConvention, IndexError, OvernightIndex};

#[derive(Debug, Error, Clone, PartialEq)]
pub enum SofrFutureError {
    #[error(transparent)]
    Date(#[from] DateError),
    #[error(transparent)]
    Imm(#[from] ImmError),
    #[error(transparent)]
    Index(#[from] IndexError),
    #[error("futures price {0} is not in (0, 200)")]
    InvalidPrice(f64),
}

#[derive(Debug, Clone, PartialEq)]
pub struct SofrFuture {
    contract: ImmMonth,
    /// Reference period [start, end): IMM date to next IMM date, Following-adjusted.
    start: Date,
    end: Date,
    price: f64,
    convexity_adjustment: f64,
}

impl SofrFuture {
    /// `convexity_adjustment` in rate units (e.g. 0.0003 for 3bp):
    /// `forward = (100 - price) / 100 - convexity_adjustment`.
    pub fn new(contract: ImmMonth, price: f64, convexity_adjustment: f64, calendar: &Calendar) -> Result<Self, SofrFutureError> {
        if !(0.0..200.0).contains(&price) {
            return Err(SofrFutureError::InvalidPrice(price));
        }
        let start = calendar.adjust(contract.imm_date()?, BusinessDayConvention::Following)?;
        let end = calendar.adjust(contract.next().imm_date()?, BusinessDayConvention::Following)?;
        Ok(Self { contract, start, end, price, convexity_adjustment })
    }

    pub fn contract(&self) -> ImmMonth {
        self.contract
    }

    pub fn start(&self) -> Date {
        self.start
    }

    pub fn end(&self) -> Date {
        self.end
    }

    pub fn price(&self) -> f64 {
        self.price
    }

    pub fn convexity_adjustment(&self) -> f64 {
        self.convexity_adjustment
    }

    /// `100 - P`, the rate the contract trades at.
    pub fn futures_rate(&self) -> f64 {
        (100.0 - self.price) / 100.0
    }

    /// Futures rate less the convexity adjustment: the forward the curve must reproduce.
    pub fn implied_forward(&self) -> f64 {
        self.futures_rate() - self.convexity_adjustment
    }

    /// Compounded SOFR over the reference period from fixings and/or the curve.
    pub fn curve_rate(&self, index: &OvernightIndex, curve: &DiscountCurve) -> Result<f64, SofrFutureError> {
        Ok(index.compounded_rate(self.start, self.end, Some(curve), CompoundingConvention::default())?)
    }

    /// Curve rate minus implied forward, in rate units. Zero when the curve
    /// reprices the contract; this is the bootstrap objective.
    pub fn npv(&self, index: &OvernightIndex, curve: &DiscountCurve) -> Result<f64, SofrFutureError> {
        Ok(self.curve_rate(index, curve)? - self.implied_forward())
    }

    /// Price the curve implies, with the convexity adjustment added back.
    pub fn fair_price(&self, index: &OvernightIndex, curve: &DiscountCurve) -> Result<f64, SofrFutureError> {
        Ok(100.0 * (1.0 - (self.curve_rate(index, curve)? + self.convexity_adjustment)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_abs_diff_eq;
    use fq_core::time::Market;

    type R = Result<(), Box<dyn std::error::Error>>;

    fn d(y: i32, m: u32, dd: u32) -> Date {
        Date::from_ymd(y, m, dd).expect("valid test date")
    }

    fn usgs() -> Calendar {
        Calendar::new(Market::UsGovernmentBond)
    }

    #[test]
    fn reference_period_runs_imm_to_imm() -> R {
        let f = SofrFuture::new("Z26".parse()?, 96.0, 0.0, &usgs())?;
        assert_eq!((f.start(), f.end()), (d(2026, 12, 16), d(2027, 3, 17)));
        assert_abs_diff_eq!(f.futures_rate(), 0.04, epsilon = 1e-15);
        assert!(SofrFuture::new("Z26".parse()?, 200.0, 0.0, &usgs()).is_err());
        Ok(())
    }

    #[test]
    fn reprices_on_a_flat_curve_and_convexity_shifts_the_forward() -> R {
        let today = d(2026, 10, 13);
        let index = OvernightIndex::sofr();
        let curve = DiscountCurve::flat(today, 0.04, d(2030, 1, 1))?;
        let f0 = SofrFuture::new("H27".parse()?, 96.0, 0.0, &usgs())?;
        let r = f0.curve_rate(&index, &curve)?;
        // 91 days on a 4% cc ACT/365F curve, quoted ACT/360: (e^{0.04*91/365} - 1) * 360/91 = 3.965%.
        assert_abs_diff_eq!(r, ((0.04_f64 * 91.0 / 365.0).exp() - 1.0) * 360.0 / 91.0, epsilon = 1e-12);
        assert_abs_diff_eq!(f0.fair_price(&index, &curve)?, 100.0 * (1.0 - r), epsilon = 1e-12);
        assert_abs_diff_eq!(f0.npv(&index, &curve)?, r - 0.04, epsilon = 1e-15);
        // A 3bp convexity adjustment lowers the forward the curve must hit.
        let f3 = SofrFuture::new("H27".parse()?, 96.0, 0.0003, &usgs())?;
        assert_abs_diff_eq!(f3.implied_forward(), 0.0397, epsilon = 1e-15);
        assert_abs_diff_eq!(f3.npv(&index, &curve)? - f0.npv(&index, &curve)?, 0.0003, epsilon = 1e-15);
        Ok(())
    }
}
