//! Overnight (RFR) index with compounded-in-arrears setting.
//!
//! Each business day's rate applies from that day up to (not including) the
//! next business day, so a Friday fixing carries a weight of 3 calendar days.
//! Past days use stored fixings; future days use the simple forward implied
//! by a discount curve. Supports lookback (with or without observation
//! shift) and lockout as in the ISDA/ARRC conventions.

use std::collections::BTreeMap;

use thiserror::Error;

use fq_core::time::{BusinessDayConvention, Calendar, Date, DateError, DayCount, DayCountError, Market};

use crate::curve::{CurveError, DiscountCurve};

#[derive(Debug, Error, Clone, PartialEq)]
pub enum IndexError {
    #[error(transparent)]
    Date(#[from] DateError),
    #[error(transparent)]
    DayCount(#[from] DayCountError),
    #[error(transparent)]
    Curve(#[from] CurveError),
    #[error("no fixing for {index} on {date} and no curve to project it")]
    MissingFixing { index: String, date: Date },
    #[error("accrual period {start}..{end} is empty or reversed")]
    EmptyPeriod { start: Date, end: Date },
}

/// In-arrears compounding adjustments. Defaults to plain compounding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CompoundingConvention {
    /// Business days to look back for each day's rate.
    pub lookback_days: i32,
    /// With lookback: also take the day weights from the shifted observation period.
    pub observation_shift: bool,
    /// Freeze the rate for the last `lockout_days` business days of the period.
    pub lockout_days: i32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OvernightIndex {
    name: String,
    fixing_calendar: Calendar,
    day_count: DayCount,
    fixings: BTreeMap<Date, f64>,
}

/// One compounding step: the observation date whose fixing applies and the
/// calendar-day weight it carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObservationPeriod {
    pub observation_date: Date,
    pub days: i32,
}

impl OvernightIndex {
    pub fn new(name: impl Into<String>, fixing_calendar: Calendar, day_count: DayCount) -> Self {
        Self { name: name.into(), fixing_calendar, day_count, fixings: BTreeMap::new() }
    }

    /// SOFR: published on US government securities business days, ACT/360.
    pub fn sofr() -> Self {
        Self::new("SOFR", Calendar::new(Market::UsGovernmentBond), DayCount::Act360)
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn fixing_calendar(&self) -> &Calendar {
        &self.fixing_calendar
    }

    pub fn day_count(&self) -> DayCount {
        self.day_count
    }

    pub fn add_fixing(&mut self, date: Date, rate: f64) {
        self.fixings.insert(date, rate);
    }

    pub fn add_fixings(&mut self, fixings: impl IntoIterator<Item = (Date, f64)>) {
        self.fixings.extend(fixings);
    }

    pub fn fixing(&self, date: Date) -> Option<f64> {
        self.fixings.get(&date).copied()
    }

    pub fn fixings(&self) -> &BTreeMap<Date, f64> {
        &self.fixings
    }

    /// The rate for `observation_date`: a stored fixing, else the curve's
    /// simple forward to the next business day.
    pub fn rate_on(&self, observation_date: Date, curve: Option<&DiscountCurve>) -> Result<f64, IndexError> {
        if let Some(r) = self.fixing(observation_date) {
            return Ok(r);
        }
        let missing = || IndexError::MissingFixing { index: self.name.clone(), date: observation_date };
        let curve = curve.ok_or_else(missing)?;
        if observation_date < curve.reference_date() {
            return Err(missing());
        }
        let next = self.fixing_calendar.advance(observation_date, 1)?;
        Ok(curve.forward_rate(observation_date, next, self.day_count)?)
    }

    /// Observation dates and weights covering [start, end).
    pub fn observation_periods(&self, start: Date, end: Date, conv: CompoundingConvention) -> Result<Vec<ObservationPeriod>, IndexError> {
        if end <= start {
            return Err(IndexError::EmptyPeriod { start, end });
        }
        let cal = &self.fixing_calendar;
        let mut out = Vec::new();
        let mut d = start;
        while d < end {
            let next = cal.advance(d, 1)?.min(end);
            // A non-business accrual day belongs to the preceding fixing.
            let accrual_day = cal.adjust(d, BusinessDayConvention::Preceding)?;
            let observation_date = cal.advance(accrual_day, -conv.lookback_days)?;
            let days = if conv.observation_shift && conv.lookback_days > 0 {
                cal.advance(observation_date, 1)? - observation_date
            } else {
                next - d
            };
            out.push(ObservationPeriod { observation_date, days });
            d = next;
        }
        let lock = conv.lockout_days.max(0) as usize;
        if lock > 0 && out.len() > lock {
            let anchor = out[out.len() - lock - 1].observation_date;
            for p in &mut out[out.len() - lock..] {
                p.observation_date = anchor;
            }
        }
        Ok(out)
    }

    /// Compounding factor prod(1 + r_i * days_i / basis) over [start, end).
    pub fn compounded_factor(&self, start: Date, end: Date, curve: Option<&DiscountCurve>, conv: CompoundingConvention) -> Result<f64, IndexError> {
        let mut factor = 1.0;
        for p in self.observation_periods(start, end, conv)? {
            let r = self.rate_on(p.observation_date, curve)?;
            let yf = self.day_count.year_fraction(p.observation_date, p.observation_date + p.days, None)?;
            factor *= 1.0 + r * yf;
        }
        Ok(factor)
    }

    /// Annualised compounded rate over [start, end) in the index day count.
    /// With observation shift the denominator is the shifted observation
    /// period's length, per the convention.
    pub fn compounded_rate(&self, start: Date, end: Date, curve: Option<&DiscountCurve>, conv: CompoundingConvention) -> Result<f64, IndexError> {
        let factor = self.compounded_factor(start, end, curve, conv)?;
        let days: i32 = if conv.observation_shift && conv.lookback_days > 0 {
            self.observation_periods(start, end, conv)?.iter().map(|p| p.days).sum()
        } else {
            end - start
        };
        let yf = self.day_count.year_fraction(start, start + days, None)?;
        Ok((factor - 1.0) / yf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_abs_diff_eq;

    type R = Result<(), IndexError>;

    fn d(y: i32, m: u32, dd: u32) -> Date {
        Date::from_ymd(y, m, dd).expect("valid test date")
    }

    /// Mon 2026-09-21 .. Fri 2026-09-25 fixings.
    fn week_index() -> OvernightIndex {
        let mut idx = OvernightIndex::sofr();
        idx.add_fixings([(d(2026, 9, 21), 0.040), (d(2026, 9, 22), 0.041), (d(2026, 9, 23), 0.042), (d(2026, 9, 24), 0.043), (d(2026, 9, 25), 0.044)]);
        idx
    }

    #[test]
    fn weights_span_weekends_and_holidays() -> R {
        let idx = OvernightIndex::sofr();
        let periods = idx.observation_periods(d(2026, 9, 21), d(2026, 9, 28), CompoundingConvention::default())?;
        let days: Vec<i32> = periods.iter().map(|p| p.days).collect();
        assert_eq!(days, [1, 1, 1, 1, 3]);
        assert_eq!(periods[4].observation_date, d(2026, 9, 25));
        // Thu 2026-04-02 covers Good Friday and the weekend.
        let gf = idx.observation_periods(d(2026, 4, 2), d(2026, 4, 7), CompoundingConvention::default())?;
        assert_eq!(gf.iter().map(|p| p.days).collect::<Vec<_>>(), [4, 1]);
        Ok(())
    }

    #[test]
    fn plain_compounding_by_hand() -> R {
        let idx = week_index();
        let (s, e) = (d(2026, 9, 21), d(2026, 9, 28));
        let factor = (1.0 + 0.040 / 360.0) * (1.0 + 0.041 / 360.0) * (1.0 + 0.042 / 360.0) * (1.0 + 0.043 / 360.0) * (1.0 + 0.044 * 3.0 / 360.0);
        assert_abs_diff_eq!(idx.compounded_factor(s, e, None, CompoundingConvention::default())?, factor, epsilon = 1e-15);
        assert_abs_diff_eq!(idx.compounded_rate(s, e, None, CompoundingConvention::default())?, (factor - 1.0) * 360.0 / 7.0, epsilon = 1e-15);
        Ok(())
    }

    #[test]
    fn lookback_without_shift_keeps_weights() -> R {
        let idx = week_index();
        let conv = CompoundingConvention { lookback_days: 2, ..Default::default() };
        // Accrual Wed..Mon: Wed uses Mon's fixing, Thu uses Tue's, Fri (weight 3) uses Wed's.
        let periods = idx.observation_periods(d(2026, 9, 23), d(2026, 9, 28), conv)?;
        let obs: Vec<Date> = periods.iter().map(|p| p.observation_date).collect();
        assert_eq!(obs, [d(2026, 9, 21), d(2026, 9, 22), d(2026, 9, 23)]);
        assert_eq!(periods.iter().map(|p| p.days).collect::<Vec<_>>(), [1, 1, 3]);
        let factor = (1.0 + 0.040 / 360.0) * (1.0 + 0.041 / 360.0) * (1.0 + 0.042 * 3.0 / 360.0);
        assert_abs_diff_eq!(idx.compounded_factor(d(2026, 9, 23), d(2026, 9, 28), None, conv)?, factor, epsilon = 1e-15);
        Ok(())
    }

    #[test]
    fn observation_shift_uses_shifted_weights() -> R {
        let idx = week_index();
        let conv = CompoundingConvention { lookback_days: 2, observation_shift: true, ..Default::default() };
        // Accrual Fri 25 and Mon 28 look back two business days to Wed 23 and
        // Thu 24; the weights come from those observation days (1 each), not
        // from the accrual days (3 and 1).
        let periods = idx.observation_periods(d(2026, 9, 25), d(2026, 9, 29), conv)?;
        let pairs: Vec<(Date, i32)> = periods.iter().map(|p| (p.observation_date, p.days)).collect();
        assert_eq!(pairs, [(d(2026, 9, 23), 1), (d(2026, 9, 24), 1)]);
        let factor = (1.0 + 0.042 / 360.0) * (1.0 + 0.043 / 360.0);
        assert_abs_diff_eq!(idx.compounded_rate(d(2026, 9, 25), d(2026, 9, 29), None, conv)?, (factor - 1.0) * 360.0 / 2.0, epsilon = 1e-15);
        Ok(())
    }

    #[test]
    fn lockout_freezes_last_days() -> R {
        let idx = week_index();
        let conv = CompoundingConvention { lockout_days: 2, ..Default::default() };
        let periods = idx.observation_periods(d(2026, 9, 21), d(2026, 9, 28), conv)?;
        let obs: Vec<Date> = periods.iter().map(|p| p.observation_date).collect();
        assert_eq!(obs, [d(2026, 9, 21), d(2026, 9, 22), d(2026, 9, 23), d(2026, 9, 23), d(2026, 9, 23)]);
        Ok(())
    }

    #[test]
    fn curve_projection_matches_simple_forward_exactly() -> R {
        let idx = OvernightIndex::sofr();
        let (s, e) = (d(2026, 9, 23), d(2026, 12, 23));
        let curve = DiscountCurve::flat(s, 0.045, d(2028, 9, 23))?;
        // Compounding daily simple forwards telescopes to DF(s)/DF(e).
        let expected = (curve.discount(s)? / curve.discount(e)? - 1.0) * 360.0 / f64::from(e - s);
        assert_abs_diff_eq!(idx.compounded_rate(s, e, Some(&curve), CompoundingConvention::default())?, expected, epsilon = 1e-13);
        Ok(())
    }

    #[test]
    fn fixings_take_precedence_and_missing_is_an_error() -> R {
        let mut idx = OvernightIndex::sofr();
        let curve = DiscountCurve::flat(d(2026, 9, 23), 0.045, d(2028, 9, 23))?;
        idx.add_fixing(d(2026, 9, 23), 0.05);
        assert_abs_diff_eq!(idx.rate_on(d(2026, 9, 23), Some(&curve))?, 0.05, epsilon = 1e-15);
        assert!(matches!(idx.rate_on(d(2026, 9, 22), Some(&curve)), Err(IndexError::MissingFixing { .. })));
        assert!(matches!(idx.rate_on(d(2026, 9, 24), None), Err(IndexError::MissingFixing { .. })));
        assert!(matches!(idx.observation_periods(d(2026, 9, 24), d(2026, 9, 24), CompoundingConvention::default()), Err(IndexError::EmptyPeriod { .. })));
        Ok(())
    }
}
