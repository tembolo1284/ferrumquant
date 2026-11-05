//! Currencies and currency pairs with market conventions.
//!
//! A pair is always stored in market order (`EURUSD`, never `USDEUR`): the
//! base is the currency with higher priority in the interbank ranking
//! EUR > GBP > AUD > NZD > USD > CAD > CHF > JPY, so a rate is "units of
//! quote per one unit of base". Spot settles T+2 on the joint calendar of
//! both currencies except for USDCAD, USDTRY, USDRUB (T+1), and a spot or
//! forward date must also be a US business day because USD is the
//! settlement leg of almost every pair.

use std::fmt;
use std::str::FromStr;

use thiserror::Error;

use fq_core::time::{BusinessDayConvention, Calendar, Date, DateError, JoinRule, Market, Period};

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum CurrencyError {
    #[error("unknown currency '{0}'")]
    UnknownCurrency(String),
    #[error("cannot parse currency pair '{0}' (expected e.g. EURUSD or EUR/USD)")]
    BadPair(String),
    #[error("a currency pair needs two different currencies, got {0} twice")]
    SameCurrency(Currency),
    #[error(transparent)]
    Date(#[from] DateError),
}

/// ISO 4217 currencies with FX conventions attached.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Currency {
    EUR,
    GBP,
    AUD,
    NZD,
    USD,
    CAD,
    CHF,
    JPY,
    SEK,
    NOK,
    DKK,
    MXN,
    BRL,
    TRY,
    RUB,
    CNY,
    KRW,
    INR,
}

impl Currency {
    pub const fn code(self) -> &'static str {
        match self {
            Self::EUR => "EUR",
            Self::GBP => "GBP",
            Self::AUD => "AUD",
            Self::NZD => "NZD",
            Self::USD => "USD",
            Self::CAD => "CAD",
            Self::CHF => "CHF",
            Self::JPY => "JPY",
            Self::SEK => "SEK",
            Self::NOK => "NOK",
            Self::DKK => "DKK",
            Self::MXN => "MXN",
            Self::BRL => "BRL",
            Self::TRY => "TRY",
            Self::RUB => "RUB",
            Self::CNY => "CNY",
            Self::KRW => "KRW",
            Self::INR => "INR",
        }
    }

    /// Interbank quoting priority: the higher-priority currency is the base.
    pub const fn priority(self) -> u8 {
        match self {
            Self::EUR => 0,
            Self::GBP => 1,
            Self::AUD => 2,
            Self::NZD => 3,
            Self::USD => 4,
            Self::CAD => 5,
            Self::CHF => 6,
            Self::JPY => 7,
            Self::SEK => 8,
            Self::NOK => 9,
            Self::DKK => 10,
            Self::MXN => 11,
            Self::BRL => 12,
            Self::TRY => 13,
            Self::RUB => 14,
            Self::CNY => 15,
            Self::KRW => 16,
            Self::INR => 17,
        }
    }

    /// Settlement calendar when one is implemented; otherwise weekends only.
    pub const fn market(self) -> Market {
        match self {
            Self::USD => Market::UsSettlement,
            Self::EUR => Market::Target,
            Self::GBP => Market::UkSettlement,
            Self::JPY => Market::Japan,
            _ => Market::WeekendsOnly,
        }
    }

    pub fn calendar(self) -> Calendar {
        Calendar::new(self.market())
    }

    /// Minor units per major unit (100 for most, 1 for JPY and KRW).
    pub const fn minor_units(self) -> u32 {
        match self {
            Self::JPY | Self::KRW => 1,
            _ => 100,
        }
    }

    /// Currencies that are usually non-deliverable offshore.
    pub const fn is_non_deliverable(self) -> bool {
        matches!(self, Self::BRL | Self::CNY | Self::KRW | Self::INR)
    }
}

impl fmt::Display for Currency {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

impl FromStr for Currency {
    type Err = CurrencyError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s.trim().to_ascii_uppercase().as_str() {
            "EUR" => Self::EUR,
            "GBP" => Self::GBP,
            "AUD" => Self::AUD,
            "NZD" => Self::NZD,
            "USD" => Self::USD,
            "CAD" => Self::CAD,
            "CHF" => Self::CHF,
            "JPY" => Self::JPY,
            "SEK" => Self::SEK,
            "NOK" => Self::NOK,
            "DKK" => Self::DKK,
            "MXN" => Self::MXN,
            "BRL" => Self::BRL,
            "TRY" => Self::TRY,
            "RUB" => Self::RUB,
            "CNY" => Self::CNY,
            "KRW" => Self::KRW,
            "INR" => Self::INR,
            _ => return Err(CurrencyError::UnknownCurrency(s.to_owned())),
        })
    }
}

/// A currency pair in market order, quoted as quote-per-base.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CurrencyPair {
    base: Currency,
    quote: Currency,
}

impl CurrencyPair {
    /// Builds the pair in market order whatever order the arguments come in.
    pub fn new(a: Currency, b: Currency) -> Result<Self, CurrencyError> {
        if a == b {
            return Err(CurrencyError::SameCurrency(a));
        }
        let (base, quote) = if a.priority() <= b.priority() { (a, b) } else { (b, a) };
        Ok(Self { base, quote })
    }

