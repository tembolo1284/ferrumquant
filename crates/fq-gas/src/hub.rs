//! Gas hubs and NYMEX date conventions.
//!
//! Henry Hub is the benchmark: NYMEX NG futures settle there and every
//! other North American location trades as a *basis* to it. The hubs below
//! are the ones that show up on a typical desk's basis sheet; the codes
//! are the common short names rather than any one vendor's tickers.
//!
//! NYMEX NG futures stop trading three business days before the first
//! calendar day of the delivery month, and the monthly index for a hub is
//! set during *bid week*, the last five business days before that expiry.
//! Both rules live here as plain functions on the core calendar.

use fq_commod_core::{Commodity, ContractMonth, Hub, Unit};
use fq_core::time::{Calendar, Date, Market};

use crate::error::GasError;

/// Major North American pricing points.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GasHub {
    /// Erath, LA. NYMEX NG delivery point and the US benchmark.
    HenryHub,
    /// Transco Zone 6 New York: the premium East Coast winter hub.
    TranscoZ6NY,
    /// Transco Zone 6 non-NY (NJ/PA).
    TranscoZ6NonNY,
    /// Texas Eastern M3 (Mid-Atlantic).
    TetcoM3,
    /// Texas Eastern M2 receipts (Appalachia producing region).
    TetcoM2,
    /// Dominion South Point (Appalachia producing region).
    DominionSouth,
    /// Algonquin Citygate (New England).
    AlgonquinCitygate,
    /// Chicago Citygate.
    ChicagoCitygate,
    /// MichCon Citygate.
    MichCon,
    /// Dawn, Ontario storage hub.
    Dawn,
    /// Waha (Permian). Routinely negative when takeaway is constrained.
    Waha,
    /// Houston Ship Channel.
    HoustonShipChannel,
    /// SoCal Citygate.
    SoCalCitygate,
    /// PG&E Citygate.
    PgeCitygate,
    /// Opal, WY (Rockies).
    Opal,
    /// AECO-C, Alberta.
    Aeco,
    /// Columbia Gas TCO pool (Appalachia).
    ColumbiaTco,
    /// NGPL Midcontinent.
    NgplMidcon,
}

impl GasHub {
    pub const ALL: [GasHub; 18] = [
        Self::HenryHub,
        Self::TranscoZ6NY,
        Self::TranscoZ6NonNY,
        Self::TetcoM3,
        Self::TetcoM2,
        Self::DominionSouth,
        Self::AlgonquinCitygate,
        Self::ChicagoCitygate,
        Self::MichCon,
        Self::Dawn,
        Self::Waha,
        Self::HoustonShipChannel,
        Self::SoCalCitygate,
        Self::PgeCitygate,
        Self::Opal,
        Self::Aeco,
        Self::ColumbiaTco,
        Self::NgplMidcon,
    ];

    pub const fn code(self) -> &'static str {
        match self {
            Self::HenryHub => "HH",
            Self::TranscoZ6NY => "TZ6NY",
            Self::TranscoZ6NonNY => "TZ6NNY",
            Self::TetcoM3 => "TETM3",
            Self::TetcoM2 => "TETM2",
            Self::DominionSouth => "DOMSP",
            Self::AlgonquinCitygate => "ALGCG",
            Self::ChicagoCitygate => "CHICG",
            Self::MichCon => "MICHCON",
            Self::Dawn => "DAWN",
            Self::Waha => "WAHA",
            Self::HoustonShipChannel => "HSC",
            Self::SoCalCitygate => "SOCALCG",
            Self::PgeCitygate => "PGECG",
            Self::Opal => "OPAL",
            Self::Aeco => "AECO",
            Self::ColumbiaTco => "TCO",
            Self::NgplMidcon => "NGPLMC",
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::HenryHub => "Henry Hub",
            Self::TranscoZ6NY => "Transco Zone 6 NY",
            Self::TranscoZ6NonNY => "Transco Zone 6 non-NY",
            Self::TetcoM3 => "Texas Eastern M3",
            Self::TetcoM2 => "Texas Eastern M2",
            Self::DominionSouth => "Dominion South Point",
            Self::AlgonquinCitygate => "Algonquin Citygate",
            Self::ChicagoCitygate => "Chicago Citygate",
            Self::MichCon => "MichCon Citygate",
            Self::Dawn => "Dawn Hub",
            Self::Waha => "Waha",
            Self::HoustonShipChannel => "Houston Ship Channel",
            Self::SoCalCitygate => "SoCal Citygate",
            Self::PgeCitygate => "PG&E Citygate",
            Self::Opal => "Opal",
            Self::Aeco => "AECO-C",
            Self::ColumbiaTco => "Columbia Gas TCO Pool",
            Self::NgplMidcon => "NGPL Midcontinent",
        }
    }

    /// Hubs where basis is structurally negative (producing regions with
    /// constrained takeaway). Useful for sanity checks on quotes.
    pub const fn is_producing_region(self) -> bool {
        matches!(self, Self::TetcoM2 | Self::DominionSouth | Self::Waha | Self::Opal | Self::Aeco | Self::ColumbiaTco)
    }

    /// Quoted in CAD/GJ natively; everything else is USD/MMBtu.
    pub const fn is_canadian(self) -> bool {
        matches!(self, Self::Dawn | Self::Aeco)
    }

    /// The core `Hub` for this location. All priced in MMBtu here; the
    /// GJ/CAD conversion for AECO and Dawn is a market-data concern.
    pub fn hub(self) -> Hub {
        Hub::new(self.code(), Commodity::NaturalGas, Unit::MmBtu)
    }

    pub fn from_code(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|h| h.code().eq_ignore_ascii_case(code))
    }
}

