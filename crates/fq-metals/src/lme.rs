//! LME prompt-date rules.
//!
//! The LME is a forward market with daily settlement dates ("prompts")
//! rather than a handful of monthly futures:
//!
//! * **Cash** is two business days after the trade date.
//! * **3M** is the same calendar day three months ahead; if that is not a
//!   business day it moves to the next business day, unless that crosses
//!   into the following month, in which case it moves back.
//! * Every business day from cash to 3M is a valid prompt; from 3M to 6M
//!   only Wednesdays; beyond 6M only the third Wednesday of each month,
//!   out to the metal's horizon (123 months for most metals, 15 for tin).
//!
//! LME options are on the third-Wednesday prompt of their month and are
//! declared on the first Wednesday of that month.
//!
//! The LME closes on UK bank holidays and, since it settles in USD, treats
//! US holidays as non-settlement days too; the cash/3M date rules here use
//! the UK calendar, which is what matters for the prompt structure.

use fq_core::time::{Calendar, Date, Market, Weekday};

use crate::error::MetalsError;
use crate::metal::Metal;

#[derive(Debug, Clone)]
pub struct LmeCalendar {
    cal: Calendar,
}

impl Default for LmeCalendar {
    fn default() -> Self {
        Self::new()
    }
}

impl LmeCalendar {
    pub fn new() -> Self {
        Self { cal: Calendar::new(Market::UkSettlement) }
    }

    pub fn calendar(&self) -> &Calendar {
        &self.cal
    }

    pub fn is_business_day(&self, d: Date) -> bool {
        self.cal.is_business_day(d)
    }
}

/// Where a prompt sits in the LME structure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PromptKind {
    Cash,
    /// Daily prompt between cash and 3M.
    Daily,
    ThreeMonth,
    /// Wednesday prompt between 3M and 6M.
    Weekly,
    /// Third Wednesday of a month beyond 6M.
    Monthly,
}

pub fn cash_prompt(trade_date: Date, cal: &LmeCalendar) -> Result<Date, MetalsError> {
    Ok(cal.calendar().advance(trade_date, 2)?)
}

/// Same day `months` ahead (clamped to month end), then the LME roll:
/// forward to the next business day unless that leaves the month, then
/// back to the previous one.
fn month_prompt(trade_date: Date, months: i32, cal: &LmeCalendar) -> Result<Date, MetalsError> {
    let target = trade_date.add_months(months)?;
    if cal.is_business_day(target) {
        return Ok(target);
    }
    let fwd = cal.calendar().advance(target, 1)?;
    if fwd.month() == target.month() { Ok(fwd) } else { Ok(cal.calendar().advance(target, -1)?) }
}

pub fn three_month_prompt(trade_date: Date, cal: &LmeCalendar) -> Result<Date, MetalsError> {
    month_prompt(trade_date, 3, cal)
}

pub fn six_month_prompt(trade_date: Date, cal: &LmeCalendar) -> Result<Date, MetalsError> {
    month_prompt(trade_date, 6, cal)
}

/// Third Wednesday of a month, moved to the next business day if it is a
/// holiday (rare but it happens, e.g. around Christmas).
pub fn third_wednesday(year: i32, month: u32, cal: &LmeCalendar) -> Result<Date, MetalsError> {
    let d = Date::nth_weekday(3, Weekday::Wednesday, year, month)?;
    Ok(cal.calendar().advance(d, 0)?)
}

/// Declaration day for the option on `year`/`month`'s third-Wednesday
/// prompt: the first Wednesday of that month.
pub fn declaration_day(year: i32, month: u32, cal: &LmeCalendar) -> Result<Date, MetalsError> {
    let d = Date::nth_weekday(1, Weekday::Wednesday, year, month)?;
    Ok(cal.calendar().advance(d, 0)?)
}

/// Classify `d` as a prompt as seen from `trade_date`, or `None` if it is
/// not a valid prompt for `metal`.
pub fn classify_prompt(metal: Metal, trade_date: Date, d: Date, cal: &LmeCalendar) -> Result<Option<PromptKind>, MetalsError> {
    if metal.is_precious() {
        return Err(MetalsError::NotBaseMetal(metal.name()));
    }
    if !cal.is_business_day(d) {
        return Ok(None);
    }
    let cash = cash_prompt(trade_date, cal)?;
    let m3 = three_month_prompt(trade_date, cal)?;
    let m6 = six_month_prompt(trade_date, cal)?;
    let horizon = trade_date.add_months(metal.lme_horizon_months())?.end_of_month();
    if d < cash || d > horizon {
        return Ok(None);
    }
    if d == cash {
        return Ok(Some(PromptKind::Cash));
    }
    if d == m3 {
        return Ok(Some(PromptKind::ThreeMonth));
    }
    if d < m3 {
        return Ok(Some(PromptKind::Daily));
    }
    let is_third_wed = third_wednesday(d.year(), d.month(), cal)? == d;
    if d <= m6 {
        if d.weekday() == Weekday::Wednesday || is_third_wed {
            return Ok(Some(if is_third_wed { PromptKind::Monthly } else { PromptKind::Weekly }));
        }
        return Ok(None);
    }
    Ok(if is_third_wed { Some(PromptKind::Monthly) } else { None })
}

pub fn is_valid_prompt(metal: Metal, trade_date: Date, d: Date, cal: &LmeCalendar) -> Result<bool, MetalsError> {
    Ok(classify_prompt(metal, trade_date, d, cal)?.is_some())
}

