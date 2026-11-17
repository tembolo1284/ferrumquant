//! Identifiers for what is being delivered, where, and when.
//!
//! Commodity curves are indexed by *delivery* rather than by a time-to-
//! maturity axis: a Henry Hub Jan-27 contract is a different instrument from
//! Feb-27 even though the two expire two business days apart, because they
//! deliver a different month of gas. [`ContractMonth`] is that key.
//! [`DeliveryPeriod`] is the concrete date range (a calendar month for most
//! futures; a day, week or quarter for OTC blocks), and the sector crates
//! layer their own shaping (peak/off-peak hours, daily gas days) on top.

use std::fmt;

use fq_core::time::{Date, DateError};

use crate::error::CommodError;

/// Physical unit the price is quoted per.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Unit {
    /// Natural gas (NYMEX NG, most North American hubs).
    MmBtu,
    /// Power.
    MWh,
    /// Crude and most refined products (WTI, Brent).
    Barrel,
    /// RBOB / heating oil are quoted per US gallon.
    Gallon,
    /// Precious metals.
    TroyOunce,
    /// Base metals (LME), coal, some ags.
    MetricTon,
    /// Grains (corn, wheat, soybeans).
    Bushel,
    /// Softs and livestock (coffee, sugar, cotton, cattle, hogs).
    Pound,
    /// Emissions allowances (EUA, CCA): one tonne of CO2e.
    Allowance,
}

impl Unit {
    pub const fn code(self) -> &'static str {
        match self {
            Self::MmBtu => "MMBtu",
            Self::MWh => "MWh",
            Self::Barrel => "bbl",
            Self::Gallon => "gal",
            Self::TroyOunce => "ozt",
            Self::MetricTon => "t",
            Self::Bushel => "bu",
            Self::Pound => "lb",
            Self::Allowance => "tCO2e",
        }
    }
}

/// Broad commodity sector. Sector crates refine this with their own product
/// enums; the core only needs it for sanity checks (you cannot spread Henry
/// Hub against gold without saying what you mean).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Commodity {
    NaturalGas,
    Power,
    Crude,
    RefinedProduct,
    Coal,
    Emissions,
    PreciousMetal,
    BaseMetal,
    Grain,
    Soft,
    Livestock,
}

/// A pricing location: a gas hub, a power node or zone, a crude delivery
/// point, an LME warehouse region. Codes are free-form so sector crates can
/// carry ISO node names, pipeline receipt points and the like without the
/// core having to know them.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Hub {
    code: String,
    commodity: Commodity,
    unit: Unit,
}

impl Hub {
    pub fn new(code: impl Into<String>, commodity: Commodity, unit: Unit) -> Self {
        Self { code: code.into(), commodity, unit }
    }

    pub fn code(&self) -> &str {
        &self.code
    }

    pub fn commodity(&self) -> Commodity {
        self.commodity
    }

    pub fn unit(&self) -> Unit {
        self.unit
    }
}

impl fmt::Display for Hub {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({:?}/{})", self.code, self.commodity, self.unit.code())
    }
}

/// A delivery month, the natural key of a futures curve. Ordered
/// chronologically.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ContractMonth {
    year: i32,
    month: u32,
}

impl ContractMonth {
    pub fn new(year: i32, month: u32) -> Result<Self, CommodError> {
        if !(1..=12).contains(&month) {
            return Err(CommodError::InvalidMonth(month));
        }
        Ok(Self { year, month })
    }

    /// The month containing `d`.
    pub fn of(d: Date) -> Self {
        Self { year: d.year(), month: d.month() }
    }

    pub fn year(self) -> i32 {
        self.year
    }

    pub fn month(self) -> u32 {
        self.month
    }

    /// Months since year 0, used for arithmetic and as an interpolation axis.
    pub fn index(self) -> i32 {
        self.year * 12 + (self.month as i32 - 1)
    }

    pub fn from_index(i: i32) -> Self {
        Self { year: i.div_euclid(12), month: (i.rem_euclid(12) + 1) as u32 }
    }

    pub fn add_months(self, n: i32) -> Self {
        Self::from_index(self.index() + n)
    }

    pub fn next(self) -> Self {
        self.add_months(1)
    }

    pub fn prev(self) -> Self {
        self.add_months(-1)
    }

    /// Whole months from `self` to `other` (negative if `other` is earlier).
    pub fn months_until(self, other: Self) -> i32 {
        other.index() - self.index()
    }

    pub fn first_day(self) -> Result<Date, DateError> {
        Date::from_ymd(self.year, self.month, 1)
    }

    pub fn last_day(self) -> Result<Date, DateError> {
        Ok(self.first_day()?.end_of_month())
    }

    /// The calendar-month delivery period.
    pub fn delivery(self) -> Result<DeliveryPeriod, CommodError> {
        DeliveryPeriod::new(self.first_day()?, self.last_day()?)
    }

