//! Agricultural contracts: units, listed months, expiry rules, calendar.
//!
//! Listed months and last-trading-day rules follow the exchange rulebooks:
//!
//! * **CBOT grains and oilseeds** (ZC, ZW, ZS, ZM, ZL): the business day
//!   before the 15th calendar day of the contract month. Options expire on
//!   the last Friday that is at least two business days before the last
//!   business day of the month preceding the contract month.
//! * **ICE US softs**: coffee (KC) eight business days before the last
//!   business day of the delivery month; sugar No. 11 (SB) the last
//!   business day of the month preceding delivery; cocoa (CC) eleven
//!   business days before the last business day; cotton (CT) seventeen
//!   business days from the end of the spot month; FCOJ (OJ) the
//!   fourteenth business day before the last business day.
//! * **CME livestock**: live cattle (LE) the last business day of the
//!   contract month; feeder cattle (GF) the last Thursday of the contract
//!   month; lean hogs (HE) the tenth business day of the contract month.
//!
//! All three exchanges close on Good Friday, which is not a US settlement
//! holiday, so [`AgCalendar`] layers it on top of the core calendar.
//! Softs and livestock option expiries are approximated as the business
//! day before the futures expiry; the published rules differ by a day or
//! two per contract and can be refined per product later.

use fq_commod_core::{Commodity, ContractMonth, Hub, Unit};
use fq_core::time::calendar::easter_sunday;
use fq_core::time::{Calendar, Date, Market, Weekday};

use crate::error::AgsError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Exchange {
    Cbot,
    IceUs,
    Cme,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AgSector {
    Grain,
    Oilseed,
    Soft,
    Livestock,
}

/// How the exchange quotes the price.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum QuoteUnit {
    CentsPerBushel,
    CentsPerPound,
    DollarsPerShortTon,
    DollarsPerMetricTon,
}