/// All valid prompts from cash out to `months` ahead, in order.
pub fn prompts(metal: Metal, trade_date: Date, months: i32, cal: &LmeCalendar) -> Result<Vec<(Date, PromptKind)>, MetalsError> {
    let cash = cash_prompt(trade_date, cal)?;
    let end = trade_date.add_months(months.min(metal.lme_horizon_months()))?.end_of_month();
    let mut out = Vec::new();
    for s in cash.serial()..=end.serial() {
        let d = Date::from_serial(s)?;
        if let Some(k) = classify_prompt(metal, trade_date, d, cal)? {
            out.push((d, k));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ymd(y: i32, m: u32, d: u32) -> Date {
        Date::from_ymd(y, m, d).expect("valid date")
    }

    #[test]
    fn cash_and_three_month() {
        let cal = LmeCalendar::new();
        // Mon Nov 9 2026: cash Wed Nov 11 (no UK holiday), 3M Tue Feb 9 2027.
        assert_eq!(cash_prompt(ymd(2026, 11, 9), &cal).expect("c"), ymd(2026, 11, 11));
        assert_eq!(three_month_prompt(ymd(2026, 11, 9), &cal).expect("3m"), ymd(2027, 2, 9));
        // Sat target rolls forward: Thu Nov 12 2026 + 3M = Fri Feb 12 2027 (business day).
        assert_eq!(three_month_prompt(ymd(2026, 11, 12), &cal).expect("3m"), ymd(2027, 2, 12));
        // Fri Nov 13 + 3M = Sat Feb 13 -> Mon Feb 15.
        assert_eq!(three_month_prompt(ymd(2026, 11, 13), &cal).expect("3m"), ymd(2027, 2, 15));
        // Month-end: Thu Jan 28 2027 + 3M = Wed Apr 28 (fine). Fri Jan 29 + 3M = Thu Apr 29.
        // Mon Mar 1 + 3M = Tue Jun 1. A case that would cross the month:
        // Wed Mar 31 2027 + 3M = Wed Jun 30 (business day) -> Jun 30.
        assert_eq!(three_month_prompt(ymd(2027, 3, 31), &cal).expect("3m"), ymd(2027, 6, 30));
        // Thu Jul 29 2027 + 3M = Fri Oct 29 (business). Fri Jul 30 + 3M = Sat Oct 30 -> forward
        // is Mon Nov 1 (next month) -> back to Fri Oct 29.
        assert_eq!(three_month_prompt(ymd(2027, 7, 30), &cal).expect("3m"), ymd(2027, 10, 29));
    }

    #[test]
    fn third_wednesdays_and_declaration() {
        let cal = LmeCalendar::new();
        assert_eq!(third_wednesday(2027, 3, &cal).expect("w"), ymd(2027, 3, 17));
        assert_eq!(declaration_day(2027, 3, &cal).expect("d"), ymd(2027, 3, 3));
        assert_eq!(third_wednesday(2027, 12, &cal).expect("w"), ymd(2027, 12, 15));
    }

    #[test]
    fn prompt_classification() {
        let cal = LmeCalendar::new();
        let td = ymd(2026, 11, 9);
        let c = |d: Date| classify_prompt(Metal::Copper, td, d, &cal).expect("classify");
        assert_eq!(c(ymd(2026, 11, 11)), Some(PromptKind::Cash));
        assert_eq!(c(ymd(2026, 11, 10)), None); // before cash
        assert_eq!(c(ymd(2026, 12, 3)), Some(PromptKind::Daily));
        assert_eq!(c(ymd(2026, 12, 5)), None); // Saturday
        assert_eq!(c(ymd(2027, 2, 9)), Some(PromptKind::ThreeMonth));
        // Between 3M (Feb 9) and 6M (May 10 2027): Wednesdays only.
        assert_eq!(c(ymd(2027, 3, 10)), Some(PromptKind::Weekly));
        assert_eq!(c(ymd(2027, 3, 11)), None);
        assert_eq!(c(ymd(2027, 3, 17)), Some(PromptKind::Monthly));
        // Beyond 6M: third Wednesdays only.
        assert_eq!(c(ymd(2027, 9, 15)), Some(PromptKind::Monthly));
        assert_eq!(c(ymd(2027, 9, 8)), None);
        // Tin's horizon is 15 months: Feb 2028 is out.
        assert!(!is_valid_prompt(Metal::Tin, td, ymd(2028, 3, 15), &cal).expect("v"));
        assert!(is_valid_prompt(Metal::Copper, td, ymd(2028, 3, 15), &cal).expect("v"));
        assert!(matches!(classify_prompt(Metal::Gold, td, ymd(2027, 1, 20), &cal), Err(MetalsError::NotBaseMetal(_))));
    }

    #[test]
    fn prompt_list_is_dense_then_sparse() {
        let cal = LmeCalendar::new();
        let td = ymd(2026, 11, 9);
        let ps = prompts(Metal::Aluminium, td, 9, &cal).expect("prompts");
        assert_eq!(ps[0], (ymd(2026, 11, 11), PromptKind::Cash));
        let daily = ps.iter().filter(|p| p.1 == PromptKind::Daily).count();
        let weekly = ps.iter().filter(|p| p.1 == PromptKind::Weekly).count();
        let monthly = ps.iter().filter(|p| p.1 == PromptKind::Monthly).count();
        assert!(daily > 55 && daily < 65, "daily {daily}");
        assert!((9..=11).contains(&weekly), "weekly {weekly}");
        // Third Wednesdays from Feb/Mar 2027 to Aug 2027.
        assert!((6..=8).contains(&monthly), "monthly {monthly}");
        assert!(ps.windows(2).all(|w| w[0].0 < w[1].0));
    }
}
