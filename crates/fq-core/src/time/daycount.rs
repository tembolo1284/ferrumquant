//! Day count conventions.
//!
//! `year_fraction` takes an optional reference (coupon) period, which only
//! ACT/ACT ICMA uses. The ICMA algorithm is a port of QuantLib's
//! `ActualActual::ISMA` (reference-period form), including its handling of
//! long/short first and last coupons via notional periods.

use std::fmt;
use std::str::FromStr;

use thiserror::Error;

use super::date::{Date, DateError, is_leap_year};

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum DayCountError {
    #[error(transparent)]
    Date(#[from] DateError),
    #[error("invalid reference period {start}..{end} for accrual {d1}..{d2}")]
    InvalidReferencePeriod { d1: Date, d2: Date, start: Date, end: Date },
    #[error("unknown day count convention '{0}'")]
    Unknown(String),
}

/// 30/360 variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Thirty360 {
    /// ISDA 2006 "30/360" / Bond Basis: D1 31 -> 30; D2 31 -> 30 if D1 is 30.
    BondBasis,
    /// 30/360 US (SIA), with the end-of-February rules.
    Us,
    /// 30E/360 (Eurobond Basis): any 31 -> 30.
    European,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DayCount {
    Act360,
    Act365Fixed,
    Thirty360(Thirty360),
    ActActIsda,
    /// ACT/ACT ICMA (ISMA): UST and most government bonds. Needs a reference period.
    ActActIcma,
}

/// The coupon period an accrual belongs to (regular or notional).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RefPeriod {
    pub start: Date,
    pub end: Date,
}

impl RefPeriod {
    pub const fn new(start: Date, end: Date) -> Self {
        Self { start, end }
    }
}

impl DayCount {
    /// Day count numerator under this convention.
    pub fn days(self, d1: Date, d2: Date) -> i32 {
        match self {
            Self::Thirty360(c) => thirty360_days(c, d1, d2),
            _ => d2 - d1,
        }
    }

    pub fn year_fraction(
        self,
        d1: Date,
        d2: Date,
        ref_period: Option<RefPeriod>,
    ) -> Result<f64, DayCountError> {
        match self {
            Self::Act360 => Ok(f64::from(d2 - d1) / 360.0),
            Self::Act365Fixed => Ok(f64::from(d2 - d1) / 365.0),
            Self::Thirty360(c) => Ok(f64::from(thirty360_days(c, d1, d2)) / 360.0),
            Self::ActActIsda => act_act_isda(d1, d2),
            Self::ActActIcma => {
                let rp = ref_period.unwrap_or(RefPeriod::new(d1, d2));
                act_act_icma(d1, d2, rp.start, rp.end)
            }
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::Act360 => "ACT/360",
            Self::Act365Fixed => "ACT/365F",
            Self::Thirty360(Thirty360::BondBasis) => "30/360",
            Self::Thirty360(Thirty360::Us) => "30/360 US",
            Self::Thirty360(Thirty360::European) => "30E/360",
            Self::ActActIsda => "ACT/ACT ISDA",
            Self::ActActIcma => "ACT/ACT ICMA",
        }
    }
}

impl fmt::Display for DayCount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl FromStr for DayCount {
    type Err = DayCountError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let key: String = s
            .to_ascii_uppercase()
            .replace("ACTUAL", "ACT")
            .chars()
            .filter(|c| !matches!(c, ' ' | '-' | '_'))
            .collect();
        Ok(match key.as_str() {
            "ACT/360" | "A360" => Self::Act360,
            "ACT/365F" | "ACT/365FIXED" | "ACT/365" | "A365F" => Self::Act365Fixed,
            "30/360" | "30/360BONDBASIS" | "BONDBASIS" => Self::Thirty360(Thirty360::BondBasis),
            "30/360US" | "30U/360" | "30/360SIA" => Self::Thirty360(Thirty360::Us),
            "30E/360" | "30/360EUROPEAN" | "EUROBONDBASIS" => Self::Thirty360(Thirty360::European),
            "ACT/ACTISDA" | "ACT/ACT" | "ACT/ACTHISTORICAL" => Self::ActActIsda,
            "ACT/ACTICMA" | "ACT/ACTISMA" | "ACT/ACTBOND" => Self::ActActIcma,
            _ => return Err(DayCountError::Unknown(s.to_owned())),
        })
    }
}

fn is_last_of_february(d: Date) -> bool {
    d.month() == 2 && d.is_end_of_month()
}