impl QuoteUnit {
    /// Multiply a quoted price by this to get dollars per physical unit.
    pub const fn to_dollars(self) -> f64 {
        match self {
            Self::CentsPerBushel | Self::CentsPerPound => 0.01,
            Self::DollarsPerShortTon | Self::DollarsPerMetricTon => 1.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AgContract {
    /// CBOT corn, 5,000 bu, cents/bu. H K N U Z.
    Corn,
    /// CBOT SRW wheat, 5,000 bu, cents/bu. H K N U Z.
    Wheat,
    /// CBOT soybeans, 5,000 bu, cents/bu. F H K N Q U X.
    Soybeans,
    /// CBOT soybean meal, 100 short tons, $/short ton. F H K N Q U V Z.
    SoybeanMeal,
    /// CBOT soybean oil, 60,000 lb, cents/lb. F H K N Q U V Z.
    SoybeanOil,
    /// ICE coffee "C", 37,500 lb, cents/lb. H K N U Z.
    Coffee,
    /// ICE sugar No. 11, 112,000 lb, cents/lb. H K N V.
    Sugar11,
    /// ICE cocoa, 10 metric tons, $/t. H K N U Z.
    Cocoa,
    /// ICE cotton No. 2, 50,000 lb, cents/lb. H K N V Z.
    Cotton,
    /// ICE FCOJ-A, 15,000 lb, cents/lb. F H K N U X.
    Fcoj,
    /// CME live cattle, 40,000 lb, cents/lb. G J M Q V Z.
    LiveCattle,
    /// CME feeder cattle, 50,000 lb, cents/lb. F H J K Q U V X.
    FeederCattle,
    /// CME lean hogs, 40,000 lb, cents/lb. G J K M N Q V Z.
    LeanHogs,
}

impl AgContract {
    pub const ALL: [AgContract; 13] = [
        Self::Corn,
        Self::Wheat,
        Self::Soybeans,
        Self::SoybeanMeal,
        Self::SoybeanOil,
        Self::Coffee,
        Self::Sugar11,
        Self::Cocoa,
        Self::Cotton,
        Self::Fcoj,
        Self::LiveCattle,
        Self::FeederCattle,
        Self::LeanHogs,
    ];

    pub const fn code(self) -> &'static str {
        match self {
            Self::Corn => "ZC",
            Self::Wheat => "ZW",
            Self::Soybeans => "ZS",
            Self::SoybeanMeal => "ZM",
            Self::SoybeanOil => "ZL",
            Self::Coffee => "KC",
            Self::Sugar11 => "SB",
            Self::Cocoa => "CC",
            Self::Cotton => "CT",
            Self::Fcoj => "OJ",
            Self::LiveCattle => "LE",
            Self::FeederCattle => "GF",
            Self::LeanHogs => "HE",
        }
    }

    pub const fn exchange(self) -> Exchange {
        match self {
            Self::Corn | Self::Wheat | Self::Soybeans | Self::SoybeanMeal | Self::SoybeanOil => Exchange::Cbot,
            Self::Coffee | Self::Sugar11 | Self::Cocoa | Self::Cotton | Self::Fcoj => Exchange::IceUs,
            Self::LiveCattle | Self::FeederCattle | Self::LeanHogs => Exchange::Cme,
        }
    }

    pub const fn sector(self) -> AgSector {
        match self {
            Self::Corn | Self::Wheat => AgSector::Grain,
            Self::Soybeans | Self::SoybeanMeal | Self::SoybeanOil => AgSector::Oilseed,
            Self::Coffee | Self::Sugar11 | Self::Cocoa | Self::Cotton | Self::Fcoj => AgSector::Soft,
            Self::LiveCattle | Self::FeederCattle | Self::LeanHogs => AgSector::Livestock,
        }
    }

    pub const fn commodity(self) -> Commodity {
        match self.sector() {
            AgSector::Grain | AgSector::Oilseed => Commodity::Grain,
            AgSector::Soft => Commodity::Soft,
            AgSector::Livestock => Commodity::Livestock,
        }
    }

    pub const fn quote_unit(self) -> QuoteUnit {
        match self {
            Self::Corn | Self::Wheat | Self::Soybeans => QuoteUnit::CentsPerBushel,
            Self::SoybeanMeal => QuoteUnit::DollarsPerShortTon,
            Self::Cocoa => QuoteUnit::DollarsPerMetricTon,
            _ => QuoteUnit::CentsPerPound,
        }
    }

    /// Physical unit of the lot (the core `Unit`).
    pub const fn unit(self) -> Unit {
        match self {
            Self::Corn | Self::Wheat | Self::Soybeans => Unit::Bushel,
            Self::SoybeanMeal | Self::Cocoa => Unit::MetricTon,
            _ => Unit::Pound,
        }
    }

    /// Lot size in the quote's physical unit (short tons for meal).
    pub const fn lot_size(self) -> f64 {
        match self {
            Self::Corn | Self::Wheat | Self::Soybeans => 5_000.0,
            Self::SoybeanMeal => 100.0,
            Self::SoybeanOil => 60_000.0,
            Self::Coffee => 37_500.0,
            Self::Sugar11 => 112_000.0,
            Self::Cocoa => 10.0,
            Self::Cotton => 50_000.0,
            Self::Fcoj => 15_000.0,
            Self::LiveCattle | Self::LeanHogs => 40_000.0,
            Self::FeederCattle => 50_000.0,
        }
    }

    /// Dollar value of one lot at a quoted price.
    pub fn lot_value(self, quoted_price: f64) -> f64 {
        quoted_price * self.quote_unit().to_dollars() * self.lot_size()
    }

    /// Storable commodities have calendar spreads bounded by the cost of
    /// carry; livestock does not.
    pub const fn is_storable(self) -> bool {
        !matches!(self.sector(), AgSector::Livestock)
    }

    /// Listed contract months (1 = Jan).
    pub const fn listed_months(self) -> &'static [u32] {
        match self {
            Self::Corn | Self::Wheat | Self::Coffee | Self::Cocoa => &[3, 5, 7, 9, 12],
            Self::Soybeans => &[1, 3, 5, 7, 8, 9, 11],
            Self::SoybeanMeal | Self::SoybeanOil => &[1, 3, 5, 7, 8, 9, 10, 12],
            Self::Sugar11 => &[3, 5, 7, 10],
            Self::Cotton => &[3, 5, 7, 10, 12],
            Self::Fcoj => &[1, 3, 5, 7, 9, 11],
            Self::LiveCattle => &[2, 4, 6, 8, 10, 12],
            Self::FeederCattle => &[1, 3, 4, 5, 8, 9, 10, 11],
            Self::LeanHogs => &[2, 4, 5, 6, 7, 8, 10, 12],
        }
    }

    pub fn is_listed(self, month: ContractMonth) -> bool {
        self.listed_months().contains(&month.month())
    }

    /// The first listed month on or after `month`.
    pub fn next_listed(self, month: ContractMonth) -> ContractMonth {
        let mut m = month;
        while !self.is_listed(m) {
            m = m.next();
        }
        m
    }

    pub fn hub(self) -> Hub {
        Hub::new(self.code(), self.commodity(), self.unit())
    }

    pub fn from_code(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.code().eq_ignore_ascii_case(code))
    }
}

/// US exchange calendar: US settlement holidays plus Good Friday.
#[derive(Debug, Clone)]
pub struct AgCalendar {
    cal: Calendar,
}

impl Default for AgCalendar {
    fn default() -> Self {
        Self::new()
    }
}

