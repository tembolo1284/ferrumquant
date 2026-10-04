//! CBOT/CME US Treasury futures: contracts, delivery months, deliverable
//! eligibility, and the exchange conversion factor.
//!
//! The conversion factor is CME's published formula: the price of $1 par at
//! a 6% yield, with remaining term measured in whole months from the first
//! day of the delivery month, rounded down to quarters (ZN, TN, TWE, ZB, UB)
//! or months (ZT, Z3N, ZF), coupon rounded to the nearest 1/8 (ties up), and
//! the result rounded to four decimals.
//!
//! Eligibility windows encode the CME rulebook as of 2026; verify against the
//! current contract specs before relying on basket construction.

use std::fmt;
use std::str::FromStr;

use thiserror::Error;

use fq_core::time::{Date, DateError, days_in_month};

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ContractError {
    #[error(transparent)]
    Date(#[from] DateError),
    #[error("delivery month must be Mar/Jun/Sep/Dec, got {year:04}-{month:02}")]
    InvalidDeliveryMonth { year: i32, month: u32 },
    #[error("unknown Treasury futures contract '{0}'")]
    UnknownContract(String),
}

/// How remaining term is rounded for the conversion factor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TermRounding {
    Month,
    Quarter,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TreasuryContract {
    /// ZT (TU)
    TwoYear,
    /// Z3N
    ThreeYear,
    /// ZF (FV)
    FiveYear,
    /// ZN (TY)
    TenYear,
    /// TN
    UltraTenYear,
    /// TWE
    TwentyYear,
    /// ZB (US)
    Bond,
    /// UB
    UltraBond,
}

/// Deliverable window in whole months. `max_from_last_day` measures the
/// maximum from the last day of the delivery month (ZT, Z3N).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Eligibility {
    pub min_remaining_months: i32,
    pub max_remaining_months: Option<i32>,
    pub max_from_last_day: bool,
    pub max_original_months: Option<i32>,
}

