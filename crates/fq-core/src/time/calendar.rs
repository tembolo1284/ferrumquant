//! Business-day calendars and adjustment conventions.
//!
//! A `Calendar` is a set of `Market`s joined by a `JoinRule`, so joint
//! calendars (e.g. "USGS+USNY" for a SOFR swap) are plain values that clone
//! cheaply and parse from strings for the Python/Excel layers.
//!
//! US rules follow QuantLib's `UnitedStates::Settlement` and
//! `UnitedStates::GovernmentBond`. For government-bond settlement, Good Friday
//! is always a holiday: even in years SIFMA recommends only an early close
//! (payroll release days, e.g. 2015, 2021, 2023, 2026), it recommends that the
//! day not be treated as a good settlement day.

use std::fmt;
use std::str::FromStr;

use thiserror::Error;

use super::date::{Date, DateError, Weekday};

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum CalendarError {
    #[error(transparent)]
    Date(#[from] DateError),
    #[error("unknown calendar '{0}'")]
    UnknownCalendar(String),
    #[error("unknown business day convention '{0}'")]
    UnknownConvention(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BusinessDayConvention {
    Following,
    ModifiedFollowing,
    Preceding,
    ModifiedPreceding,
    Unadjusted,
}

impl BusinessDayConvention {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Following => "Following",
            Self::ModifiedFollowing => "ModifiedFollowing",
            Self::Preceding => "Preceding",
            Self::ModifiedPreceding => "ModifiedPreceding",
            Self::Unadjusted => "Unadjusted",
        }
    }
}

impl fmt::Display for BusinessDayConvention {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl FromStr for BusinessDayConvention {
    type Err = CalendarError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let key: String = s
            .to_ascii_uppercase()
            .chars()
            .filter(|c| !matches!(c, ' ' | '-' | '_'))
            .collect();
        Ok(match key.as_str() {
            "F" | "FOLLOWING" => Self::Following,
            "MF" | "MODFOLLOWING" | "MODIFIEDFOLLOWING" => Self::ModifiedFollowing,
            "P" | "PRECEDING" => Self::Preceding,
            "MP" | "MODPRECEDING" | "MODIFIEDPRECEDING" => Self::ModifiedPreceding,
            "U" | "NONE" | "UNADJUSTED" => Self::Unadjusted,
            _ => return Err(CalendarError::UnknownConvention(s.to_owned())),
        })
    }
}

/// A single holiday-rule set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Market {
    /// Saturdays and Sundays only.
    WeekendsOnly,
    /// US settlement (Federal Reserve / NY banking holidays), code "USNY".
    UsSettlement,
    /// US government securities (SIFMA recommendations), code "USGS".
    UsGovernmentBond,
}

impl Market {
    pub const fn code(self) -> &'static str {
        match self {
            Self::WeekendsOnly => "WEEKENDS",
            Self::UsSettlement => "USNY",
            Self::UsGovernmentBond => "USGS",
        }
    }

    pub fn is_holiday(self, d: Date) -> bool {
        let w = d.weekday();
        if matches!(w, Weekday::Saturday | Weekday::Sunday) {
            return true;
        }
        match self {
            Self::WeekendsOnly => false,
            Self::UsSettlement => us_settlement_holiday(d, w),
            Self::UsGovernmentBond => us_government_bond_holiday(d, w),
        }
    }
}

impl FromStr for Market {
    type Err = CalendarError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s.trim().to_ascii_uppercase().as_str() {
            "WEEKENDS" | "WEEKENDSONLY" => Self::WeekendsOnly,
            "USNY" | "US-SETTLEMENT" | "USSETTLEMENT" | "NYC" => Self::UsSettlement,
            "USGS" | "SIFMA" | "US-GOVBOND" | "USGOVERNMENTBOND" => Self::UsGovernmentBond,
            _ => return Err(CalendarError::UnknownCalendar(s.to_owned())),
        })
    }
}

/// How a joint calendar combines its markets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum JoinRule {
    /// Holiday if a holiday in any market (the usual choice for payments).
    #[default]
    Holidays,
    /// Holiday only if a holiday in every market.
    BusinessDays,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Calendar {
    markets: Vec<Market>,
    join: JoinRule,
}

impl From<Market> for Calendar {
    fn from(m: Market) -> Self {
        Self::new(m)
    }
}

impl Calendar {
    pub fn new(market: Market) -> Self {
        Self { markets: vec![market], join: JoinRule::Holidays }
    }