impl AgCalendar {
    pub fn new() -> Self {
        Self { cal: Calendar::new(Market::UsSettlement) }
    }

    pub fn is_good_friday(d: Date) -> bool {
        let (m, day) = easter_sunday(d.year());
        match Date::from_ymd(d.year(), m, day) {
            Ok(easter) => easter - d == 2,
            Err(_) => false,
        }
    }

    pub fn is_business_day(&self, d: Date) -> bool {
        self.cal.is_business_day(d) && !Self::is_good_friday(d)
    }

    /// Move `n` business days (`n = 0` rolls forward to a business day).
    pub fn advance(&self, d: Date, n: i32) -> Result<Date, AgsError> {
        let mut out = d;
        if n == 0 {
            while !self.is_business_day(out) {
                out = out.checked_add_days(1)?;
            }
            return Ok(out);
        }
        let step = n.signum();
        for _ in 0..n.abs() {
            out = out.checked_add_days(step)?;
            while !self.is_business_day(out) {
                out = out.checked_add_days(step)?;
            }
        }
        Ok(out)
    }

    /// Last business day on or before `d`.
    pub fn preceding(&self, d: Date) -> Result<Date, AgsError> {
        let mut out = d;
        while !self.is_business_day(out) {
            out = out.checked_add_days(-1)?;
        }
        Ok(out)
    }

    /// `n`-th business day of a month (1-based).
    pub fn nth_business_day(&self, month: ContractMonth, n: u32) -> Result<Date, AgsError> {
        let mut d = self.advance(month.first_day()?, 0)?;
        for _ in 1..n {
            d = self.advance(d, 1)?;
        }
        if d.month() != month.month() {
            return Err(AgsError::Invalid(format!("{month} has fewer than {n} business days")));
        }
        Ok(d)
    }

    pub fn last_business_day(&self, month: ContractMonth) -> Result<Date, AgsError> {
        self.preceding(month.last_day()?)
    }
}

fn check_listed(contract: AgContract, month: ContractMonth) -> Result<(), AgsError> {
    if !contract.is_listed(month) {
        return Err(AgsError::NotListed { contract: contract.code(), month });
    }
    Ok(())
}

/// Futures last trading day.
pub fn expiry(contract: AgContract, month: ContractMonth, cal: &AgCalendar) -> Result<Date, AgsError> {
    check_listed(contract, month)?;
    use AgContract as C;
    Ok(match contract {
        C::Corn | C::Wheat | C::Soybeans | C::SoybeanMeal | C::SoybeanOil => {
            let d15 = Date::from_ymd(month.year(), month.month(), 15)?;
            cal.advance(d15, -1)?
        }
        C::Coffee => cal.advance(cal.last_business_day(month)?, -8)?,
        C::Sugar11 => cal.last_business_day(month.prev())?,
        C::Cocoa => cal.advance(cal.last_business_day(month)?, -11)?,
        C::Cotton => cal.advance(cal.last_business_day(month)?, -17)?,
        C::Fcoj => cal.advance(cal.last_business_day(month)?, -14)?,
        C::LiveCattle => cal.last_business_day(month)?,
        C::FeederCattle => {
            let d = Date::last_weekday(Weekday::Thursday, month.year(), month.month())?;
            cal.preceding(d)?
        }
        C::LeanHogs => cal.nth_business_day(month, 10)?,
    })
}