impl TreasuryContract {
    pub const fn code(self) -> &'static str {
        match self {
            Self::TwoYear => "ZT",
            Self::ThreeYear => "Z3N",
            Self::FiveYear => "ZF",
            Self::TenYear => "ZN",
            Self::UltraTenYear => "TN",
            Self::TwentyYear => "TWE",
            Self::Bond => "ZB",
            Self::UltraBond => "UB",
        }
    }

    pub const fn rounding(self) -> TermRounding {
        match self {
            Self::TwoYear | Self::ThreeYear | Self::FiveYear => TermRounding::Month,
            _ => TermRounding::Quarter,
        }
    }

    /// Short-end contracts deliver until the 3rd business day of the next month.
    pub const fn is_short_end(self) -> bool {
        matches!(self, Self::TwoYear | Self::ThreeYear | Self::FiveYear)
    }

    pub const fn eligibility(self) -> Eligibility {
        const fn e(min: i32, max: Option<i32>, from_last: bool, orig: Option<i32>) -> Eligibility {
            Eligibility {
                min_remaining_months: min,
                max_remaining_months: max,
                max_from_last_day: from_last,
                max_original_months: orig,
            }
        }
        match self {
            Self::TwoYear => e(21, Some(24), true, Some(63)),
            Self::ThreeYear => e(33, Some(36), true, Some(63)),
            Self::FiveYear => e(50, None, false, Some(63)),
            Self::TenYear => e(78, Some(120), false, Some(120)),
            Self::UltraTenYear => e(113, Some(120), false, Some(120)),
            Self::TwentyYear => e(230, Some(239), false, None),
            Self::Bond => e(180, Some(299), false, None),
            Self::UltraBond => e(300, None, false, None),
        }
    }

    /// Whether a bullet Treasury is deliverable into this contract/month.
    pub fn is_deliverable(self, dated_date: Date, maturity: Date, delivery: DeliveryMonth) -> Result<bool, ContractError> {
        let rule = self.eligibility();
        let first = delivery.first_day()?;
        let remaining = whole_months_between(first, maturity);
        if remaining < rule.min_remaining_months {
            return Ok(false);
        }
        if let Some(max) = rule.max_remaining_months {
            let from = if rule.max_from_last_day { delivery.last_day()? } else { first };
            if whole_months_between(from, maturity) > max {
                return Ok(false);
            }
        }
        if let Some(max_orig) = rule.max_original_months {
            if whole_months_between(dated_date, maturity) > max_orig {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

impl fmt::Display for TreasuryContract {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

impl FromStr for TreasuryContract {
    type Err = ContractError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s.trim().to_ascii_uppercase().as_str() {
            "ZT" | "TU" | "2Y" => Self::TwoYear,
            "Z3N" | "3Y" => Self::ThreeYear,
            "ZF" | "FV" | "5Y" => Self::FiveYear,
            "ZN" | "TY" | "10Y" => Self::TenYear,
            "TN" | "UXY" | "ULTRA10" => Self::UltraTenYear,
            "TWE" | "20Y" => Self::TwentyYear,
            "ZB" | "US" | "30Y" => Self::Bond,
            "UB" | "WN" | "ULTRA" => Self::UltraBond,
            _ => return Err(ContractError::UnknownContract(s.to_owned())),
        })
    }
}

/// A quarterly delivery month (March, June, September, December).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DeliveryMonth {
    year: i32,
    month: u32,
}

impl DeliveryMonth {
    pub fn new(year: i32, month: u32) -> Result<Self, ContractError> {
        if !matches!(month, 3 | 6 | 9 | 12) {
            return Err(ContractError::InvalidDeliveryMonth { year, month });
        }
        Ok(Self { year, month })
    }

    pub const fn year(self) -> i32 {
        self.year
    }

    pub const fn month(self) -> u32 {
        self.month
    }

    /// CME month code: H, M, U, Z.
    pub const fn month_code(self) -> char {
        match self.month {
            3 => 'H',
            6 => 'M',
            9 => 'U',
            _ => 'Z',
        }
    }

    pub fn first_day(self) -> Result<Date, DateError> {
        Date::from_ymd(self.year, self.month, 1)
    }

    pub fn last_day(self) -> Result<Date, DateError> {
        Date::from_ymd(self.year, self.month, days_in_month(self.year, self.month))
    }
}

impl fmt::Display for DeliveryMonth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{:02}", self.month_code(), self.year.rem_euclid(100))
    }
}

/// Complete months from `from` to `to` (0 if `to` precedes `from`).
pub fn whole_months_between(from: Date, to: Date) -> i32 {
    let (y1, m1, d1) = from.ymd();
    let (y2, m2, d2) = to.ymd();
    let months = (y2 - y1) * 12 + (m2 as i32 - m1 as i32) - i32::from(d2 < d1);
    months.max(0)
}

/// Coupon rounded to the nearest 1/8 of 1%, ties rounded up.
pub fn round_coupon_to_eighth(coupon: f64) -> f64 {
    (coupon * 800.0 + 0.5 + 1e-9).floor() / 800.0
}