fn thirty360_days(conv: Thirty360, d1: Date, d2: Date) -> i32 {
    let (y1, m1, mut dd1) = d1.ymd();
    let (y2, m2, mut dd2) = d2.ymd();
    match conv {
        Thirty360::BondBasis => {
            if dd1 == 31 {
                dd1 = 30;
            }
            if dd2 == 31 && dd1 == 30 {
                dd2 = 30;
            }
        }
        Thirty360::Us => {
            if is_last_of_february(d1) {
                if is_last_of_february(d2) {
                    dd2 = 30;
                }
                dd1 = 30;
            }
            if dd2 == 31 && dd1 >= 30 {
                dd2 = 30;
            }
            if dd1 == 31 {
                dd1 = 30;
            }
        }
        Thirty360::European => {
            dd1 = dd1.min(30);
            dd2 = dd2.min(30);
        }
    }
    360 * (y2 - y1) + 30 * (m2 as i32 - m1 as i32) + (dd2 as i32 - dd1 as i32)
}

fn days_in_year(y: i32) -> f64 {
    if is_leap_year(y) { 366.0 } else { 365.0 }
}

fn act_act_isda(d1: Date, d2: Date) -> Result<f64, DayCountError> {
    if d1 == d2 {
        return Ok(0.0);
    }
    if d1 > d2 {
        return Ok(-act_act_isda(d2, d1)?);
    }
    let (y1, y2) = (d1.year(), d2.year());
    if y1 == y2 {
        return Ok(f64::from(d2 - d1) / days_in_year(y1));
    }
    let next_jan1 = Date::from_ymd(y1 + 1, 1, 1)?;
    let last_jan1 = Date::from_ymd(y2, 1, 1)?;
    Ok(f64::from(next_jan1 - d1) / days_in_year(y1)
        + f64::from(y2 - y1 - 1)
        + f64::from(d2 - last_jan1) / days_in_year(y2))
}

