//! Oil contract specifications, expiry rules and front-month rolls.
//!
//! Each benchmark has its own last-trading-day rule, and because monthly
//! averages are taken over the *front month* settle on each pricing day,
//! a one-day error in an expiry rule shifts a chunk of the month's
//! averaging onto the wrong contract. The rules here are the exchange
//! rules as published:
//!
//! * **WTI (CME CL)**: three business days before the 25th calendar day of
//!   the month preceding delivery; if the 25th is not a business day,
//!   three business days before the last business day preceding the 25th.
//! * **Brent (ICE B)**: the last business day of the second month
//!   preceding the delivery month (Mar expires end of January).
//! * **RBOB (CME RB) and Heating Oil / ULSD (CME HO)**: the last business
//!   day of the month preceding delivery.
//! * **Low-sulphur gasoil (ICE G)**: two business days before the 14th
//!   calendar day of the delivery month.
//!
//! CME contracts use the US settlement calendar, ICE the UK one, which is
//! close enough for expiry arithmetic (exchange holiday lists differ from
//! bank holidays on a handful of days a decade).

use fq_commod_core::{Commodity, ContractMonth, Hub, Unit};
use fq_core::time::{BusinessDayConvention, Calendar, Date, Market};

use crate::error::OilError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Exchange {
    Cme,
    Ice,
}

impl Exchange {
    pub fn calendar(self) -> Calendar {
        match self {
            Self::Cme => Calendar::new(Market::UsSettlement),
            Self::Ice => Calendar::new(Market::UkSettlement),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OilContract {
    /// CME Light Sweet Crude, Cushing. $/bbl, 1,000 bbl.
    Wti,
    /// ICE Brent. $/bbl, 1,000 bbl.
    Brent,
    /// CME RBOB gasoline, NY Harbor. $/gal, 42,000 gal.
    Rbob,
    /// CME NY Harbor ULSD (heating oil). $/gal, 42,000 gal.
    HeatingOil,
    /// ICE Low Sulphur Gasoil. $/tonne, 100 tonnes.
    Gasoil,
}

impl OilContract {
    pub const ALL: [OilContract; 5] = [Self::Wti, Self::Brent, Self::Rbob, Self::HeatingOil, Self::Gasoil];

    pub const fn code(self) -> &'static str {
        match self {
            Self::Wti => "CL",
            Self::Brent => "BRN",
            Self::Rbob => "RB",
            Self::HeatingOil => "HO",
            Self::Gasoil => "GAS",
        }
    }

    pub const fn exchange(self) -> Exchange {
        match self {
            Self::Wti | Self::Rbob | Self::HeatingOil => Exchange::Cme,
            Self::Brent | Self::Gasoil => Exchange::Ice,
        }
    }

    pub const fn commodity(self) -> Commodity {
        match self {
            Self::Wti | Self::Brent => Commodity::Crude,
            Self::Rbob | Self::HeatingOil | Self::Gasoil => Commodity::RefinedProduct,
        }
    }

    /// Quotation unit.
    pub const fn unit(self) -> Unit {
        match self {
            Self::Wti | Self::Brent => Unit::Barrel,
            Self::Rbob | Self::HeatingOil => Unit::Gallon,
            Self::Gasoil => Unit::MetricTon,
        }
    }

    /// Contract size in quotation units.
    pub const fn lot_size(self) -> f64 {
        match self {
            Self::Wti | Self::Brent => 1_000.0,
            Self::Rbob | Self::HeatingOil => 42_000.0,
            Self::Gasoil => 100.0,
        }
    }

    pub const fn is_crude(self) -> bool {
        matches!(self, Self::Wti | Self::Brent)
    }

    /// Multiply a quoted price by this to get $/bbl: 42 gal/bbl for
    /// products quoted per gallon, 1/7.45 for gasoil (7.45 bbl per tonne
    /// is the standard conversion for middle distillates).
    pub const fn to_per_barrel(self) -> f64 {
        match self {
            Self::Wti | Self::Brent => 1.0,
            Self::Rbob | Self::HeatingOil => 42.0,
            Self::Gasoil => 1.0 / 7.45,
        }
    }

    /// The core `Hub` for this contract in its quotation unit.
    pub fn hub(self) -> Hub {
        Hub::new(self.code(), self.commodity(), self.unit())
    }

    pub fn from_code(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.code().eq_ignore_ascii_case(code))
    }
}

/// Holds the two exchange calendars so expiry and pricing-day lookups do
/// not rebuild them per call.
#[derive(Debug, Clone)]
pub struct OilMarket {
    cme: Calendar,
    ice: Calendar,
}

impl Default for OilMarket {
    fn default() -> Self {
        Self::new()
    }
}

impl OilMarket {
    pub fn new() -> Self {
        Self { cme: Exchange::Cme.calendar(), ice: Exchange::Ice.calendar() }
    }

