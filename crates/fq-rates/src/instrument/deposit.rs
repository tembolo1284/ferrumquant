//! Cash deposit: simple interest between two dates. On a SOFR curve the only
//! one that matters is the overnight deposit (today to the next business
//! day at the SOFR fixing), which pins the very front of the curve. Term
//! deposits are supported for other markets.

use thiserror::Error;

use fq_core::time::{BusinessDayConvention, Calendar, Date, DateError, DayCount, DayCountError, Market, Period};

use crate::curve::{CurveError, DiscountCurve};

#[derive(Debug, Error, Clone, PartialEq)]
pub enum DepositError {
    #[error(transparent)]
    Date(#[from] DateError),
    #[error(transparent)]
    DayCount(#[from] DayCountError),
    #[error(transparent)]
    Curve(#[from] CurveError),
    #[error("deposit period {start}..{end} is empty or reversed")]
    EmptyPeriod { start: Date, end: Date },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Deposit {
    start: Date,
    end: Date,
    rate: f64,
    day_count: DayCount,
    year_fraction: f64,
}

impl Deposit {
    pub fn new(start: Date, end: Date, rate: f64, day_count: DayCount) -> Result<Self, DepositError> {
        if end <= start {
            return Err(DepositError::EmptyPeriod { start, end });
        }
        let year_fraction = day_count.year_fraction(start, end, None)?;
        Ok(Self { start, end, rate, day_count, year_fraction })
    }

    /// Deposit starting `settlement_days` business days after `trade_date`
    /// for `tenor`, end date ModifiedFollowing on `calendar`.
    pub fn term(trade_date: Date, settlement_days: i32, tenor: Period, rate: f64, day_count: DayCount, calendar: &Calendar) -> Result<Self, DepositError> {
        let start = calendar.advance(trade_date, settlement_days)?;
        let end = calendar.adjust(tenor.add_to(start)?, BusinessDayConvention::ModifiedFollowing)?;
        Self::new(start, end, rate, day_count)
    }

    /// Overnight SOFR deposit: today to the next USGS business day, ACT/360.
    pub fn sofr_overnight(today: Date, rate: f64) -> Result<Self, DepositError> {
        let cal = Calendar::new(Market::UsGovernmentBond);
        let start = cal.adjust(today, BusinessDayConvention::Following)?;
        let end = cal.advance(start, 1)?;
        Self::new(start, end, rate, DayCount::Act360)
    }

    pub fn start(&self) -> Date {
        self.start
    }

    pub fn end(&self) -> Date {
        self.end
    }

    pub fn rate(&self) -> f64 {
        self.rate
    }

    pub fn day_count(&self) -> DayCount {
        self.day_count
    }

    pub fn year_fraction(&self) -> f64 {
        self.year_fraction
    }

    /// `DF(end)` implied by `DF(start)` and the deposit rate.
    pub fn implied_discount(&self, df_start: f64) -> f64 {
        df_start / (1.0 + self.rate * self.year_fraction)
    }

    /// Simple forward the curve implies over the deposit period.
    pub fn curve_rate(&self, curve: &DiscountCurve) -> Result<f64, DepositError> {
        Ok(curve.forward_rate(self.start, self.end, self.day_count)?)
    }

    /// Per unit notional: PV of receiving `1 + r tau` at `end` against
    /// paying 1 at `start`. Zero when the curve reprices the deposit.
    pub fn npv(&self, curve: &DiscountCurve) -> Result<f64, DepositError> {
        let df_s = curve.discount(self.start)?;
        let df_e = curve.discount(self.end)?;
        Ok((1.0 + self.rate * self.year_fraction) * df_e - df_s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_abs_diff_eq;

    type R = Result<(), DepositError>;

    fn d(y: i32, m: u32, dd: u32) -> Date {
        Date::from_ymd(y, m, dd).expect("valid test date")
    }

    #[test]
    fn overnight_dates_and_weekend() -> R {
        let on = Deposit::sofr_overnight(d(2026, 10, 13), 0.0415)?;
        assert_eq!((on.start(), on.end()), (d(2026, 10, 13), d(2026, 10, 14)));
        assert_abs_diff_eq!(on.year_fraction(), 1.0 / 360.0, epsilon = 1e-15);
        // Friday: overnight runs to Tuesday over Columbus Day (Mon 2026-10-12).
        let fri = Deposit::sofr_overnight(d(2026, 10, 9), 0.0415)?;
        assert_eq!(fri.end(), d(2026, 10, 13));
        assert_abs_diff_eq!(fri.year_fraction(), 4.0 / 360.0, epsilon = 1e-15);
        Ok(())
    }

    #[test]
    fn term_deposit_uses_spot_and_modified_following() -> R {
        let cal = Calendar::new(Market::UsSettlement);
        // 2026-10-13 + 2bd = Thu 15; +1M = Sun 2026-11-15 -> Mon 16.
        let dep = Deposit::term(d(2026, 10, 13), 2, Period::months(1), 0.04, DayCount::Act360, &cal)?;
        assert_eq!((dep.start(), dep.end()), (d(2026, 10, 15), d(2026, 11, 16)));
        Ok(())
    }

    #[test]
    fn npv_is_zero_when_curve_reprices() -> R {
        let today = d(2026, 10, 13);
        let on = Deposit::sofr_overnight(today, 0.0415)?;
        let df_e = on.implied_discount(1.0);
        let curve = DiscountCurve::new(today, &[(on.end(), df_e)], crate::curve::Interpolation::LogLinearDiscount)?;
        assert_abs_diff_eq!(on.npv(&curve)?, 0.0, epsilon = 1e-15);
        assert_abs_diff_eq!(on.curve_rate(&curve)?, 0.0415, epsilon = 1e-13);
        // Richer curve than the deposit: NPV of lending at 4.15% is negative.
        let rich = DiscountCurve::flat(today, 0.05, d(2027, 10, 13))?;
        assert!(on.npv(&rich)? < 0.0);
        assert!(matches!(Deposit::new(today, today, 0.04, DayCount::Act360), Err(DepositError::EmptyPeriod { .. })));
        Ok(())
    }
}