    pub fn joint(markets: impl IntoIterator<Item = Market>, join: JoinRule) -> Self {
        let mut markets: Vec<Market> = markets.into_iter().collect();
        markets.dedup();
        if markets.is_empty() {
            markets.push(Market::WeekendsOnly);
        }
        Self { markets, join }
    }

    pub fn markets(&self) -> &[Market] {
        &self.markets
    }

    pub fn is_holiday(&self, d: Date) -> bool {
        match self.join {
            JoinRule::Holidays => self.markets.iter().any(|m| m.is_holiday(d)),
            JoinRule::BusinessDays => self.markets.iter().all(|m| m.is_holiday(d)),
        }
    }

    pub fn is_business_day(&self, d: Date) -> bool {
        !self.is_holiday(d)
    }

    pub fn adjust(&self, d: Date, bdc: BusinessDayConvention) -> Result<Date, DateError> {
        use BusinessDayConvention as C;
        match bdc {
            C::Unadjusted => Ok(d),
            C::Following | C::ModifiedFollowing => {
                let adj = self.roll(d, 1)?;
                if bdc == C::ModifiedFollowing && adj.month() != d.month() {
                    self.roll(d, -1)
                } else {
                    Ok(adj)
                }
            }
            C::Preceding | C::ModifiedPreceding => {
                let adj = self.roll(d, -1)?;
                if bdc == C::ModifiedPreceding && adj.month() != d.month() {
                    self.roll(d, 1)
                } else {
                    Ok(adj)
                }
            }
        }
    }

    /// Move `n` business days (n = 0 adjusts Following), e.g. T+1 settlement.
    pub fn advance(&self, d: Date, n: i32) -> Result<Date, DateError> {
        if n == 0 {
            return self.adjust(d, BusinessDayConvention::Following);
        }
        let step = n.signum();
        let mut out = d;
        for _ in 0..n.abs() {
            out = self.roll(out.checked_add_days(step)?, step)?;
        }
        Ok(out)
    }

    /// Business days in [from, to) (negative if to < from).
    pub fn business_days_between(&self, from: Date, to: Date) -> i32 {
        let (lo, hi, sign) = if from <= to { (from, to, 1) } else { (to, from, -1) };
        let count = (lo.serial()..hi.serial())
            .filter_map(|s| Date::from_serial(s).ok())
            .filter(|&d| self.is_business_day(d))
            .count() as i32;
        sign * count
    }

    /// Weekday holidays in [from, to].
    pub fn holidays(&self, from: Date, to: Date) -> Vec<Date> {
        (from.serial()..=to.serial())
            .filter_map(|s| Date::from_serial(s).ok())
            .filter(|&d| !matches!(d.weekday(), Weekday::Saturday | Weekday::Sunday))
            .filter(|&d| self.is_holiday(d))
            .collect()
    }

    fn roll(&self, d: Date, step: i32) -> Result<Date, DateError> {
        let mut out = d;
        while self.is_holiday(out) {
            out = out.checked_add_days(step)?;
        }
        Ok(out)
    }
}

impl fmt::Display for Calendar {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sep = match self.join {
            JoinRule::Holidays => "+",
            JoinRule::BusinessDays => "|",
        };
        let codes: Vec<&str> = self.markets.iter().map(|m| m.code()).collect();
        f.write_str(&codes.join(sep))
    }
}

/// "USGS", "USGS+USNY" (holidays join), "USGS|USNY" (business-days join).
impl FromStr for Calendar {
    type Err = CalendarError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (join, sep) = if s.contains('|') { (JoinRule::BusinessDays, '|') } else { (JoinRule::Holidays, '+') };
        let markets = s
            .split([sep, ','])
            .filter(|p| !p.trim().is_empty())
            .map(str::parse)
            .collect::<Result<Vec<Market>, _>>()?;
        if markets.is_empty() {
            return Err(CalendarError::UnknownCalendar(s.to_owned()));
        }
        Ok(Self::joint(markets, join))
    }
}

/// Easter Sunday (anonymous Gregorian algorithm) as (month, day).
pub fn easter_sunday(year: i32) -> (u32, u32) {
    let a = year % 19;
    let (b, c) = (year / 100, year % 100);
    let (d, e) = (b / 4, b % 4);
    let f = (b + 8) / 25;
    let g = (b - f + 1) / 3;
    let h = (19 * a + b - d - g + 15) % 30;
    let (i, k) = (c / 4, c % 4);
    let l = (32 + 2 * e + 2 * i - h - k) % 7;
    let m = (a + 11 * h + 22 * l) / 451;
    let n = h + l - 7 * m + 114;
    ((n / 31) as u32, (n % 31 + 1) as u32)
}

