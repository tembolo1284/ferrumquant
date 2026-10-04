//! Calendar dates as serial day numbers.
//!
//! `Date` stores an Excel-compatible serial number (days since 1899-12-30),
//! matching QuantLib and Excel for every date from 1900-03-01 onward.
//! The supported range is 1901-01-01 ..= 2199-12-31, as in QuantLib.

use std::fmt;
use std::ops::{Add, AddAssign, Sub, SubAssign};
use std::str::FromStr;

use thiserror::Error;

/// Excel serial number of 1970-01-01.
const UNIX_EPOCH_SERIAL: i32 = 25_569;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum DateError {
    #[error("invalid date {year:04}-{month:02}-{day:02}")]
    Invalid { year: i32, month: u32, day: u32 },
    #[error("date outside supported range 1901-01-01..=2199-12-31")]
    OutOfRange,
    #[error("no occurrence {n} of {weekday} in {year:04}-{month:02}")]
    NoSuchWeekday { n: u32, weekday: Weekday, year: i32, month: u32 },
    #[error("cannot parse date '{0}', expected YYYY-MM-DD")]
    Parse(String),
}

/// ISO weekday (Monday = 1 ... Sunday = 7). Weekend rules belong to calendars.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Weekday {
    Monday = 1,
    Tuesday,
    Wednesday,
    Thursday,
    Friday,
    Saturday,
    Sunday,
}

impl Weekday {
    pub const fn from_iso(n: u32) -> Option<Self> {
        match n {
            1 => Some(Self::Monday),
            2 => Some(Self::Tuesday),
            3 => Some(Self::Wednesday),
            4 => Some(Self::Thursday),
            5 => Some(Self::Friday),
            6 => Some(Self::Saturday),
            7 => Some(Self::Sunday),
            _ => None,
        }
    }

    pub const fn iso(self) -> u32 {
        self as u32
    }
}

impl fmt::Display for Weekday {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}

