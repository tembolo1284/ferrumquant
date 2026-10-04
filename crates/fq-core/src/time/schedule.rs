//! Coupon schedule generation.
//!
//! Mirrors QuantLib's `Schedule` for the Backward and Forward rules: explicit
//! first / next-to-last stub dates, the end-of-month rule, duplicate removal
//! after adjustment, and per-period regularity flags. `ref_period` returns the
//! (possibly notional) coupon period that ACT/ACT ICMA needs for odd coupons.

use std::fmt;
use std::str::FromStr;

use thiserror::Error;

use super::calendar::{BusinessDayConvention, Calendar};
use super::date::{Date, DateError};
use super::daycount::RefPeriod;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ScheduleError {
    #[error(transparent)]
    Date(#[from] DateError),
    #[error("invalid schedule: {0}")]
    Invalid(String),
    #[error("unknown frequency '{0}'")]
    UnknownFrequency(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Frequency {
    Annual,
    Semiannual,
    Quarterly,
    Monthly,
}

impl Frequency {
    pub const fn months(self) -> i32 {
        match self {
            Self::Annual => 12,
            Self::Semiannual => 6,
            Self::Quarterly => 3,
            Self::Monthly => 1,
        }
    }

    pub const fn per_year(self) -> u32 {
        (12 / self.months()) as u32
    }
}

impl fmt::Display for Frequency {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}

impl FromStr for Frequency {
    type Err = ScheduleError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s.trim().to_ascii_uppercase().as_str() {
            "A" | "1Y" | "12M" | "ANNUAL" => Self::Annual,
            "S" | "SA" | "6M" | "SEMIANNUAL" => Self::Semiannual,
            "Q" | "3M" | "QUARTERLY" => Self::Quarterly,
            "M" | "1M" | "MONTHLY" => Self::Monthly,
            _ => return Err(ScheduleError::UnknownFrequency(s.to_owned())),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum DateGeneration {
    /// Roll back from termination; stub (if any) at the front. Bonds and swaps.
    #[default]
    Backward,
    /// Roll forward from effective; stub (if any) at the back.
    Forward,
}

/// Inputs for a schedule. Build with `new` then the chainable setters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduleSpec {
    pub effective: Date,
    pub termination: Date,
    pub frequency: Frequency,
    pub calendar: Calendar,
    pub convention: BusinessDayConvention,
    pub termination_convention: BusinessDayConvention,
    pub rule: DateGeneration,
    pub end_of_month: bool,
    pub first_date: Option<Date>,
    pub next_to_last_date: Option<Date>,
}

impl ScheduleSpec {
    /// Defaults: ModifiedFollowing (both ends), Backward, no EOM, no stubs.
    pub fn new(effective: Date, termination: Date, frequency: Frequency, calendar: Calendar) -> Self {
        Self {
            effective,
            termination,
            frequency,
            calendar,
            convention: BusinessDayConvention::ModifiedFollowing,
            termination_convention: BusinessDayConvention::ModifiedFollowing,
            rule: DateGeneration::Backward,
            end_of_month: false,
            first_date: None,
            next_to_last_date: None,
        }
    }

    /// Sets both the regular and the termination-date convention.
    pub fn convention(mut self, bdc: BusinessDayConvention) -> Self {
        self.convention = bdc;
        self.termination_convention = bdc;
        self
    }

    pub fn termination_convention(mut self, bdc: BusinessDayConvention) -> Self {
        self.termination_convention = bdc;
        self
    }

    pub fn rule(mut self, rule: DateGeneration) -> Self {
        self.rule = rule;
        self
    }

    pub fn end_of_month(mut self, eom: bool) -> Self {
        self.end_of_month = eom;
        self
    }

    pub fn first_date(mut self, d: Date) -> Self {
        self.first_date = Some(d);
        self
    }

    pub fn next_to_last_date(mut self, d: Date) -> Self {
        self.next_to_last_date = Some(d);
        self
    }

    pub fn build(&self) -> Result<Schedule, ScheduleError> {
        Schedule::generate(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Schedule {
    dates: Vec<Date>,
    adjusted: Vec<Date>,
    regular: Vec<bool>,
    frequency: Frequency,
    end_of_month: bool,
}

impl Schedule {
    pub fn generate(spec: &ScheduleSpec) -> Result<Self, ScheduleError> {
        validate(spec)?;
        let m = spec.frequency.months();
        let (dates, regular) = match spec.rule {
            DateGeneration::Backward => backward(spec, m)?,
            DateGeneration::Forward => forward(spec, m)?,
        };

        let last = dates.len() - 1;
        let adjusted = dates
            .iter()
            .enumerate()
            .map(|(i, &d)| {
                let bdc = if i == last { spec.termination_convention } else { spec.convention };
                spec.calendar.adjust(d, bdc)
            })
            .collect::<Result<Vec<_>, _>>()?;

        Ok(Self { dates, adjusted, regular, frequency: spec.frequency, end_of_month: spec.end_of_month })
    }

    /// Unadjusted dates (accrual dates for bonds).
    pub fn dates(&self) -> &[Date] {
        &self.dates
    }

    /// Business-day adjusted dates.
    pub fn adjusted_dates(&self) -> &[Date] {
        &self.adjusted
    }

    pub fn periods(&self) -> usize {
        self.regular.len()
    }

    pub fn is_regular(&self, i: usize) -> bool {
        self.regular[i]
    }

    pub fn frequency(&self) -> Frequency {
        self.frequency
    }

    /// Unadjusted (start, end) of period `i`.
    pub fn period(&self, i: usize) -> (Date, Date) {
        (self.dates[i], self.dates[i + 1])
    }

    /// Coupon reference period for ACT/ACT ICMA: the period itself if regular,
    /// otherwise the notional regular period ending (first stub) or starting
    /// (other stubs) at the period's coupon-side date.
    pub fn ref_period(&self, i: usize) -> Result<RefPeriod, ScheduleError> {
        let (s, e) = self.period(i);
        if self.regular[i] {
            return Ok(RefPeriod::new(s, e));
        }
        let m = self.frequency.months();
        if i == 0 {
            Ok(RefPeriod::new(roll(e, -m, self.end_of_month)?, e))
        } else {
            Ok(RefPeriod::new(s, roll(s, m, self.end_of_month)?))
        }
    }
}

fn validate(s: &ScheduleSpec) -> Result<(), ScheduleError> {
    let bad = |msg: String| Err(ScheduleError::Invalid(msg));
    if s.effective >= s.termination {
        return bad(format!("effective {} not before termination {}", s.effective, s.termination));
    }
    if let Some(fd) = s.first_date {
        if fd <= s.effective || fd > s.termination {
            return bad(format!("first date {fd} outside ({}, {}]", s.effective, s.termination));
        }
    }
    if let Some(ntl) = s.next_to_last_date {
        if ntl < s.effective || ntl >= s.termination {
            return bad(format!("next-to-last date {ntl} outside [{}, {})", s.effective, s.termination));
        }
        if s.first_date.is_some_and(|fd| ntl < fd) {
            return bad(format!("next-to-last date {ntl} before first date"));
        }
    }
    Ok(())
}

/// Calendar-month roll; with EOM, a month-end seed stays on month ends.
fn roll(seed: Date, months: i32, eom: bool) -> Result<Date, DateError> {
    let d = seed.add_months(months)?;
    Ok(if eom && seed.is_end_of_month() { d.end_of_month() } else { d })
}

fn same_adjusted(s: &ScheduleSpec, a: Date, b: Date, bdc: BusinessDayConvention) -> Result<bool, DateError> {
    Ok(s.calendar.adjust(a, bdc)? == s.calendar.adjust(b, bdc)?)
}

type Generated = (Vec<Date>, Vec<bool>);

fn backward(s: &ScheduleSpec, m: i32) -> Result<Generated, ScheduleError> {
    let eom = s.end_of_month;
    let mut dates = vec![s.termination];
    let mut regular = Vec::new();

    let seed = match s.next_to_last_date {
        Some(ntl) => {
            dates.push(ntl);
            regular.push(roll(ntl, m, eom)? == s.termination);
            ntl
        }
        None => s.termination,
    };
    let exit = s.first_date.unwrap_or(s.effective);

    for i in 1.. {
        let temp = roll(seed, -m * i, eom)?;
        let back = dates[dates.len() - 1];
        if temp < exit {
            if let Some(fd) = s.first_date {
                if !same_adjusted(s, back, fd, s.convention)? {
                    dates.push(fd);
                    regular.push(false);
                }
            }
            break;
        }
        if !same_adjusted(s, back, temp, s.convention)? {
            dates.push(temp);
            regular.push(true);
        }
    }

    let back = dates[dates.len() - 1];
    if !same_adjusted(s, back, s.effective, s.termination_convention)? {
        dates.push(s.effective);
        regular.push(false);
    }

    dates.reverse();
    regular.reverse();
    Ok((dates, regular))
}

fn forward(s: &ScheduleSpec, m: i32) -> Result<Generated, ScheduleError> {
    let eom = s.end_of_month;
    let mut dates = vec![s.effective];
    let mut regular = Vec::new();

    let seed = match s.first_date {
        Some(fd) => {
            dates.push(fd);
            regular.push(roll(s.effective, m, eom)? == fd);
            fd
        }
        None => s.effective,
    };
    let exit = s.next_to_last_date.unwrap_or(s.termination);

    for i in 1.. {
        let temp = roll(seed, m * i, eom)?;
        let back = dates[dates.len() - 1];
        if temp > exit {
            if let Some(ntl) = s.next_to_last_date {
                if !same_adjusted(s, back, ntl, s.convention)? {
                    dates.push(ntl);
                    regular.push(false);
                }
            }
            break;
        }
        if !same_adjusted(s, back, temp, s.convention)? {
            dates.push(temp);
            regular.push(true);
        }
    }

    let back = dates[dates.len() - 1];
    if !same_adjusted(s, back, s.termination, s.termination_convention)? {
        dates.push(s.termination);
        regular.push(false);
    }
    Ok((dates, regular))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time::calendar::Market;
    use BusinessDayConvention as C;

    type R = Result<(), ScheduleError>;

    fn d(y: i32, m: u32, dd: u32) -> Date {
        Date::from_ymd(y, m, dd).expect("valid test date")
    }

    fn ust(effective: Date, maturity: Date) -> ScheduleSpec {
        ScheduleSpec::new(effective, maturity, Frequency::Semiannual, Calendar::new(Market::UsGovernmentBond))
            .convention(C::Unadjusted)
    }

    #[test]
    fn regular_ten_year_note() -> R {
        let s = ust(d(2026, 8, 15), d(2036, 8, 15)).build()?;
        assert_eq!(s.periods(), 20);
        assert_eq!(s.dates()[1], d(2027, 2, 15));
        assert!((0..s.periods()).all(|i| s.is_regular(i)));
        Ok(())
    }

    #[test]
    fn end_of_month_rule() -> R {
        let eom = ust(d(2026, 2, 28), d(2028, 2, 29)).end_of_month(true).build()?;
        assert_eq!(
            eom.dates(),
            &[d(2026, 2, 28), d(2026, 8, 31), d(2027, 2, 28), d(2027, 8, 31), d(2028, 2, 29)]
        );
        let plain = ust(d(2026, 2, 28), d(2028, 2, 29)).build()?;
        assert_eq!(
            plain.dates(),
            &[d(2026, 2, 28), d(2026, 8, 29), d(2027, 2, 28), d(2027, 8, 29), d(2028, 2, 29)]
        );
        Ok(())
    }

    #[test]
    fn short_first_stub() -> R {
        let s = ust(d(2026, 9, 22), d(2031, 8, 15)).build()?;
        assert_eq!(s.dates()[..2], [d(2026, 9, 22), d(2027, 2, 15)]);
        assert!(!s.is_regular(0));
        assert!(s.is_regular(1));
        assert_eq!(s.ref_period(0)?, RefPeriod::new(d(2026, 8, 15), d(2027, 2, 15)));
        Ok(())
    }

    #[test]
    fn long_first_coupon_via_first_date() -> R {
        let s = ust(d(2026, 9, 22), d(2031, 8, 15)).first_date(d(2027, 8, 15)).build()?;
        assert_eq!(s.dates()[..3], [d(2026, 9, 22), d(2027, 8, 15), d(2028, 2, 15)]);
        assert!(!s.is_regular(0));
        assert_eq!(s.ref_period(0)?, RefPeriod::new(d(2027, 2, 15), d(2027, 8, 15)));
        Ok(())
    }

    #[test]
    fn forward_with_short_last_stub() -> R {
        let spec = ScheduleSpec::new(d(2026, 1, 15), d(2027, 3, 1), Frequency::Quarterly, Calendar::new(Market::UsSettlement))
            .rule(DateGeneration::Forward);
        let s = spec.build()?;
        assert_eq!(s.dates().last(), Some(&d(2027, 3, 1)));
        assert_eq!(s.dates()[s.periods() - 1], d(2027, 1, 15));
        assert!(!s.is_regular(s.periods() - 1));
        assert_eq!(s.ref_period(s.periods() - 1)?, RefPeriod::new(d(2027, 1, 15), d(2027, 4, 15)));
        Ok(())
    }

    #[test]
    fn adjusted_dates_modified_following() -> R {
        let cal: Calendar = "USGS+USNY".parse().map_err(|e| ScheduleError::Invalid(format!("{e}")))?;
        let s = ScheduleSpec::new(d(2026, 9, 24), d(2031, 9, 24), Frequency::Annual, cal).build()?;
        assert_eq!(s.dates()[2], d(2028, 9, 24)); // Sunday
        assert_eq!(s.adjusted_dates()[2], d(2028, 9, 25));
        Ok(())
    }

    #[test]
    fn invalid_specs_rejected() {
        let cal = Calendar::new(Market::WeekendsOnly);
        let bad = ScheduleSpec::new(d(2027, 1, 1), d(2026, 1, 1), Frequency::Annual, cal.clone()).build();
        assert!(matches!(bad, Err(ScheduleError::Invalid(_))));
        let bad_stub = ScheduleSpec::new(d(2026, 1, 1), d(2027, 1, 1), Frequency::Annual, cal)
            .first_date(d(2025, 6, 1))
            .build();
        assert!(matches!(bad_stub, Err(ScheduleError::Invalid(_))));
    }

    #[test]
    fn parse_frequency() -> R {
        assert_eq!("SA".parse::<Frequency>()?, Frequency::Semiannual);
        assert_eq!("3M".parse::<Frequency>()?, Frequency::Quarterly);
        assert_eq!(Frequency::Semiannual.per_year(), 2);
        Ok(())
    }
}