    /// Futures month code (F=Jan ... Z=Dec), e.g. "NGF27" would be
    /// `format!("NG{}", cm.code())`.
    pub fn code(self) -> String {
        const CODES: [char; 12] = ['F', 'G', 'H', 'J', 'K', 'M', 'N', 'Q', 'U', 'V', 'X', 'Z'];
        format!("{}{}", CODES[(self.month - 1) as usize], self.year.rem_euclid(100))
    }

    /// Inclusive range of months.
    pub fn range(from: Self, to: Self) -> Vec<Self> {
        if to < from {
            return Vec::new();
        }
        (from.index()..=to.index()).map(Self::from_index).collect()
    }

    /// Northern-hemisphere gas convention: Nov-Mar is winter.
    pub fn is_winter(self) -> bool {
        matches!(self.month, 11 | 12 | 1 | 2 | 3)
    }
}

impl fmt::Display for ContractMonth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        const NAMES: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
        write!(f, "{}-{}", NAMES[(self.month - 1) as usize], self.year)
    }
}

/// An inclusive date range over which a commodity is delivered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DeliveryPeriod {
    start: Date,
    end: Date,
}

impl DeliveryPeriod {
    pub fn new(start: Date, end: Date) -> Result<Self, CommodError> {
        if end < start {
            return Err(CommodError::Invalid(format!("delivery end {end} is before start {start}")));
        }
        Ok(Self { start, end })
    }

    pub fn day(d: Date) -> Self {
        Self { start: d, end: d }
    }

    pub fn start(self) -> Date {
        self.start
    }

    pub fn end(self) -> Date {
        self.end
    }

    /// Calendar days delivered, inclusive of both ends.
    pub fn days(self) -> i32 {
        self.end.serial() - self.start.serial() + 1
    }

    pub fn contains(self, d: Date) -> bool {
        self.start <= d && d <= self.end
    }

    /// Delivery months touched by the period, in order.
    pub fn months(self) -> Vec<ContractMonth> {
        ContractMonth::range(ContractMonth::of(self.start), ContractMonth::of(self.end))
    }

    /// Fraction of the period (by calendar days) falling in each touched
    /// month. Used to weight a strip price out of monthly futures.
    pub fn month_weights(self) -> Vec<(ContractMonth, f64)> {
        let total = f64::from(self.days());
        self.months()
            .into_iter()
            .map(|m| {
                let (ms, me) = match (m.first_day(), m.last_day()) {
                    (Ok(a), Ok(b)) => (a, b),
                    _ => return (m, 0.0),
                };
                let lo = if ms > self.start { ms } else { self.start };
                let hi = if me < self.end { me } else { self.end };
                let days = (hi.serial() - lo.serial() + 1).max(0);
                (m, f64::from(days) / total)
            })
            .collect()
    }
}

impl fmt::Display for DeliveryPeriod {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}..={}", self.start, self.end)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_abs_diff_eq;

    fn ymd(y: i32, m: u32, d: u32) -> Date {
        Date::from_ymd(y, m, d).expect("valid date")
    }

    #[test]
    fn month_arithmetic_wraps_years() {
        let dec = ContractMonth::new(2026, 12).expect("month");
        assert_eq!(dec.next(), ContractMonth::new(2027, 1).expect("month"));
        assert_eq!(dec.add_months(-12), ContractMonth::new(2025, 12).expect("month"));
        assert_eq!(ContractMonth::from_index(dec.index()), dec);
        assert_eq!(dec.months_until(dec.add_months(14)), 14);
        assert_eq!(dec.code(), "Z26");
        assert_eq!(dec.to_string(), "Dec-2026");
        assert!(ContractMonth::new(2026, 13).is_err());
    }

    #[test]
    fn range_is_inclusive_and_ordered() {
        let a = ContractMonth::new(2026, 11).expect("month");
        let b = ContractMonth::new(2027, 3).expect("month");
        let r = ContractMonth::range(a, b);
        assert_eq!(r.len(), 5);
        assert!(r.iter().all(|m| m.is_winter()));
        assert!(ContractMonth::range(b, a).is_empty());
    }

    #[test]
    fn delivery_period_weights_split_across_months() {
        let p = DeliveryPeriod::new(ymd(2027, 1, 16), ymd(2027, 2, 14)).expect("period");
        assert_eq!(p.days(), 30);
        let w = p.month_weights();
        assert_eq!(w.len(), 2);
        assert_abs_diff_eq!(w[0].1, 16.0 / 30.0, epsilon = 1e-12);
        assert_abs_diff_eq!(w[1].1, 14.0 / 30.0, epsilon = 1e-12);
        assert!(p.contains(ymd(2027, 2, 1)));
        assert!(!p.contains(ymd(2027, 2, 15)));
        assert!(DeliveryPeriod::new(ymd(2027, 2, 1), ymd(2027, 1, 1)).is_err());
    }

    #[test]
    fn calendar_month_delivery() {
        let m = ContractMonth::new(2028, 2).expect("month");
        let d = m.delivery().expect("delivery");
        assert_eq!(d.start(), ymd(2028, 2, 1));
        assert_eq!(d.end(), ymd(2028, 2, 29));
        assert_eq!(d.months(), vec![m]);
    }
}