pub const fn is_leap_year(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

pub const fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        2 => 28,
        _ => 0,
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Date(i32);

impl Date {
    /// 1901-01-01
    pub const MIN: Date = Date(367);
    /// 2199-12-31
    pub const MAX: Date = Date(109_574);

    pub fn from_ymd(year: i32, month: u32, day: u32) -> Result<Self, DateError> {
        if !(1..=12).contains(&month) || day == 0 || day > days_in_month(year, month) {
            return Err(DateError::Invalid { year, month, day });
        }
        Self::from_serial(days_from_civil(year, month, day) + UNIX_EPOCH_SERIAL)
    }

    pub fn from_serial(serial: i32) -> Result<Self, DateError> {
        if (Self::MIN.0..=Self::MAX.0).contains(&serial) {
            Ok(Self(serial))
        } else {
            Err(DateError::OutOfRange)
        }
    }

    pub const fn serial(self) -> i32 {
        self.0
    }

    pub fn ymd(self) -> (i32, u32, u32) {
        civil_from_days(self.0 - UNIX_EPOCH_SERIAL)
    }

    pub fn year(self) -> i32 {
        self.ymd().0
    }

    pub fn month(self) -> u32 {
        self.ymd().1
    }

    pub fn day(self) -> u32 {
        self.ymd().2
    }

    pub fn weekday(self) -> Weekday {
        // 1970-01-01 was a Thursday (ISO 4).
        match (self.0 - UNIX_EPOCH_SERIAL + 3).rem_euclid(7) + 1 {
            1 => Weekday::Monday,
            2 => Weekday::Tuesday,
            3 => Weekday::Wednesday,
            4 => Weekday::Thursday,
            5 => Weekday::Friday,
            6 => Weekday::Saturday,
            _ => Weekday::Sunday,
        }
    }

    pub fn day_of_year(self) -> u32 {
        let (y, _, _) = self.ymd();
        (self.0 - (days_from_civil(y, 1, 1) + UNIX_EPOCH_SERIAL) + 1) as u32
    }

    pub fn start_of_month(self) -> Self {
        Self(self.0 - (self.day() as i32 - 1))
    }

    pub fn end_of_month(self) -> Self {
        let (y, m, d) = self.ymd();
        Self(self.0 + (days_in_month(y, m) - d) as i32)
    }

    pub fn is_end_of_month(self) -> bool {
        let (y, m, d) = self.ymd();
        d == days_in_month(y, m)
    }

    pub fn checked_add_days(self, n: i32) -> Result<Self, DateError> {
        Self::from_serial(self.0.checked_add(n).ok_or(DateError::OutOfRange)?)
    }

    /// Calendar-month shift, clamping the day to the target month's length.
    /// No end-of-month rule is applied here; schedules handle that.
    pub fn add_months(self, n: i32) -> Result<Self, DateError> {
        let (y, m, d) = self.ymd();
        let total = y * 12 + (m as i32 - 1) + n;
        let ny = total.div_euclid(12);
        let nm = total.rem_euclid(12) as u32 + 1;
        Self::from_ymd(ny, nm, d.min(days_in_month(ny, nm)))
    }

    pub fn add_years(self, n: i32) -> Result<Self, DateError> {
        self.add_months(n * 12)
    }

    /// The `n`-th (1-based) given weekday of a month, e.g. MLK Day = 3rd Monday of January.
    pub fn nth_weekday(n: u32, weekday: Weekday, year: i32, month: u32) -> Result<Self, DateError> {
        let first = Self::from_ymd(year, month, 1)?;
        let offset = (weekday.iso() as i32 - first.weekday().iso() as i32).rem_euclid(7) as u32;
        let day = 1 + offset + 7 * n.saturating_sub(1);
        if n == 0 || day > days_in_month(year, month) {
            return Err(DateError::NoSuchWeekday { n, weekday, year, month });
        }
        Self::from_ymd(year, month, day)
    }

    /// The last given weekday of a month, e.g. Memorial Day = last Monday of May.
    pub fn last_weekday(weekday: Weekday, year: i32, month: u32) -> Result<Self, DateError> {
        let eom = Self::from_ymd(year, month, days_in_month(year, month))?;
        let back = (eom.weekday().iso() as i32 - weekday.iso() as i32).rem_euclid(7);
        eom.checked_add_days(-back)
    }
}

/// Panics if the result leaves the supported range; use `checked_add_days` otherwise.
impl Add<i32> for Date {
    type Output = Date;
    fn add(self, rhs: i32) -> Date {
        self.checked_add_days(rhs).unwrap_or_else(|e| panic!("{e}"))
    }
}

impl Sub<i32> for Date {
    type Output = Date;
    fn sub(self, rhs: i32) -> Date {
        self + (-rhs)
    }
}

impl AddAssign<i32> for Date {
    fn add_assign(&mut self, rhs: i32) {
        *self = *self + rhs;
    }
}

impl SubAssign<i32> for Date {
    fn sub_assign(&mut self, rhs: i32) {
        *self = *self - rhs;
    }
}

/// Actual days between two dates.
impl Sub<Date> for Date {
    type Output = i32;
    fn sub(self, rhs: Date) -> i32 {
        self.0 - rhs.0
    }
}

impl fmt::Display for Date {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (y, m, d) = self.ymd();
        write!(f, "{y:04}-{m:02}-{d:02}")
    }
}

impl fmt::Debug for Date {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Date({self})")
    }
}

impl FromStr for Date {
    type Err = DateError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let err = || DateError::Parse(s.to_owned());
        let mut parts = s.trim().splitn(3, '-');
        let y = parts.next().and_then(|p| p.parse().ok()).ok_or_else(err)?;
        let m = parts.next().and_then(|p| p.parse().ok()).ok_or_else(err)?;
        let d = parts.next().and_then(|p| p.parse().ok()).ok_or_else(err)?;
        Self::from_ymd(y, m, d)
    }
}