// ---- US holiday rules (weekday dates only; weekends are handled by the caller) ----

/// Fixed-date holiday observed Monday if Sunday, Friday if Saturday.
fn observed(day: u32, dd: u32, w: Weekday) -> bool {
    dd == day || (dd == day + 1 && w == Weekday::Monday) || (dd + 1 == day && w == Weekday::Friday)
}

fn is_mlk(y: i32, m: u32, dd: u32, w: Weekday) -> bool {
    y >= 1983 && m == 1 && w == Weekday::Monday && (15..=21).contains(&dd)
}

fn is_washington(y: i32, m: u32, dd: u32, w: Weekday) -> bool {
    if y >= 1971 { m == 2 && w == Weekday::Monday && (15..=21).contains(&dd) } else { m == 2 && dd == 22 }
}

fn is_memorial(y: i32, m: u32, dd: u32, w: Weekday) -> bool {
    if y >= 1971 { m == 5 && w == Weekday::Monday && dd >= 25 } else { m == 5 && dd == 30 }
}

fn is_juneteenth(y: i32, m: u32, dd: u32, w: Weekday) -> bool {
    y >= 2022 && m == 6 && observed(19, dd, w)
}

fn is_independence(m: u32, dd: u32, w: Weekday) -> bool {
    m == 7 && observed(4, dd, w)
}

fn is_labor(m: u32, dd: u32, w: Weekday) -> bool {
    m == 9 && w == Weekday::Monday && dd <= 7
}

fn is_columbus(y: i32, m: u32, dd: u32, w: Weekday) -> bool {
    y >= 1971 && m == 10 && w == Weekday::Monday && (8..=14).contains(&dd)
}

/// Veterans Day; 1971-1977 it was the 4th Monday of October.
fn is_veterans(y: i32, m: u32, dd: u32, w: Weekday, saturday_to_friday: bool) -> bool {
    if (1971..=1977).contains(&y) {
        return m == 10 && w == Weekday::Monday && (22..=28).contains(&dd);
    }
    m == 11
        && (dd == 11
            || (dd == 12 && w == Weekday::Monday)
            || (saturday_to_friday && dd == 10 && w == Weekday::Friday))
}

fn is_thanksgiving(m: u32, dd: u32, w: Weekday) -> bool {
    m == 11 && w == Weekday::Thursday && (22..=28).contains(&dd)
}

fn is_christmas(m: u32, dd: u32, w: Weekday) -> bool {
    m == 12 && observed(25, dd, w)
}

fn us_settlement_holiday(d: Date, w: Weekday) -> bool {
    let (y, m, dd) = d.ymd();
    (m == 1 && (dd == 1 || (dd == 2 && w == Weekday::Monday)))
        || (m == 12 && dd == 31 && w == Weekday::Friday) // New Year's on Saturday
        || is_mlk(y, m, dd, w)
        || is_washington(y, m, dd, w)
        || is_memorial(y, m, dd, w)
        || is_juneteenth(y, m, dd, w)
        || is_independence(m, dd, w)
        || is_labor(m, dd, w)
        || is_columbus(y, m, dd, w)
        || is_veterans(y, m, dd, w, true)
        || is_thanksgiving(m, dd, w)
        || is_christmas(m, dd, w)
}

fn us_government_bond_holiday(d: Date, w: Weekday) -> bool {
    let (y, m, dd) = d.ymd();
    let (em, ed) = easter_sunday(y);
    let good_friday = Date::from_ymd(y, em, ed).is_ok_and(|easter| easter - d == 2);
    // New Year's on Saturday is not moved to Friday (year-end stays open).
    (m == 1 && (dd == 1 || (dd == 2 && w == Weekday::Monday)))
        || is_mlk(y, m, dd, w)
        || is_washington(y, m, dd, w)
        || good_friday
        || is_memorial(y, m, dd, w)
        || is_juneteenth(y, m, dd, w)
        || is_independence(m, dd, w)
        || is_labor(m, dd, w)
        || is_columbus(y, m, dd, w)
        || is_veterans(y, m, dd, w, false)
        || is_thanksgiving(m, dd, w)
        || is_christmas(m, dd, w)
        // Special closings
        || (y, m, dd) == (2004, 6, 11) // Reagan funeral
        || (y, m, dd) == (2012, 10, 30) // Hurricane Sandy
        || (y, m, dd) == (2018, 12, 5) // G.H.W. Bush funeral
}

