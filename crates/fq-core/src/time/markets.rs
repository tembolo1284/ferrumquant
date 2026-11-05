//! Non-US holiday rules: TARGET (EUR), UK settlement (GBP), Japan (JPY).
//!
//! Each function answers "is this weekday a holiday?"; weekends are handled
//! by the caller. Rules follow QuantLib's `TARGET`, `UnitedKingdom::Settlement`
//! and `Japan` calendars and are diffed against them in the tests of
//! `tools/quantlib_golden.py` (2000-2060).

use super::calendar::easter_sunday;
use super::date::{Date, Weekday};

/// Days from Easter Sunday: -2 is Good Friday, +1 is Easter Monday.
fn days_from_easter(d: Date) -> Option<i32> {
    let (y, _, _) = d.ymd();
    let (em, ed) = easter_sunday(y);
    Date::from_ymd(y, em, ed).ok().map(|easter| d - easter)
}

/// TARGET2 (Eurosystem payments). Code "TARGET".
pub(super) fn target_holiday(d: Date, _w: Weekday) -> bool {
    let (y, m, dd) = d.ymd();
    let easter = days_from_easter(d);
    (m == 1 && dd == 1)
        || (y >= 2000 && matches!(easter, Some(-2) | Some(1)))
        || (y >= 2000 && m == 5 && dd == 1)
        || (m == 12 && dd == 25)
        || (y >= 2000 && m == 12 && dd == 26)
        || (m == 12 && dd == 31 && matches!(y, 1998 | 1999 | 2001))
}

/// UK bank holidays (England and Wales). Code "UKLN".
pub(super) fn uk_settlement_holiday(d: Date, w: Weekday) -> bool {
    let (y, m, dd) = d.ymd();
    let easter = days_from_easter(d);
    let monday = w == Weekday::Monday;
    // New Year's Day (Monday if on a weekend)
    (m == 1 && (dd == 1 || ((dd == 2 || dd == 3) && monday)))
        || matches!(easter, Some(-2) | Some(1))
        // Early May bank holiday: first Monday of May (moved in 1995 and 2020)
        || (m == 5 && dd <= 7 && monday && y != 1995 && y != 2020)
        || (m == 5 && dd == 8 && (y == 1995 || y == 2020))
        // Spring bank holiday: last Monday of May (moved for jubilees)
        || (m == 5 && dd >= 25 && monday && !matches!(y, 2002 | 2012 | 2022))
        || (y == 2002 && m == 6 && (dd == 3 || dd == 4))
        || (y == 2012 && m == 6 && (dd == 4 || dd == 5))
        || (y == 2022 && m == 6 && (dd == 2 || dd == 3))
        // Summer bank holiday: last Monday of August
        || (m == 8 && dd >= 25 && monday)
        // Christmas and Boxing Day with substitutes
        || (m == 12 && (dd == 25 || (dd == 27 && (monday || w == Weekday::Tuesday))))
        || (m == 12 && (dd == 26 || (dd == 28 && (monday || w == Weekday::Tuesday))))
        // One-offs
        || (y == 1999 && m == 12 && dd == 31)
        || (y == 2011 && m == 4 && dd == 29)
        || (y == 2022 && m == 9 && dd == 19)
        || (y == 2023 && m == 5 && dd == 8)
}

/// Vernal and autumnal equinox days (QuantLib's approximation).
fn japan_equinoxes(y: i32) -> (u32, u32) {
    let n = y - 2000;
    let moving = 0.242_194 * f64::from(n);
    let leaps = f64::from(n / 4 + n / 100 - n / 400);
    let ve = (20.691_15 + moving - leaps).floor() as u32;
    let ae = (23.09 + moving - leaps).floor() as u32;
    (ve, ae)
}