fn act_act_icma(d1: Date, d2: Date, rs: Date, re: Date) -> Result<f64, DayCountError> {
    if d1 == d2 {
        return Ok(0.0);
    }
    if d1 > d2 {
        return Ok(-act_act_icma(d2, d1, rs, re)?);
    }
    let invalid = || DayCountError::InvalidReferencePeriod { d1, d2, start: rs, end: re };
    if !(re > rs && re > d1) {
        return Err(invalid());
    }

    let (mut rs, mut re) = (rs, re);
    // Approximate period length in months (6 for semiannual, 12 for annual, ...).
    let mut months = (12.0 * f64::from(re - rs) / 365.0).round() as i32;
    if months == 0 {
        rs = d1;
        re = d1.add_years(1)?;
        months = 12;
    }
    let period = f64::from(months) / 12.0;

    if d2 <= re {
        if d1 >= rs {
            // Regular case: rs <= d1 <= d2 <= re.
            return Ok(period * f64::from(d2 - d1) / f64::from(re - rs));
        }
        // Long first coupon: d1 < rs; roll back to the previous notional date.
        let prev = rs.add_months(-months)?;
        if d2 > rs {
            return Ok(act_act_icma(d1, rs, prev, rs)? + act_act_icma(rs, d2, rs, re)?);
        }
        return act_act_icma(d1, d2, prev, rs);
    }

    // Long last coupon: rs <= d1 < re < d2; walk forward in notional periods.
    if rs > d1 {
        return Err(invalid());
    }
    let mut sum = act_act_icma(d1, re, rs, re)?;
    let mut i = 0;
    loop {
        let start = re.add_months(months * i)?;
        let end = re.add_months(months * (i + 1))?;
        if d2 < end {
            return Ok(sum + act_act_icma(start, d2, start, end)?);
        }
        sum += period;
        i += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_abs_diff_eq;

    type R = Result<(), DayCountError>;
    const EPS: f64 = 1e-12;

    fn d(y: i32, m: u32, dd: u32) -> Date {
        Date::from_ymd(y, m, dd).expect("valid test date")
    }

    fn icma(d1: Date, d2: Date, rs: Date, re: Date) -> Result<f64, DayCountError> {
        DayCount::ActActIcma.year_fraction(d1, d2, Some(RefPeriod::new(rs, re)))
    }

    #[test]
    fn act_360_and_365f() -> R {
        let (a, b) = (d(2026, 1, 1), d(2026, 7, 1)); // 181 days
        assert_abs_diff_eq!(DayCount::Act360.year_fraction(a, b, None)?, 181.0 / 360.0, epsilon = EPS);
        assert_abs_diff_eq!(DayCount::Act365Fixed.year_fraction(a, b, None)?, 181.0 / 365.0, epsilon = EPS);
        Ok(())
    }

    #[test]
    fn thirty_360_variants() {
        let bb = DayCount::Thirty360(Thirty360::BondBasis);
        let us = DayCount::Thirty360(Thirty360::Us);
        let eu = DayCount::Thirty360(Thirty360::European);

        assert_eq!(bb.days(d(2026, 1, 31), d(2026, 3, 31)), 60);
        assert_eq!(eu.days(d(2026, 1, 31), d(2026, 3, 31)), 60);
        // D2 = 31 with D1 < 30: bond basis keeps 31, 30E/360 caps it.
        assert_eq!(bb.days(d(2026, 1, 15), d(2026, 3, 31)), 76);
        assert_eq!(eu.days(d(2026, 1, 15), d(2026, 3, 31)), 75);
        // End-of-February rule only in 30/360 US.
        assert_eq!(us.days(d(2026, 2, 28), d(2026, 8, 31)), 180);
        assert_eq!(bb.days(d(2026, 2, 28), d(2026, 8, 31)), 183);
    }

    // Examples from the ISDA "ACT/ACT" memo (1999).
    #[test]
    fn isda_memo_regular_semiannual() -> R {
        let (a, b) = (d(2003, 11, 1), d(2004, 5, 1));
        assert_abs_diff_eq!(DayCount::ActActIsda.year_fraction(a, b, None)?, 0.497_724_380_567, epsilon = 1e-12);
        assert_abs_diff_eq!(icma(a, b, a, b)?, 0.5, epsilon = EPS);
        Ok(())
    }

    #[test]
    fn isda_memo_short_first_annual() -> R {
        let yf = icma(d(1999, 2, 1), d(1999, 7, 1), d(1998, 7, 1), d(1999, 7, 1))?;
        assert_abs_diff_eq!(yf, 0.410_958_904_110, epsilon = 1e-12);
        Ok(())
    }

    #[test]
    fn isda_memo_long_first_semiannual() -> R {
        let (a, b) = (d(2002, 8, 15), d(2003, 7, 15));
        assert_abs_diff_eq!(icma(a, b, d(2003, 1, 15), b)?, 0.915_760_869_565, epsilon = 1e-12);
        assert_abs_diff_eq!(DayCount::ActActIsda.year_fraction(a, b, None)?, 0.915_068_493_151, epsilon = 1e-12);
        Ok(())
    }

    #[test]
    fn icma_spans_multiple_periods() -> R {
        let yf = icma(d(2003, 11, 1), d(2004, 11, 1), d(2003, 11, 1), d(2004, 5, 1))?;
        assert_abs_diff_eq!(yf, 1.0, epsilon = EPS);
        Ok(())
    }

    #[test]
    fn treasury_accrued_fraction() -> R {
        // Feb/Aug-15 note settling 2026-09-22: 38 of 184 days into the period.
        let yf = icma(d(2026, 8, 15), d(2026, 9, 22), d(2026, 8, 15), d(2027, 2, 15))?;
        assert_abs_diff_eq!(yf, 0.5 * 38.0 / 184.0, epsilon = EPS);
        Ok(())
    }

    #[test]
    fn reversed_dates_negate() -> R {
        let (a, b) = (d(2003, 11, 1), d(2004, 5, 1));
        let dc = DayCount::ActActIsda;
        assert_abs_diff_eq!(dc.year_fraction(b, a, None)?, -dc.year_fraction(a, b, None)?, epsilon = EPS);
        Ok(())
    }

    #[test]
    fn bad_reference_period_rejected() {
        let r = icma(d(2026, 9, 1), d(2026, 10, 1), d(2026, 8, 15), d(2026, 8, 15));
        assert!(matches!(r, Err(DayCountError::InvalidReferencePeriod { .. })));
    }

    #[test]
    fn parse_names() -> R {
        assert_eq!("act/360".parse::<DayCount>()?, DayCount::Act360);
        assert_eq!("Actual/365 Fixed".parse::<DayCount>()?, DayCount::Act365Fixed);
        assert_eq!("30E/360".parse::<DayCount>()?, DayCount::Thirty360(Thirty360::European));
        assert_eq!("ACT/ACT ISMA".parse::<DayCount>()?, DayCount::ActActIcma);
        for dc in [DayCount::Act360, DayCount::ActActIcma, DayCount::Thirty360(Thirty360::Us)] {
            assert_eq!(dc.to_string().parse::<DayCount>()?, dc);
        }
        assert!("ACT/999".parse::<DayCount>().is_err());
        Ok(())
    }
}