#[cfg(test)]
mod tests {
    use super::*;
    use BusinessDayConvention as C;

    type R = Result<(), CalendarError>;

    fn d(y: i32, m: u32, dd: u32) -> Date {
        Date::from_ymd(y, m, dd).expect("valid test date")
    }

    #[test]
    fn easter() {
        assert_eq!(easter_sunday(2024), (3, 31));
        assert_eq!(easter_sunday(2026), (4, 5));
        assert_eq!(easter_sunday(2038), (4, 25));
    }

    #[test]
    fn sifma_2026() {
        let cal = Calendar::new(Market::UsGovernmentBond);
        let expected = vec![
            d(2026, 1, 1), d(2026, 1, 19), d(2026, 2, 16), d(2026, 4, 3),
            d(2026, 5, 25), d(2026, 6, 19), d(2026, 7, 3), d(2026, 9, 7),
            d(2026, 10, 12), d(2026, 11, 11), d(2026, 11, 26), d(2026, 12, 25),
        ];
        assert_eq!(cal.holidays(d(2026, 1, 1), d(2026, 12, 31)), expected);
    }

    #[test]
    fn settlement_2026_has_no_good_friday() {
        let cal = Calendar::new(Market::UsSettlement);
        let hols = cal.holidays(d(2026, 1, 1), d(2026, 12, 31));
        assert_eq!(hols.len(), 11);
        assert!(!hols.contains(&d(2026, 4, 3)));
    }

    #[test]
    fn saturday_holiday_differences() {
        let (gs, ny) = (Calendar::new(Market::UsGovernmentBond), Calendar::new(Market::UsSettlement));
        // New Year's 2022 fell on Saturday.
        assert!(gs.is_business_day(d(2021, 12, 31)));
        assert!(ny.is_holiday(d(2021, 12, 31)));
        // Veterans Day 2028 falls on Saturday.
        assert!(gs.is_business_day(d(2028, 11, 10)));
        assert!(ny.is_holiday(d(2028, 11, 10)));
    }

    #[test]
    fn adjustment_conventions() -> R {
        let cal = Calendar::new(Market::UsGovernmentBond);
        assert_eq!(cal.adjust(d(2026, 4, 3), C::Following)?, d(2026, 4, 6));
        assert_eq!(cal.adjust(d(2026, 1, 31), C::Following)?, d(2026, 2, 2));
        assert_eq!(cal.adjust(d(2026, 1, 31), C::ModifiedFollowing)?, d(2026, 1, 30));
        assert_eq!(cal.adjust(d(2026, 7, 4), C::Preceding)?, d(2026, 7, 2));
        assert_eq!(cal.adjust(d(2026, 11, 1), C::ModifiedPreceding)?, d(2026, 11, 2));
        assert_eq!(cal.adjust(d(2026, 7, 4), C::Unadjusted)?, d(2026, 7, 4));
        Ok(())
    }

    #[test]
    fn advance_business_days() -> R {
        let cal = Calendar::new(Market::UsGovernmentBond);
        assert_eq!(cal.advance(d(2026, 9, 22), 1)?, d(2026, 9, 23));
        assert_eq!(cal.advance(d(2026, 4, 2), 1)?, d(2026, 4, 6)); // T+1 over Good Friday
        assert_eq!(cal.advance(d(2026, 4, 6), -1)?, d(2026, 4, 2));
        assert_eq!(cal.business_days_between(d(2026, 4, 2), d(2026, 4, 7)), 2);
        Ok(())
    }

    #[test]
    fn joint_calendars() -> R {
        let both: Calendar = "USGS+USNY".parse()?;
        assert!(both.is_holiday(d(2026, 4, 3))); // USGS only
        assert!(both.is_holiday(d(2028, 11, 10))); // USNY only
        let either: Calendar = "USGS|USNY".parse()?;
        assert!(either.is_business_day(d(2026, 4, 3)));
        assert_eq!(both.to_string(), "USGS+USNY");
        assert!("XXXX".parse::<Calendar>().is_err());
        Ok(())
    }

    #[test]
    fn parse_conventions() -> R {
        assert_eq!("MF".parse::<C>()?, C::ModifiedFollowing);
        assert_eq!("Modified Following".parse::<C>()?, C::ModifiedFollowing);
        assert_eq!("unadjusted".parse::<C>()?, C::Unadjusted);
        Ok(())
    }
}