/// Japanese public holidays. Code "JPTO".
pub(super) fn japan_holiday(d: Date, w: Weekday) -> bool {
    let (y, m, dd) = d.ymd();
    let (ve, ae) = japan_equinoxes(y);
    let monday = w == Weekday::Monday;
    // Bank holidays
    (m == 1 && dd <= 3)
        // Coming of Age Day: second Monday of January (Jan 15 before 2000)
        || (m == 1 && y >= 2000 && monday && (8..=14).contains(&dd))
        || (m == 1 && y < 2000 && (dd == 15 || (dd == 16 && monday)))
        // National Foundation Day
        || (m == 2 && (dd == 11 || (dd == 12 && monday)))
        // Emperor's Birthday
        || (m == 2 && y >= 2020 && (dd == 23 || (dd == 24 && monday)))
        || (m == 12 && (1989..2019).contains(&y) && (dd == 23 || (dd == 24 && monday)))
        // Vernal Equinox
        || (m == 3 && (dd == ve || (dd == ve + 1 && monday)))
        // Greenery Day (Showa Day from 2007)
        || (m == 4 && (dd == 29 || (dd == 30 && monday)))
        // Constitution Memorial Day, Holiday for a Nation, Children's Day
        || (m == 5 && (dd == 3 || (dd == 6 && monday)))
        || (m == 5 && (dd == 4 || (dd == 6 && w == Weekday::Tuesday)))
        || (m == 5 && (dd == 5 || (dd == 6 && w == Weekday::Wednesday)))
        // Marine Day: third Monday of July from 2003 (July 20 1996-2002; Olympics 2020/2021)
        || (m == 7 && monday && (15..=21).contains(&dd) && ((2003..2020).contains(&y) || y >= 2022))
        || (m == 7 && (1996..2003).contains(&y) && (dd == 20 || (dd == 21 && monday)))
        || (y == 2020 && m == 7 && dd == 23)
        || (y == 2021 && m == 7 && dd == 22)
        // Mountain Day from 2016 (Olympics 2020/2021)
        || (m == 8 && ((2016..2020).contains(&y) || y >= 2022) && (dd == 11 || (dd == 12 && monday)))
        || (y == 2020 && m == 8 && dd == 10)
        || (y == 2021 && m == 8 && (dd == 8 || dd == 9))
        // Respect for the Aged Day: third Monday of September from 2003
        || (m == 9 && y >= 2003 && monday && (15..=21).contains(&dd))
        || (m == 9 && y < 2003 && (dd == 15 || (dd == 16 && monday)))
        // Bridge day between Respect for the Aged Day and the Autumnal Equinox
        || (m == 9 && y >= 2003 && w == Weekday::Tuesday && dd + 1 == ae && (16..=22).contains(&dd))
        // Autumnal Equinox
        || (m == 9 && (dd == ae || (dd == ae + 1 && monday)))
        // Health and Sports Day: second Monday of October from 2000 (Olympics 2020/2021)
        || (m == 10 && monday && (8..=14).contains(&dd) && ((2000..2020).contains(&y) || y >= 2022))
        || (m == 10 && y < 2000 && (dd == 10 || (dd == 11 && monday)))
        || (y == 2020 && m == 7 && dd == 24)
        || (y == 2021 && m == 7 && dd == 23)
        // Culture Day, Labor Thanksgiving Day
        || (m == 11 && (dd == 3 || (dd == 4 && monday)))
        || (m == 11 && (dd == 23 || (dd == 24 && monday)))
        // Bank holiday
        || (m == 12 && dd == 31)
        // One-offs
        || (y == 1959 && m == 4 && dd == 10)
        || (y == 1989 && m == 2 && dd == 24)
        || (y == 1990 && m == 11 && dd == 12)
        || (y == 1993 && m == 6 && dd == 9)
        || (y == 2019 && ((m == 4 && dd == 30) || (m == 5 && (dd == 1 || dd == 2)) || (m == 10 && dd == 22)))
}

#[cfg(test)]
mod tests {
    use super::super::calendar::{Calendar, Market};
    use super::*;

    fn d(y: i32, m: u32, dd: u32) -> Date {
        Date::from_ymd(y, m, dd).expect("valid test date")
    }

    #[test]
    fn target_known_days() {
        let cal = Calendar::new(Market::Target);
        assert!(cal.is_holiday(d(2026, 4, 3))); // Good Friday
        assert!(cal.is_holiday(d(2026, 4, 6))); // Easter Monday
        assert!(cal.is_holiday(d(2026, 5, 1)));
        assert!(cal.is_holiday(d(2025, 12, 26)));
        assert!(cal.is_business_day(d(2026, 7, 3))); // US holiday, not TARGET
        assert!(cal.is_business_day(d(2026, 12, 24)));
    }

    #[test]
    fn uk_known_days() {
        let cal = Calendar::new(Market::UkSettlement);
        assert!(cal.is_holiday(d(2026, 5, 4))); // early May
        assert!(cal.is_holiday(d(2026, 5, 25))); // spring
        assert!(cal.is_holiday(d(2026, 8, 31))); // summer
        assert!(cal.is_holiday(d(2027, 12, 27))); // Christmas on Saturday -> Monday
        assert!(cal.is_holiday(d(2027, 12, 28))); // Boxing Day on Sunday -> Tuesday
        assert!(cal.is_holiday(d(2023, 5, 8))); // coronation
        assert!(cal.is_holiday(d(2022, 6, 2)) && cal.is_holiday(d(2022, 6, 3)));
        assert!(cal.is_business_day(d(2022, 5, 30)));
    }

    #[test]
    fn japan_known_days() {
        let cal = Calendar::new(Market::Japan);
        assert_eq!(japan_equinoxes(2026), (20, 23));
        assert!(cal.is_holiday(d(2026, 1, 12))); // Coming of Age
        assert!(cal.is_holiday(d(2026, 3, 20)));
        assert!(cal.is_holiday(d(2026, 5, 6))); // Constitution Day on Sunday -> substitute
        assert!(cal.is_holiday(d(2026, 7, 20))); // Marine Day
        assert!(cal.is_holiday(d(2026, 9, 22))); // bridge day: Mon 21 Respect, Wed 23 equinox
        assert!(cal.is_holiday(d(2020, 7, 23)) && cal.is_holiday(d(2020, 7, 24)));
        assert!(cal.is_business_day(d(2020, 7, 20)));
        assert!(cal.is_holiday(d(2019, 5, 1)));
    }
}