// Howard Hinnant's civil-date algorithms; day 0 = 1970-01-01.
fn days_from_civil(y: i32, m: u32, d: u32) -> i32 {
    let (m, d) = (m as i32, d as i32);
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12; // March = 0 ... February = 11
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(z: i32) -> (i32, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i32::from(m <= 2);
    (y, m as u32, d as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    type R = Result<(), DateError>;

    #[test]
    fn excel_serial_anchors() -> R {
        assert_eq!(Date::from_ymd(1970, 1, 1)?.serial(), 25_569);
        assert_eq!(Date::from_ymd(2000, 1, 1)?.serial(), 36_526);
        assert_eq!(Date::MIN.ymd(), (1901, 1, 1));
        assert_eq!(Date::MAX.ymd(), (2199, 12, 31));
        Ok(())
    }

    #[test]
    fn round_trips_full_range() -> R {
        for s in Date::MIN.serial()..=Date::MAX.serial() {
            let d = Date::from_serial(s)?;
            let (y, m, dd) = d.ymd();
            assert_eq!(Date::from_ymd(y, m, dd)?, d);
        }
        Ok(())
    }

    #[test]
    fn weekdays() -> R {
        assert_eq!(Date::from_ymd(2000, 1, 1)?.weekday(), Weekday::Saturday);
        assert_eq!(Date::from_ymd(2026, 9, 22)?.weekday(), Weekday::Tuesday);
        Ok(())
    }

    #[test]
    fn leap_years() {
        assert!(is_leap_year(2000) && is_leap_year(2024));
        assert!(!is_leap_year(1900) && !is_leap_year(2100) && !is_leap_year(2023));
    }

    #[test]
    fn invalid_dates_rejected() {
        assert!(Date::from_ymd(2023, 2, 29).is_err());
        assert!(Date::from_ymd(2024, 13, 1).is_err());
        assert_eq!(Date::from_ymd(1900, 6, 1), Err(DateError::OutOfRange));
    }

    #[test]
    fn month_arithmetic_clamps() -> R {
        let d = |y, m, dd| Date::from_ymd(y, m, dd);
        assert_eq!(d(2024, 1, 31)?.add_months(1)?, d(2024, 2, 29)?);
        assert_eq!(d(2023, 1, 31)?.add_months(1)?, d(2023, 2, 28)?);
        assert_eq!(d(2024, 3, 31)?.add_months(-1)?, d(2024, 2, 29)?);
        assert_eq!(d(2024, 2, 29)?.add_years(1)?, d(2025, 2, 28)?);
        assert_eq!(d(2026, 11, 15)?.add_months(3)?, d(2027, 2, 15)?);
        Ok(())
    }

    #[test]
    fn us_holiday_rules() -> R {
        let d = |y, m, dd| Date::from_ymd(y, m, dd);
        assert_eq!(Date::nth_weekday(3, Weekday::Monday, 2026, 1)?, d(2026, 1, 19)?); // MLK
        assert_eq!(Date::last_weekday(Weekday::Monday, 2026, 5)?, d(2026, 5, 25)?); // Memorial
        assert_eq!(Date::nth_weekday(4, Weekday::Thursday, 2026, 11)?, d(2026, 11, 26)?); // Thanksgiving
        assert!(Date::nth_weekday(5, Weekday::Monday, 2026, 2).is_err());
        Ok(())
    }

    #[test]
    fn month_boundaries() -> R {
        let d = Date::from_ymd(2024, 2, 10)?;
        assert_eq!(d.start_of_month(), Date::from_ymd(2024, 2, 1)?);
        assert_eq!(d.end_of_month(), Date::from_ymd(2024, 2, 29)?);
        assert!(d.end_of_month().is_end_of_month());
        assert_eq!(Date::from_ymd(2024, 12, 31)?.day_of_year(), 366);
        Ok(())
    }

    #[test]
    fn arithmetic_and_parsing() -> R {
        let a: Date = "2026-01-01".parse()?;
        let b: Date = "2026-12-31".parse()?;
        assert_eq!(b - a, 364);
        assert_eq!(a + 364, b);
        assert_eq!(b.to_string(), "2026-12-31");
        assert!("2026/01/01".parse::<Date>().is_err());
        Ok(())
    }
}
