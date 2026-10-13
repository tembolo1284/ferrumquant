//! Tenors: a count of days, weeks, months or years.
//!
//! Periods are stored normalised the way QuantLib does it: a whole number of
//! weeks is kept in weeks, a whole number of years in years, so `12M == 1Y`
//! and `14D == 2W`. Date arithmetic goes through `Date` (`add_months` clamps
//! to month length; business-day adjustment is the caller's job).

use std::cmp::Ordering;
use std::fmt;
use std::str::FromStr;

use thiserror::Error;

use super::date::{Date, DateError};

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum PeriodError {
    #[error("invalid period '{0}' (expected e.g. 1D, 2W, 6M, 10Y)")]
    Parse(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TimeUnit {
    Days,
    Weeks,
    Months,
    Years,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Period {
    n: i32,
    unit: TimeUnit,
}

impl Period {
    pub const fn new(n: i32, unit: TimeUnit) -> Self {
        Self { n, unit }.normalised()
    }

    pub const fn days(n: i32) -> Self {
        Self::new(n, TimeUnit::Days)
    }

    pub const fn weeks(n: i32) -> Self {
        Self::new(n, TimeUnit::Weeks)
    }

    pub const fn months(n: i32) -> Self {
        Self::new(n, TimeUnit::Months)
    }

    pub const fn years(n: i32) -> Self {
        Self::new(n, TimeUnit::Years)
    }

    pub const fn length(self) -> i32 {
        self.n
    }

    pub const fn unit(self) -> TimeUnit {
        self.unit
    }

    /// Whole months for month- or year-based periods, `None` for days/weeks.
    pub const fn in_months(self) -> Option<i32> {
        match self.unit {
            TimeUnit::Months => Some(self.n),
            TimeUnit::Years => Some(self.n * 12),
            TimeUnit::Days | TimeUnit::Weeks => None,
        }
    }

    /// Whole days for day- or week-based periods, `None` for months/years.
    pub const fn in_days(self) -> Option<i32> {
        match self.unit {
            TimeUnit::Days => Some(self.n),
            TimeUnit::Weeks => Some(self.n * 7),
            TimeUnit::Months | TimeUnit::Years => None,
        }
    }

    pub const fn is_zero(self) -> bool {
        self.n == 0
    }

    pub fn add_to(self, d: Date) -> Result<Date, DateError> {
        match self.unit {
            TimeUnit::Days => d.checked_add_days(self.n),
            TimeUnit::Weeks => d.checked_add_days(7 * self.n),
            TimeUnit::Months => d.add_months(self.n),
            TimeUnit::Years => d.add_years(self.n),
        }
    }

    pub fn subtract_from(self, d: Date) -> Result<Date, DateError> {
        (-self).add_to(d)
    }

    /// Approximate length in calendar days, for ordering only.
    fn approx_days(self) -> f64 {
        let n = f64::from(self.n);
        match self.unit {
            TimeUnit::Days => n,
            TimeUnit::Weeks => 7.0 * n,
            TimeUnit::Months => n * 365.25 / 12.0,
            TimeUnit::Years => n * 365.25,
        }
    }

    const fn normalised(self) -> Self {
        match self.unit {
            TimeUnit::Days if self.n % 7 == 0 && self.n != 0 => Self { n: self.n / 7, unit: TimeUnit::Weeks },
            TimeUnit::Months if self.n % 12 == 0 && self.n != 0 => Self { n: self.n / 12, unit: TimeUnit::Years },
            _ => self,
        }
    }
}

impl std::ops::Neg for Period {
    type Output = Period;
    fn neg(self) -> Period {
        Period { n: -self.n, unit: self.unit }
    }
}

impl PartialOrd for Period {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Orders by approximate calendar length, with the unit as a tiebreak so the
/// order is total and consistent with `Eq` (which is structural).
impl Ord for Period {
    fn cmp(&self, other: &Self) -> Ordering {
        self.approx_days()
            .partial_cmp(&other.approx_days())
            .unwrap_or(Ordering::Equal)
            .then_with(|| (self.unit as u8).cmp(&(other.unit as u8)))
            .then_with(|| self.n.cmp(&other.n))
    }
}

impl fmt::Display for Period {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let u = match self.unit {
            TimeUnit::Days => 'D',
            TimeUnit::Weeks => 'W',
            TimeUnit::Months => 'M',
            TimeUnit::Years => 'Y',
        };
        write!(f, "{}{}", self.n, u)
    }
}

/// Accepts "1D", "2W", "6M", "10Y" (case-insensitive), plus "ON"/"TN"/"SN"
/// as 1D and "1M" style with surrounding whitespace.
impl FromStr for Period {
    type Err = PeriodError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let t = s.trim().to_ascii_uppercase();
        let bad = || PeriodError::Parse(s.to_owned());
        match t.as_str() {
            "ON" | "O/N" | "TN" | "T/N" | "SN" | "S/N" => return Ok(Self::days(1)),
            _ => {}
        }
        let (num, unit) = t.split_at(t.len().checked_sub(1).ok_or_else(bad)?);
        let n: i32 = num.trim().parse().map_err(|_| bad())?;
        let unit = match unit {
            "D" => TimeUnit::Days,
            "W" => TimeUnit::Weeks,
            "M" => TimeUnit::Months,
            "Y" => TimeUnit::Years,
            _ => return Err(bad()),
        };
        Ok(Self::new(n, unit))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type R = Result<(), Box<dyn std::error::Error>>;

    fn d(y: i32, m: u32, dd: u32) -> Date {
        Date::from_ymd(y, m, dd).expect("valid test date")
    }

    #[test]
    fn normalisation_and_equality() {
        assert_eq!(Period::months(12), Period::years(1));
        assert_eq!(Period::months(24), Period::years(2));
        assert_eq!(Period::days(14), Period::weeks(2));
        assert_ne!(Period::months(6), Period::years(1));
        assert_eq!(Period::months(18).unit(), TimeUnit::Months);
        assert_eq!(Period::days(0), Period::days(0));
        assert_eq!(Period::years(1).in_months(), Some(12));
        assert_eq!(Period::weeks(2).in_days(), Some(14));
        assert_eq!(Period::weeks(2).in_months(), None);
    }

    #[test]
    fn display_and_parse_round_trip() -> R {
        for s in ["1D", "2W", "6M", "18M", "10Y"] {
            assert_eq!(s.parse::<Period>()?.to_string(), s);
        }
        assert_eq!("12m".parse::<Period>()?, Period::years(1));
        assert_eq!("12M".parse::<Period>()?.to_string(), "1Y");
        assert_eq!(" 7d ".parse::<Period>()?, Period::weeks(1));
        assert_eq!("ON".parse::<Period>()?, Period::days(1));
        assert!("".parse::<Period>().is_err());
        assert!("5Q".parse::<Period>().is_err());
        assert!("Y".parse::<Period>().is_err());
        Ok(())
    }

    #[test]
    fn date_arithmetic() -> R {
        let base = d(2026, 1, 31);
        assert_eq!(Period::days(1).add_to(base)?, d(2026, 2, 1));
        assert_eq!(Period::weeks(1).add_to(base)?, d(2026, 2, 7));
        assert_eq!(Period::months(1).add_to(base)?, d(2026, 2, 28)); // clamped
        assert_eq!(Period::years(2).add_to(d(2024, 2, 29))?, d(2026, 2, 28));
        assert_eq!(Period::months(3).subtract_from(d(2026, 5, 31))?, d(2026, 2, 28));
        assert_eq!((-Period::weeks(2)).add_to(d(2026, 1, 15))?, d(2026, 1, 1));
        Ok(())
    }

    #[test]
    fn ordering_is_by_length() {
        let mut v = [Period::years(1), Period::days(1), Period::months(6), Period::weeks(2), Period::months(1), Period::days(5)];
        v.sort();
        let s: Vec<String> = v.iter().map(ToString::to_string).collect();
        assert_eq!(s, ["1D", "5D", "2W", "1M", "6M", "1Y"]);
        assert!(Period::weeks(4) < Period::months(1));
        assert!(Period::months(12) == Period::years(1));
    }
}