/// The exchange calendar used for NYMEX expiries. CME follows the US
/// settlement holidays closely enough for expiry arithmetic.
#[derive(Debug, Clone)]
pub struct NymexCalendar {
    cal: Calendar,
}

impl Default for NymexCalendar {
    fn default() -> Self {
        Self::new()
    }
}

impl NymexCalendar {
    pub fn new() -> Self {
        Self { cal: Calendar::new(Market::UsSettlement) }
    }

    pub fn calendar(&self) -> &Calendar {
        &self.cal
    }
}

/// NYMEX NG last trading day for a delivery month: three business days
/// before the first calendar day of that month.
pub fn nymex_expiry(month: ContractMonth, cal: &NymexCalendar) -> Result<Date, GasError> {
    let first = month.first_day()?;
    Ok(cal.calendar().advance(first, -3)?)
}

/// Bid week: the five business days ending on the NYMEX expiry day, over
/// which monthly index prices are assessed. Returned oldest first.
pub fn bidweek(month: ContractMonth, cal: &NymexCalendar) -> Result<Vec<Date>, GasError> {
    let last = nymex_expiry(month, cal)?;
    let mut days = Vec::with_capacity(5);
    let mut d = last;
    for _ in 0..5 {
        days.push(d);
        d = cal.calendar().advance(d, -1)?;
    }
    days.reverse();
    Ok(days)
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
    fn hub_codes_round_trip() {
        for h in GasHub::ALL {
            assert_eq!(GasHub::from_code(h.code()), Some(h));
            assert_eq!(h.hub().unit(), Unit::MmBtu);
        }
        assert_eq!(GasHub::from_code("hh"), Some(GasHub::HenryHub));
        assert!(GasHub::from_code("NOPE").is_none());
        assert!(GasHub::Waha.is_producing_region());
        assert!(!GasHub::TranscoZ6NY.is_producing_region());
    }

    #[test]
    fn nymex_expiry_three_business_days_before_month() {
        let cal = NymexCalendar::new();
        // Jan-27: Jan 1 is a Friday holiday; three business days back from
        // Jan 1 is Dec 29 (Tue): Dec 31 Thu, Dec 30 Wed, Dec 29 Tue.
        assert_eq!(nymex_expiry(cm(2027, 1), &cal).expect("exp"), ymd(2026, 12, 29));
        // Feb-27: Feb 1 is a Monday; back three business days is Jan 27 (Wed).
        assert_eq!(nymex_expiry(cm(2027, 2), &cal).expect("exp"), ymd(2027, 1, 27));
        // Jun-27: Jun 1 is Tuesday; Memorial Day is May 31, so May 28, 27, 26.
        assert_eq!(nymex_expiry(cm(2027, 6), &cal).expect("exp"), ymd(2027, 5, 26));
    }

    #[test]
    fn bidweek_is_five_business_days_ending_at_expiry() {
        let cal = NymexCalendar::new();
        let bw = bidweek(cm(2027, 2), &cal).expect("bw");
        assert_eq!(bw.len(), 5);
        assert_eq!(bw[4], ymd(2027, 1, 27));
        assert_eq!(bw[0], ymd(2027, 1, 21));
        assert!(bw.windows(2).all(|w| w[0] < w[1]));
    }
}