/// CME conversion factor, rounded to four decimals.
pub fn conversion_factor(contract: TreasuryContract, coupon: f64, maturity: Date, delivery: DeliveryMonth) -> Result<f64, ContractError> {
    let months = whole_months_between(delivery.first_day()?, maturity);
    let n = months / 12;
    let mut z = months % 12;
    let quarterly = contract.rounding() == TermRounding::Quarter;
    if quarterly {
        z -= z % 3;
    }
    let cpn = round_coupon_to_eighth(coupon);

    let v = if z < 7 { z } else if quarterly { 3 } else { z - 6 };
    let a = 1.03_f64.powf(-f64::from(v) / 6.0);
    let b = (cpn / 2.0) * f64::from(6 - v) / 6.0;
    let c = if z < 7 { 1.03_f64.powi(-2 * n) } else { 1.03_f64.powi(-(2 * n + 1)) };
    let d = (cpn / 0.06) * (1.0 - c);
    let factor = a * (cpn / 2.0 + c + d) - b;
    Ok((factor * 1e4).round() / 1e4)
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_abs_diff_eq;

    type R = Result<(), ContractError>;

    fn d(y: i32, m: u32, dd: u32) -> Date {
        Date::from_ymd(y, m, dd).expect("valid test date")
    }

    fn cf(c: TreasuryContract, cpn: f64, mat: Date, y: i32, m: u32) -> Result<f64, ContractError> {
        conversion_factor(c, cpn, mat, DeliveryMonth::new(y, m)?)
    }

    // Published CME factors.
    #[test]
    fn cme_published_factors() -> R {
        use TreasuryContract::TenYear;
        assert_abs_diff_eq!(cf(TenYear, 0.02375, d(2024, 8, 15), 2017, 12)?, 0.8072, epsilon = 1e-12);
        assert_abs_diff_eq!(cf(TenYear, 0.01875, d(2024, 8, 15), 2017, 12)?, 0.7807, epsilon = 1e-12);
        assert_abs_diff_eq!(cf(TenYear, 0.05125, d(2016, 5, 15), 2009, 3)?, 0.9506, epsilon = 1e-12);
        Ok(())
    }

    // Month-rounding branch with z >= 7 (5s of 9/30/2025 into ZT Dec 2023),
    // hand-computed from the CME formula.
    #[test]
    fn two_year_month_rounding() -> R {
        let f = cf(TreasuryContract::TwoYear, 0.05, d(2025, 9, 30), 2023, 12)?;
        assert_abs_diff_eq!(f, 0.9835, epsilon = 1e-12);
        Ok(())
    }

    #[test]
    fn six_percent_on_a_coupon_boundary_is_par() -> R {
        let f = cf(TreasuryContract::TenYear, 0.06, d(2036, 12, 15), 2026, 12)?;
        assert_abs_diff_eq!(f, 1.0, epsilon = 1e-12);
        Ok(())
    }

    #[test]
    fn coupon_rounding_ties_up() {
        assert_abs_diff_eq!(round_coupon_to_eighth(0.040625), 0.04125, epsilon = 1e-15);
        assert_abs_diff_eq!(round_coupon_to_eighth(0.0425), 0.0425, epsilon = 1e-15);
    }

    #[test]
    fn whole_months() {
        assert_eq!(whole_months_between(d(2017, 12, 1), d(2024, 8, 15)), 80);
        assert_eq!(whole_months_between(d(2027, 3, 31), d(2029, 3, 30)), 23);
        assert_eq!(whole_months_between(d(2027, 3, 1), d(2026, 1, 1)), 0);
    }

    #[test]
    fn deliverable_windows() -> R {
        let h27 = DeliveryMonth::new(2027, 3)?;
        use TreasuryContract as C;
        // New 10Y note: ZN yes, ZF no (original term too long).
        assert!(C::TenYear.is_deliverable(d(2026, 8, 15), d(2036, 8, 15), h27)?);
        assert!(!C::FiveYear.is_deliverable(d(2026, 8, 15), d(2036, 8, 15), h27)?);
        // Old 10Y with under 6.5 years left drops out of ZN.
        assert!(!C::TenYear.is_deliverable(d(2023, 5, 15), d(2033, 5, 15), h27)?);
        // New 30Y bond: UB yes, ZB no.
        assert!(C::UltraBond.is_deliverable(d(2026, 8, 15), d(2056, 8, 15), h27)?);
        assert!(!C::Bond.is_deliverable(d(2026, 8, 15), d(2056, 8, 15), h27)?);
        Ok(())
    }

    #[test]
    fn delivery_month_and_parsing() -> R {
        assert!(DeliveryMonth::new(2027, 2).is_err());
        assert_eq!(DeliveryMonth::new(2026, 12)?.to_string(), "Z26");
        assert_eq!("TY".parse::<TreasuryContract>()?, TreasuryContract::TenYear);
        assert_eq!("ub".parse::<TreasuryContract>()?, TreasuryContract::UltraBond);
        Ok(())
    }
}