    pub fn calendar(&self, contract: OilContract) -> &Calendar {
        match contract.exchange() {
            Exchange::Cme => &self.cme,
            Exchange::Ice => &self.ice,
        }
    }
}

/// Last trading day of `month`'s contract.
pub fn expiry(contract: OilContract, month: ContractMonth, market: &OilMarket) -> Result<Date, OilError> {
    let cal = market.calendar(contract);
    Ok(match contract {
        OilContract::Wti => {
            let prev = month.prev();
            let d25 = Date::from_ymd(prev.year(), prev.month(), 25)?;
            let anchor = cal.adjust(d25, BusinessDayConvention::Preceding)?;
            cal.advance(anchor, -3)?
        }
        OilContract::Brent => {
            let m = month.add_months(-2);
            cal.adjust(m.last_day()?, BusinessDayConvention::Preceding)?
        }
        OilContract::Rbob | OilContract::HeatingOil => cal.adjust(month.prev().last_day()?, BusinessDayConvention::Preceding)?,
        OilContract::Gasoil => {
            let d14 = Date::from_ymd(month.year(), month.month(), 14)?;
            cal.advance(d14, -2)?
        }
    })
}

/// Exchange business days in a calendar month: the days whose settles make
/// up the monthly average.
pub fn pricing_days(contract: OilContract, month: ContractMonth, market: &OilMarket) -> Result<Vec<Date>, OilError> {
    let cal = market.calendar(contract);
    let period = month.delivery()?;
    Ok((period.start().serial()..=period.end().serial()).filter_map(|s| Date::from_serial(s).ok()).filter(|d| cal.is_business_day(*d)).collect())
}

/// The first contract month still trading on `d` (a contract is live
/// through its expiry day). Searches from the month containing `d`.
pub fn front_month(contract: OilContract, d: Date, market: &OilMarket) -> Result<ContractMonth, OilError> {
    let mut m = ContractMonth::of(d);
    for _ in 0..6 {
        if expiry(contract, m, market)? >= d {
            return Ok(m);
        }
        m = m.next();
    }
    Err(OilError::NoFrontMonth(d))
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
    fn specs_and_conversions() {
        for c in OilContract::ALL {
            assert_eq!(OilContract::from_code(c.code()), Some(c));
            assert_eq!(c.hub().unit(), c.unit());
        }
        assert!((OilContract::Rbob.to_per_barrel() - 42.0).abs() < 1e-12);
        assert!((OilContract::Gasoil.to_per_barrel() * 7.45 - 1.0).abs() < 1e-12);
        assert!((OilContract::Wti.lot_size() - 1_000.0).abs() < 1e-12);
        assert!(OilContract::Brent.is_crude() && !OilContract::HeatingOil.is_crude());
    }

    #[test]
    fn wti_expiry_rule() {
        let mkt = OilMarket::new();
        // Jan-27: Dec 25 2026 is a Friday holiday -> anchor Dec 24 -> three
        // business days earlier is Dec 21.
        assert_eq!(expiry(OilContract::Wti, cm(2027, 1), &mkt).expect("exp"), ymd(2026, 12, 21));
        // Feb-27: Jan 25 2027 is a Monday -> Jan 22, 21, 20.
        assert_eq!(expiry(OilContract::Wti, cm(2027, 2), &mkt).expect("exp"), ymd(2027, 1, 20));
        // Mar-27: Feb 25 2027 is a Thursday -> Feb 24, 23, 22.
        assert_eq!(expiry(OilContract::Wti, cm(2027, 3), &mkt).expect("exp"), ymd(2027, 2, 22));
    }

    #[test]
    fn brent_products_and_gasoil_expiries() {
        let mkt = OilMarket::new();
        // Brent Mar-27: last business day of January 2027 = Fri Jan 29.
        assert_eq!(expiry(OilContract::Brent, cm(2027, 3), &mkt).expect("exp"), ymd(2027, 1, 29));
        // Brent Jul-27: last business day of May 2027. May 31 is a Monday
        // (Spring bank holiday in the UK) -> May 28.
        assert_eq!(expiry(OilContract::Brent, cm(2027, 7), &mkt).expect("exp"), ymd(2027, 5, 28));
        // RBOB / HO Feb-27: last business day of Jan 2027 = Jan 29.
        assert_eq!(expiry(OilContract::Rbob, cm(2027, 2), &mkt).expect("exp"), ymd(2027, 1, 29));
        assert_eq!(expiry(OilContract::HeatingOil, cm(2027, 2), &mkt).expect("exp"), ymd(2027, 1, 29));
        // Gasoil Feb-27: Feb 14 is a Sunday; two business days before -> Feb 11.
        assert_eq!(expiry(OilContract::Gasoil, cm(2027, 2), &mkt).expect("exp"), ymd(2027, 2, 11));
    }

    #[test]
    fn front_month_rolls_on_expiry() {
        let mkt = OilMarket::new();
        // WTI Jan-27 expires Dec 21 2026: Jan is front through Dec 21, Feb from Dec 22.
        assert_eq!(front_month(OilContract::Wti, ymd(2026, 12, 21), &mkt).expect("fm"), cm(2027, 1));
        assert_eq!(front_month(OilContract::Wti, ymd(2026, 12, 22), &mkt).expect("fm"), cm(2027, 2));
        // Brent on Dec 10 2026: Jan-27 expired end of Nov, Feb-27 expires end of Dec -> Feb.
        assert_eq!(front_month(OilContract::Brent, ymd(2026, 12, 10), &mkt).expect("fm"), cm(2027, 2));
    }

    #[test]
    fn pricing_days_are_exchange_business_days() {
        let mkt = OilMarket::new();
        let dec = pricing_days(OilContract::Wti, cm(2026, 12), &mkt).expect("days");
        // 23 weekdays in Dec 2026 less Christmas.
        assert_eq!(dec.len(), 22);
        assert!(!dec.contains(&ymd(2026, 12, 25)));
        let dec_ice = pricing_days(OilContract::Brent, cm(2026, 12), &mkt).expect("days");
        // UK also closes Boxing Day (Dec 26 is Saturday -> observed Mon Dec 28).
        assert_eq!(dec_ice.len(), 21);
    }
}
