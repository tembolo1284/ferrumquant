//! ISO/RTO calendars, peak definitions and block hour counting.
//!
//! Every US power market trades the same standard blocks, but two
//! conventions differ by region. Eastern and Central markets (PJM, MISO,
//! ERCOT, NYISO, ISO-NE, SPP) define on-peak as hours ending 7 through 22
//! on Monday to Friday excluding NERC holidays: the **5x16**. WECC markets
//! (CAISO, Mid-C, Palo Verde) include Saturdays: the **6x16**. Off-peak is
//! everything else, split into the **2x16** (peak hours on non-peak days)
//! and the **7x8** (nights). ATC is all hours.
//!
//! The six NERC holidays are New Year's Day, Memorial Day, Independence
//! Day, Labor Day, Thanksgiving and Christmas; a holiday falling on a
//! Sunday is observed on Monday, one falling on a Saturday is not moved.
//!
//! Hours in a day are 24 except on the US daylight-saving transitions
//! (23 on the second Sunday of March, 25 on the first Sunday of November),
//! and since those fall on Sundays in the small hours they land in the 7x8.

use fq_commod_core::DeliveryPeriod;
use fq_core::time::{Date, Weekday};

/// Independent system operators / regions with their own peak convention.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Iso {
    Pjm,
    Miso,
    Ercot,
    Nyiso,
    Isone,
    Spp,
    /// California, 6x16 peak.
    Caiso,
    /// Non-ISO WECC bilateral hubs (Mid-C, Palo Verde), 6x16 peak.
    Wecc,
}

impl Iso {
    pub const ALL: [Iso; 8] = [Self::Pjm, Self::Miso, Self::Ercot, Self::Nyiso, Self::Isone, Self::Spp, Self::Caiso, Self::Wecc];

    pub const fn code(self) -> &'static str {
        match self {
            Self::Pjm => "PJM",
            Self::Miso => "MISO",
            Self::Ercot => "ERCOT",
            Self::Nyiso => "NYISO",
            Self::Isone => "ISONE",
            Self::Spp => "SPP",
            Self::Caiso => "CAISO",
            Self::Wecc => "WECC",
        }
    }

    /// Saturday is a peak day in the West.
    pub const fn saturday_is_peak(self) -> bool {
        matches!(self, Self::Caiso | Self::Wecc)
    }

    /// 5 or 6, the "N" in Nx16.
    pub const fn peak_days_per_week(self) -> u32 {
        if self.saturday_is_peak() { 6 } else { 5 }
    }
}

/// Standard traded blocks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Block {
    /// HE7-22 on peak days (5x16 or 6x16 by ISO).
    Peak,
    /// Everything that is not Peak: 2x16 + 7x8.
    OffPeak,
    /// All hours, 7x24.
    Atc,
    /// HE7-22 on non-peak days (weekends/holidays).
    Wrap2x16,
    /// HE23-6 every day.
    Nights7x8,
}

impl Block {
    pub const fn code(self) -> &'static str {
        match self {
            Self::Peak => "PK",
            Self::OffPeak => "OP",
            Self::Atc => "ATC",
            Self::Wrap2x16 => "2x16",
            Self::Nights7x8 => "7x8",
        }
    }
}

fn observed(d: Date) -> Date {
    // NERC: Sunday holidays move to Monday; Saturday holidays stay put.
    if d.weekday() == Weekday::Sunday { d + 1 } else { d }
}

/// The six NERC holidays for a year, observed dates, in calendar order.
pub fn nerc_holidays(year: i32) -> Vec<Date> {
    let mut out = Vec::with_capacity(6);
    if let Ok(d) = Date::from_ymd(year, 1, 1) {
        out.push(observed(d));
    }
    if let Ok(d) = Date::last_weekday(Weekday::Monday, year, 5) {
        out.push(d);
    }
    if let Ok(d) = Date::from_ymd(year, 7, 4) {
        out.push(observed(d));
    }
    if let Ok(d) = Date::nth_weekday(1, Weekday::Monday, year, 9) {
        out.push(d);
    }
    if let Ok(d) = Date::nth_weekday(4, Weekday::Thursday, year, 11) {
        out.push(d);
    }
    if let Ok(d) = Date::from_ymd(year, 12, 25) {
        out.push(observed(d));
    }
    out
}

pub fn is_nerc_holiday(d: Date) -> bool {
    nerc_holidays(d.year()).contains(&d)
}

/// A peak day is a weekday (plus Saturday in the West) that is not a NERC
/// holiday.
pub fn is_peak_day(iso: Iso, d: Date) -> bool {
    let wd = d.weekday();
    let weekday_ok = match wd {
        Weekday::Sunday => false,
        Weekday::Saturday => iso.saturday_is_peak(),
        _ => true,
    };
    weekday_ok && !is_nerc_holiday(d)
}

/// Peak hours are hours ending 7 through 22 (i.e. 06:00 to 22:00).
pub const fn is_peak_hour(hour_ending: u32) -> bool {
    hour_ending >= 7 && hour_ending <= 22
}