/// Option expiry. Grains: the last Friday at least two business days
/// before the last business day of the month before the contract month.
/// Others: the business day before the futures expiry (approximation).
pub fn option_expiry(contract: AgContract, month: ContractMonth, cal: &AgCalendar) -> Result<Date, AgsError> {
    check_listed(contract, month)?;
    match contract.exchange() {
        Exchange::Cbot => {
            let limit = cal.advance(cal.last_business_day(month.prev())?, -2)?;
            let mut d = limit;
            while d.weekday() != Weekday::Friday || !cal.is_business_day(d) {
                d = d.checked_add_days(-1)?;
            }
            Ok(d)
        }
        _ => cal.advance(expiry(contract, month, cal)?, -1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ymd(y: i32, m: u32, d: u32) -> Date {
        Date::from_ymd(y, m, d).expect("valid date")
    }

    fn cm(y: i32, m: u32) -> ContractMonth {
        ContractMonth::new(y, m).expect("month")
    }

    #[test]
    fn specs_and_listing() {
        for c in AgContract::ALL {
            assert_eq!(AgContract::from_code(c.code()), Some(c));
            assert!(!c.listed_months().is_empty());
        }
        // Corn Dec-26 at 450 cents/bu: 5,000 bu * $4.50.
        assert!((AgContract::Corn.lot_value(450.0) - 22_500.0).abs() < 1e-9);
        // Meal at $320/short ton: 100 t lot.
        assert!((AgContract::SoybeanMeal.lot_value(320.0) - 32_000.0).abs() < 1e-9);
        assert!(AgContract::Corn.is_listed(cm(2027, 3)) && !AgContract::Corn.is_listed(cm(2027, 4)));
        assert_eq!(AgContract::Corn.next_listed(cm(2027, 4)), cm(2027, 5));
        assert_eq!(AgContract::Soybeans.next_listed(cm(2027, 12)), cm(2028, 1));
        assert!(AgContract::Corn.is_storable() && !AgContract::LeanHogs.is_storable());
    }

    #[test]
    fn calendar_knows_good_friday() {
        let cal = AgCalendar::new();
        // Easter 2027 is Mar 28, so Good Friday is Mar 26.
        assert!(AgCalendar::is_good_friday(ymd(2027, 3, 26)));
        assert!(!cal.is_business_day(ymd(2027, 3, 26)));
        assert!(cal.is_business_day(ymd(2027, 3, 25)));
        assert_eq!(cal.advance(ymd(2027, 3, 25), 1).expect("adv"), ymd(2027, 3, 29));
        assert_eq!(cal.nth_business_day(cm(2027, 2), 10).expect("nth"), ymd(2027, 2, 12));
        assert_eq!(cal.last_business_day(cm(2027, 3)).expect("lbd"), ymd(2027, 3, 31));
    }

    #[test]
    fn grain_expiries() {
        let cal = AgCalendar::new();
        // Corn Dec-26: Dec 15 2026 is a Tuesday -> Mon Dec 14.
        assert_eq!(expiry(AgContract::Corn, cm(2026, 12), &cal).expect("e"), ymd(2026, 12, 14));
        // Soybeans Jan-27: Jan 15 is a Friday -> Thu Jan 14.
        assert_eq!(expiry(AgContract::Soybeans, cm(2027, 1), &cal).expect("e"), ymd(2027, 1, 14));
        // Wheat Mar-27: Mar 15 is a Monday -> Fri Mar 12.
        assert_eq!(expiry(AgContract::Wheat, cm(2027, 3), &cal).expect("e"), ymd(2027, 3, 12));
        // Option on Mar-27 corn: last business day of Feb 2027 is Fri Feb 26;
        // two business days before is Wed Feb 24; last Friday on or before is Feb 19.
        assert_eq!(option_expiry(AgContract::Corn, cm(2027, 3), &cal).expect("oe"), ymd(2027, 2, 19));
        assert!(matches!(expiry(AgContract::Corn, cm(2027, 4), &cal), Err(AgsError::NotListed { .. })));
    }

    #[test]
    fn softs_expiries_march_2027_with_good_friday() {
        let cal = AgCalendar::new();
        // March 2027 business days counting back from Wed Mar 31, skipping
        // Good Friday Mar 26: 30, 29, 25, 24, 23, 22, 19, 18, 17, 16, 15, 12, 11, 10, 9, 8, 5.
        assert_eq!(expiry(AgContract::Coffee, cm(2027, 3), &cal).expect("e"), ymd(2027, 3, 18));
        assert_eq!(expiry(AgContract::Cocoa, cm(2027, 3), &cal).expect("e"), ymd(2027, 3, 15));
        assert_eq!(expiry(AgContract::Fcoj, cm(2027, 3), &cal).expect("e"), ymd(2027, 3, 10));
        assert_eq!(expiry(AgContract::Cotton, cm(2027, 3), &cal).expect("e"), ymd(2027, 3, 5));
        // Sugar Mar-27: last business day of Feb 2027 = Fri Feb 26.
        assert_eq!(expiry(AgContract::Sugar11, cm(2027, 3), &cal).expect("e"), ymd(2027, 2, 26));
        // Softs option expiry approximation: the business day before.
        assert_eq!(option_expiry(AgContract::Coffee, cm(2027, 3), &cal).expect("oe"), ymd(2027, 3, 17));
    }

    #[test]
    fn livestock_expiries() {
        let cal = AgCalendar::new();
        assert_eq!(expiry(AgContract::LiveCattle, cm(2026, 12), &cal).expect("e"), ymd(2026, 12, 31));
        // Feeder Jan-27: last Thursday is Jan 28.
        assert_eq!(expiry(AgContract::FeederCattle, cm(2027, 1), &cal).expect("e"), ymd(2027, 1, 28));
        // Lean hogs Feb-27: tenth business day is Feb 12.
        assert_eq!(expiry(AgContract::LeanHogs, cm(2027, 2), &cal).expect("e"), ymd(2027, 2, 12));
    }
}