    /// Builds the pair exactly as given (for non-standard quotes).
    pub fn direct(base: Currency, quote: Currency) -> Result<Self, CurrencyError> {
        if base == quote {
            return Err(CurrencyError::SameCurrency(base));
        }
        Ok(Self { base, quote })
    }

    pub const fn base(self) -> Currency {
        self.base
    }

    pub const fn quote(self) -> Currency {
        self.quote
    }

    pub fn inverse(self) -> Self {
        Self { base: self.quote, quote: self.base }
    }

    pub fn contains(self, ccy: Currency) -> bool {
        self.base == ccy || self.quote == ccy
    }

    /// True when the pair is in interbank market order.
    pub fn is_market_order(self) -> bool {
        self.base.priority() < self.quote.priority()
    }

    /// Spot lag in business days: 1 for USDCAD, USDTRY, USDRUB; 2 otherwise.
    pub fn spot_lag(self) -> i32 {
        let has_usd = self.contains(Currency::USD);
        if has_usd && (self.contains(Currency::CAD) || self.contains(Currency::TRY) || self.contains(Currency::RUB)) { 1 } else { 2 }
    }

    /// Size of one pip in rate units: 0.01 for JPY-quoted pairs, 0.0001 otherwise.
    pub const fn pip(self) -> f64 {
        match self.quote {
            Currency::JPY | Currency::KRW => 0.01,
            _ => 0.0001,
        }
    }

    /// Joint settlement calendar of both currencies.
    pub fn calendar(self) -> Calendar {
        Calendar::joint([self.base.market(), self.quote.market()], JoinRule::Holidays)
    }

    /// Joint calendar including USD, which settles every USD-cross.
    pub fn settlement_calendar(self) -> Calendar {
        Calendar::joint([self.base.market(), self.quote.market(), Currency::USD.market()], JoinRule::Holidays)
    }

    /// Calendar used to count the spot lag: the non-USD currencies of the
    /// pair. A USD holiday on an intermediate day does not delay spot (the
    /// usual "US holiday on T+1" rule); a holiday in either currency or in
    /// USD on the candidate spot date rolls it forward.
    pub fn lag_calendar(self) -> Calendar {
        let non_usd: Vec<Market> = [self.base, self.quote].into_iter().filter(|c| *c != Currency::USD).map(Currency::market).collect();
        Calendar::joint(non_usd, JoinRule::Holidays)
    }

    /// Spot date: `spot_lag` business days on the lag calendar, then rolled
    /// forward until it is a business day in both currencies and in USD.
    pub fn spot_date(self, trade_date: Date) -> Result<Date, CurrencyError> {
        let candidate = self.lag_calendar().advance(trade_date, self.spot_lag())?;
        Ok(self.settlement_calendar().adjust(candidate, BusinessDayConvention::Following)?)
    }

    /// Value date of an outright forward: spot plus `tenor`, modified following
    /// on the settlement calendar, with the end-of-month rule (a spot date on
    /// the last business day of its month delivers on the last business day).
    pub fn forward_date(self, trade_date: Date, tenor: Period) -> Result<Date, CurrencyError> {
        let spot = self.spot_date(trade_date)?;
        let cal = self.settlement_calendar();
        if tenor.in_days().is_some() {
            // Short dates (days/weeks) count calendar days from spot, then adjust.
            return Ok(cal.adjust(tenor.add_to(spot)?, BusinessDayConvention::Following)?);
        }
        let spot_is_eom = cal.adjust(spot.checked_add_days(1)?, BusinessDayConvention::Following)?.month() != spot.month();
        let raw = tenor.add_to(spot)?;
        let value = if spot_is_eom {
            cal.adjust(raw.end_of_month(), BusinessDayConvention::Preceding)?
        } else {
            cal.adjust(raw, BusinessDayConvention::ModifiedFollowing)?
        };
        Ok(value)
    }

    /// Converts a rate on this pair to the inverse pair.
    pub fn invert_rate(rate: f64) -> f64 {
        1.0 / rate
    }
}

impl fmt::Display for CurrencyPair {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}", self.base, self.quote)
    }
}

/// "EURUSD", "EUR/USD", "eur-usd" all parse; order is kept as written.
impl FromStr for CurrencyPair {
    type Err = CurrencyError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let cleaned: String = s.chars().filter(|c| c.is_ascii_alphabetic()).collect();
        if cleaned.len() != 6 {
            return Err(CurrencyError::BadPair(s.to_owned()));
        }
        let base: Currency = cleaned[..3].parse().map_err(|_| CurrencyError::BadPair(s.to_owned()))?;
        let quote: Currency = cleaned[3..].parse().map_err(|_| CurrencyError::BadPair(s.to_owned()))?;
        Self::direct(base, quote)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Currency::*;

    type R = Result<(), CurrencyError>;

    fn d(y: i32, m: u32, dd: u32) -> Date {
        Date::from_ymd(y, m, dd).expect("valid test date")
    }