/// US daylight-saving transitions: 23 hours on the second Sunday of March,
/// 25 on the first Sunday of November, else 24.
pub fn hours_in_day(d: Date) -> u32 {
    let y = d.year();
    let spring = Date::nth_weekday(2, Weekday::Sunday, y, 3).ok();
    let fall = Date::nth_weekday(1, Weekday::Sunday, y, 11).ok();
    if spring == Some(d) {
        23
    } else if fall == Some(d) {
        25
    } else {
        24
    }
}

/// Hours of `block` in one day at `iso`.
pub fn block_hours_in_day(iso: Iso, block: Block, d: Date) -> u32 {
    let total = hours_in_day(d);
    let peak_day = is_peak_day(iso, d);
    let daytime = 16;
    let nights = total - daytime;
    match block {
        Block::Peak => {
            if peak_day {
                daytime
            } else {
                0
            }
        }
        Block::Wrap2x16 => {
            if peak_day {
                0
            } else {
                daytime
            }
        }
        Block::Nights7x8 => nights,
        Block::OffPeak => nights + if peak_day { 0 } else { daytime },
        Block::Atc => total,
    }
}

/// Hours of `block` over a delivery period at `iso`.
pub fn block_hours(iso: Iso, block: Block, period: DeliveryPeriod) -> u32 {
    (period.start().serial()..=period.end().serial()).filter_map(|s| Date::from_serial(s).ok()).map(|d| block_hours_in_day(iso, block, d)).sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use fq_commod_core::ContractMonth;

    fn ymd(y: i32, m: u32, d: u32) -> Date {
        Date::from_ymd(y, m, d).expect("valid date")
    }

    fn month(y: i32, m: u32) -> DeliveryPeriod {
        ContractMonth::new(y, m).expect("month").delivery().expect("delivery")
    }

    #[test]
    fn nerc_holidays_2027() {
        let h = nerc_holidays(2027);
        assert_eq!(h, vec![ymd(2027, 1, 1), ymd(2027, 5, 31), ymd(2027, 7, 5), ymd(2027, 9, 6), ymd(2027, 11, 25), ymd(2027, 12, 25)]);
        // Jul 4 2027 is a Sunday -> observed Monday Jul 5; Dec 25 2027 is a
        // Saturday -> not moved.
        assert!(is_nerc_holiday(ymd(2027, 7, 5)));
        assert!(!is_nerc_holiday(ymd(2027, 7, 4)));
        assert!(is_nerc_holiday(ymd(2027, 12, 25)));
        assert!(!is_nerc_holiday(ymd(2027, 12, 27)));
    }

    #[test]
    fn peak_days_east_and_west() {
        // Sat Jan 9 2027.
        assert!(!is_peak_day(Iso::Pjm, ymd(2027, 1, 9)));
        assert!(is_peak_day(Iso::Caiso, ymd(2027, 1, 9)));
        assert!(!is_peak_day(Iso::Caiso, ymd(2027, 1, 10)));
        // New Year's Day (Friday) is off-peak everywhere.
        assert!(!is_peak_day(Iso::Pjm, ymd(2027, 1, 1)));
        assert!(is_peak_day(Iso::Ercot, ymd(2027, 1, 4)));
        assert!(is_peak_hour(7) && is_peak_hour(22) && !is_peak_hour(6) && !is_peak_hour(23));
    }

    #[test]
    fn dst_days_have_23_and_25_hours() {
        assert_eq!(hours_in_day(ymd(2027, 3, 14)), 23);
        assert_eq!(hours_in_day(ymd(2027, 11, 7)), 25);
        assert_eq!(hours_in_day(ymd(2027, 3, 7)), 24);
        assert_eq!(block_hours(Iso::Pjm, Block::Atc, month(2027, 3)), 31 * 24 - 1);
        assert_eq!(block_hours(Iso::Pjm, Block::Atc, month(2027, 11)), 30 * 24 + 1);
        // The odd hour is a night hour.
        assert_eq!(block_hours_in_day(Iso::Pjm, Block::Nights7x8, ymd(2027, 11, 7)), 9);
        assert_eq!(block_hours_in_day(Iso::Pjm, Block::Wrap2x16, ymd(2027, 11, 7)), 16);
    }

    #[test]
    fn january_2027_block_hours() {
        // Jan 2027 starts on a Friday: 21 weekdays less New Year's = 20 peak days.
        let jan = month(2027, 1);
        assert_eq!(block_hours(Iso::Pjm, Block::Peak, jan), 20 * 16);
        assert_eq!(block_hours(Iso::Pjm, Block::Atc, jan), 744);
        assert_eq!(block_hours(Iso::Pjm, Block::OffPeak, jan), 744 - 320);
        assert_eq!(block_hours(Iso::Pjm, Block::Nights7x8, jan), 31 * 8);
        assert_eq!(block_hours(Iso::Pjm, Block::Wrap2x16, jan), 11 * 16);
        // Five Saturdays in Jan 2027 join the peak in the West.
        assert_eq!(block_hours(Iso::Caiso, Block::Peak, jan), 25 * 16);
        // Blocks partition ATC.
        for iso in Iso::ALL {
            let p = block_hours(iso, Block::Peak, jan);
            let w = block_hours(iso, Block::Wrap2x16, jan);
            let n = block_hours(iso, Block::Nights7x8, jan);
            assert_eq!(p + w + n, 744);
            assert_eq!(block_hours(iso, Block::OffPeak, jan), w + n);
        }
    }
}