    #[test]
    fn market_order_and_parsing() -> R {
        assert_eq!(CurrencyPair::new(USD, EUR)?.to_string(), "EURUSD");
        assert_eq!(CurrencyPair::new(JPY, USD)?.to_string(), "USDJPY");
        assert_eq!(CurrencyPair::new(GBP, EUR)?.to_string(), "EURGBP");
        assert_eq!("usd/jpy".parse::<CurrencyPair>()?.to_string(), "USDJPY");
        assert_eq!("JPYUSD".parse::<CurrencyPair>()?.to_string(), "JPYUSD");
        assert!(!"JPYUSD".parse::<CurrencyPair>()?.is_market_order());
        assert!(matches!("EUREUR".parse::<CurrencyPair>(), Err(CurrencyError::SameCurrency(EUR))));
        assert!(matches!("EURUS".parse::<CurrencyPair>(), Err(CurrencyError::BadPair(_))));
        assert!(matches!("XXX".parse::<Currency>(), Err(CurrencyError::UnknownCurrency(_))));
        Ok(())
    }

    #[test]
    fn conventions() -> R {
        let eurusd: CurrencyPair = "EURUSD".parse()?;
        let usdjpy: CurrencyPair = "USDJPY".parse()?;
        let usdcad: CurrencyPair = "USDCAD".parse()?;
        assert_eq!((eurusd.spot_lag(), usdjpy.spot_lag(), usdcad.spot_lag()), (2, 2, 1));
        assert!((eurusd.pip() - 1e-4).abs() < 1e-15 && (usdjpy.pip() - 1e-2).abs() < 1e-15);
        assert_eq!(eurusd.calendar().to_string(), "TARGET+USNY");
        assert_eq!(usdjpy.settlement_calendar().to_string(), "USNY+JPTO");
        assert_eq!("EURGBP".parse::<CurrencyPair>()?.settlement_calendar().to_string(), "TARGET+UKLN+USNY");
        assert_eq!(eurusd.inverse().to_string(), "USDEUR");
        assert!((CurrencyPair::invert_rate(1.25) - 0.8).abs() < 1e-15);
        Ok(())
    }

    #[test]
    fn spot_dates() -> R {
        let eurusd: CurrencyPair = "EURUSD".parse()?;
        assert_eq!(eurusd.spot_date(d(2026, 10, 13))?, d(2026, 10, 15));
        // Thursday trade: Mon spot. Friday trade: Tue spot.
        assert_eq!(eurusd.spot_date(d(2026, 10, 15))?, d(2026, 10, 19));
        assert_eq!(eurusd.spot_date(d(2026, 10, 16))?, d(2026, 10, 20));
        // US holiday on T+1 (Columbus Day, Mon 12 Oct 2026) is not a TARGET holiday: T+2 counts it.
        assert_eq!(eurusd.spot_date(d(2026, 10, 9))?, d(2026, 10, 13));
        // Good Friday and Easter Monday are TARGET holidays: Thu 2 Apr trade -> T+1 Tue 7 Apr -> spot Wed 8 Apr.
        assert_eq!(eurusd.spot_date(d(2026, 4, 2))?, d(2026, 4, 8));
        assert_eq!(eurusd.lag_calendar().to_string(), "TARGET");
        assert_eq!("EURGBP".parse::<CurrencyPair>()?.lag_calendar().to_string(), "TARGET+UKLN");
        // USDCAD T+1; USD holiday on the spot date rolls it.
        let usdcad: CurrencyPair = "USDCAD".parse()?;
        assert_eq!(usdcad.spot_date(d(2026, 10, 13))?, d(2026, 10, 14));
        assert_eq!(usdcad.spot_date(d(2026, 7, 2))?, d(2026, 7, 6)); // Fri 3 Jul is the US holiday
        // EURGBP: a USD holiday on the candidate spot date rolls it even though neither leg is USD.
        let eurgbp: CurrencyPair = "EURGBP".parse()?;
        assert_eq!(eurgbp.spot_date(d(2026, 7, 1))?, d(2026, 7, 6));
        Ok(())
    }

    #[test]
    fn forward_dates() -> R {
        let eurusd: CurrencyPair = "EURUSD".parse()?;
        let trade = d(2026, 10, 13); // spot 15 Oct
        assert_eq!(eurusd.forward_date(trade, Period::weeks(1))?, d(2026, 10, 22));
        assert_eq!(eurusd.forward_date(trade, Period::months(1))?, d(2026, 11, 16)); // 15 Nov is Sunday
        assert_eq!(eurusd.forward_date(trade, Period::months(3))?, d(2027, 1, 15));
        assert_eq!(eurusd.forward_date(trade, Period::years(1))?, d(2027, 10, 15));
        // End-of-month rule: spot on 30 Apr 2026 (last business day) -> 1M on 29 May (Fri; 31 May is Sunday).
        let trade_eom = d(2026, 4, 28);
        assert_eq!(eurusd.spot_date(trade_eom)?, d(2026, 4, 30));
        assert_eq!(eurusd.forward_date(trade_eom, Period::months(1))?, d(2026, 5, 29));
        assert_eq!(eurusd.forward_date(trade_eom, Period::months(2))?, d(2026, 6, 30));
        Ok(())
    }
}
