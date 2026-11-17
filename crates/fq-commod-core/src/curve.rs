//! Futures / forward curves keyed by delivery month, and basis curves.
//!
//! A [`FuturesCurve`] is a snapshot of settlement prices for a strip of
//! contract months at one hub. Unlike a rates curve there is no no-arbitrage
//! link between adjacent months for a non-storable (power, livestock) or
//! constrained-storage (gas) commodity, so the curve is a *lookup* with a
//! chosen interpolation for months that were not quoted, not a bootstrap.
//!
//! Prices are per [`Unit`] of the hub. Bumping is by value: `shifted`,
//! `scaled` and `bumped_month` return new curves, which is what a risk
//! engine wants for parallel and bucketed deltas.

use fq_core::time::Date;

use crate::contract::{ContractMonth, DeliveryPeriod, Hub};
use crate::error::CommodError;

/// How to fill months that were not quoted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CurveInterpolation {
    /// Linear in the contract-month index between quoted months. Reasonable
    /// for crude, metals and anything with a smooth term structure.
    #[default]
    Linear,
    /// Flat from the previous quoted month. Safer for gas and power where
    /// seasonality makes linear fills meaningless across a shoulder.
    PreviousFlat,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FuturesQuote {
    pub month: ContractMonth,
    pub price: f64,
}

impl FuturesQuote {
    pub fn new(month: ContractMonth, price: f64) -> Self {
        Self { month, price }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct FuturesCurve {
    hub: Hub,
    reference: Date,
    quotes: Vec<FuturesQuote>,
    interpolation: CurveInterpolation,
}

impl FuturesCurve {
    /// `quotes` must be strictly increasing by month with positive prices.
    /// (Negative prices do happen, famously WTI May-20 and routinely at
    /// Waha and in ERCOT real-time; those live in sector models with a
    /// shift, not in the lognormal-friendly core curve.)
    pub fn new(hub: Hub, reference: Date, quotes: Vec<FuturesQuote>, interpolation: CurveInterpolation) -> Result<Self, CommodError> {
        if quotes.is_empty() {
            return Err(CommodError::NoQuotes);
        }
        for (i, q) in quotes.iter().enumerate() {
            if !(q.price.is_finite() && q.price > 0.0) {
                return Err(CommodError::NonPositivePrice { month: q.month, price: q.price });
            }
            if i > 0 && quotes[i - 1].month >= q.month {
                return Err(CommodError::UnsortedQuotes(q.month));
            }
        }
        Ok(Self { hub, reference, quotes, interpolation })
    }

    pub fn hub(&self) -> &Hub {
        &self.hub
    }

    pub fn reference_date(&self) -> Date {
        self.reference
    }

    pub fn quotes(&self) -> &[FuturesQuote] {
        &self.quotes
    }

    pub fn interpolation(&self) -> CurveInterpolation {
        self.interpolation
    }

    pub fn first_month(&self) -> ContractMonth {
        self.quotes[0].month
    }

    pub fn last_month(&self) -> ContractMonth {
        self.quotes[self.quotes.len() - 1].month
    }

    fn range_error(&self, month: ContractMonth) -> CommodError {
        CommodError::OutOfRange { month, first: self.first_month(), last: self.last_month() }
    }

    /// Price for a delivery month, interpolated if the month was not quoted.
    /// No extrapolation: months outside the quoted range are an error.
    pub fn price(&self, month: ContractMonth) -> Result<f64, CommodError> {
        if month < self.first_month() || month > self.last_month() {
            return Err(self.range_error(month));
        }
        let idx = self.quotes.partition_point(|q| q.month < month);
        let q = &self.quotes[idx];
        if q.month == month {
            return Ok(q.price);
        }
        // Strictly between quotes[idx-1] and quotes[idx].
        let prev = &self.quotes[idx - 1];
        Ok(match self.interpolation {
            CurveInterpolation::PreviousFlat => prev.price,
            CurveInterpolation::Linear => {
                let w = f64::from(prev.month.months_until(month)) / f64::from(prev.month.months_until(q.month));
                prev.price + w * (q.price - prev.price)
            }
        })
    }

    /// The first contract month delivering on or after `d` (the "prompt"
    /// month if `d` is the valuation date and the front contract has not
    /// yet expired). Sector crates refine this with last-trade rules.
    pub fn front_month(&self, d: Date) -> Result<ContractMonth, CommodError> {
        let m = ContractMonth::of(d);
        let candidate = if d.day() == 1 { m } else { m.next() };
        let candidate = if candidate < self.first_month() { self.first_month() } else { candidate };
        if candidate > self.last_month() { Err(CommodError::NoFrontMonth(d)) } else { Ok(candidate) }
    }

    /// Day-weighted average price over a delivery period, e.g. a Cal-27
    /// strip or a Q1 block. This is the flat price of a swap that settles
    /// each month against that month's future.
    pub fn strip_price(&self, period: DeliveryPeriod) -> Result<f64, CommodError> {
        let mut total = 0.0;
        for (m, w) in period.month_weights() {
            total += w * self.price(m)?;
        }
        Ok(total)
    }

    /// Simple average of the quoted prices for an inclusive month range.
    /// Equal-weighted, which is how exchange strip/"Cal" quotes are built.
    pub fn average(&self, from: ContractMonth, to: ContractMonth) -> Result<f64, CommodError> {
        let months = ContractMonth::range(from, to);
        if months.is_empty() {
            return Err(CommodError::Invalid(format!("empty month range {from}..={to}")));
        }
        let mut sum = 0.0;
        for m in &months {
            sum += self.price(*m)?;
        }
        Ok(sum / months.len() as f64)
    }

    pub fn calendar_spread(&self, near: ContractMonth, far: ContractMonth) -> Result<CalendarSpread, CommodError> {
        Ok(CalendarSpread { near, far, spread: self.price(far)? - self.price(near)? })
    }

    /// Parallel bump by an absolute amount per unit.
    pub fn shifted(&self, bump: f64) -> Result<Self, CommodError> {
        let quotes = self.quotes.iter().map(|q| FuturesQuote::new(q.month, q.price + bump)).collect();
        Self::new(self.hub.clone(), self.reference, quotes, self.interpolation)
    }

    /// Multiply every price by `factor` (relative bump, e.g. 1.01).
    pub fn scaled(&self, factor: f64) -> Result<Self, CommodError> {
        let quotes = self.quotes.iter().map(|q| FuturesQuote::new(q.month, q.price * factor)).collect();
        Self::new(self.hub.clone(), self.reference, quotes, self.interpolation)
    }

    /// Bump one quoted month (for bucketed delta). The month must be quoted.
    pub fn bumped_month(&self, month: ContractMonth, bump: f64) -> Result<Self, CommodError> {
        let Some(i) = self.quotes.iter().position(|q| q.month == month) else {
            return Err(CommodError::Invalid(format!("{month} is not a quoted month on {}", self.hub)));
        };
        let mut quotes = self.quotes.clone();
        quotes[i].price += bump;
        Self::new(self.hub.clone(), self.reference, quotes, self.interpolation)
    }

    /// Months quoted on both curves, for pairwise operations.
    fn common_months(&self, other: &Self) -> Vec<ContractMonth> {
        self.quotes.iter().map(|q| q.month).filter(|m| other.quotes.iter().any(|o| o.month == *m)).collect()
    }
}

/// Price difference between two delivery months on one curve. Positive is
/// contango (far above near), negative is backwardation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CalendarSpread {
    pub near: ContractMonth,
    pub far: ContractMonth,
    pub spread: f64,
}

impl CalendarSpread {
    pub fn is_contango(&self) -> bool {
        self.spread > 0.0
    }
}

/// Locational basis: `hub - benchmark` by month. For gas this is e.g.
/// Transco Z6 NY minus Henry Hub; for power, a node minus its hub. Basis is
/// quoted and traded in its own right, so it is a first-class curve here
/// rather than something recomputed from two outrights, and it may be
/// negative (it usually is at producing-region hubs).
#[derive(Debug, Clone, PartialEq)]
pub struct BasisCurve {
    hub: Hub,
    benchmark: Hub,
    reference: Date,
    points: Vec<(ContractMonth, f64)>,
}

impl BasisCurve {
    /// Build from quoted basis points directly (how the market quotes them).
    pub fn new(hub: Hub, benchmark: Hub, reference: Date, mut points: Vec<(ContractMonth, f64)>) -> Result<Self, CommodError> {
        if points.is_empty() {
            return Err(CommodError::NoQuotes);
        }
        if hub.unit() != benchmark.unit() {
            return Err(CommodError::CurveMismatch(hub.to_string(), benchmark.to_string()));
        }
        points.sort_by_key(|(m, _)| *m);
        for w in points.windows(2) {
            if w[0].0 == w[1].0 {
                return Err(CommodError::UnsortedQuotes(w[1].0));
            }
        }
        if let Some((m, v)) = points.iter().find(|(_, v)| !v.is_finite()) {
            return Err(CommodError::Invalid(format!("non-finite basis {v} at {m}")));
        }
        Ok(Self { hub, benchmark, reference, points })
    }

    /// Imply basis from two outright curves on their common months.
    pub fn from_outrights(hub: &FuturesCurve, benchmark: &FuturesCurve) -> Result<Self, CommodError> {
        if hub.reference_date() != benchmark.reference_date() || hub.hub().unit() != benchmark.hub().unit() {
            return Err(CommodError::CurveMismatch(hub.hub().to_string(), benchmark.hub().to_string()));
        }
        let mut points = Vec::new();
        for m in hub.common_months(benchmark) {
            points.push((m, hub.price(m)? - benchmark.price(m)?));
        }
        Self::new(hub.hub().clone(), benchmark.hub().clone(), hub.reference_date(), points)
    }

    pub fn hub(&self) -> &Hub {
        &self.hub
    }

    pub fn benchmark(&self) -> &Hub {
        &self.benchmark
    }

    pub fn reference_date(&self) -> Date {
        self.reference
    }

    pub fn points(&self) -> &[(ContractMonth, f64)] {
        &self.points
    }

    /// Basis for a month; linear between quoted months, no extrapolation.
    pub fn basis(&self, month: ContractMonth) -> Result<f64, CommodError> {
        let (first, last) = (self.points[0].0, self.points[self.points.len() - 1].0);
        if month < first || month > last {
            return Err(CommodError::OutOfRange { month, first, last });
        }
        let idx = self.points.partition_point(|(m, _)| *m < month);
        let (m1, b1) = self.points[idx];
        if m1 == month {
            return Ok(b1);
        }
        let (m0, b0) = self.points[idx - 1];
        let w = f64::from(m0.months_until(month)) / f64::from(m0.months_until(m1));
        Ok(b0 + w * (b1 - b0))
    }

    /// Outright hub curve = benchmark + basis, on the benchmark's quoted
    /// months that the basis curve covers.
    pub fn apply_to(&self, benchmark: &FuturesCurve) -> Result<FuturesCurve, CommodError> {
        if benchmark.hub() != &self.benchmark {
            return Err(CommodError::CurveMismatch(benchmark.hub().to_string(), self.benchmark.to_string()));
        }
        let mut quotes = Vec::new();
        for q in benchmark.quotes() {
            if let Ok(b) = self.basis(q.month) {
                quotes.push(FuturesQuote::new(q.month, q.price + b));
            }
        }
        FuturesCurve::new(self.hub.clone(), benchmark.reference_date(), quotes, benchmark.interpolation())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::{Commodity, Unit};
    use approx::assert_abs_diff_eq;

    fn ymd(y: i32, m: u32, d: u32) -> Date {
        Date::from_ymd(y, m, d).expect("valid date")
    }

    fn cm(y: i32, m: u32) -> ContractMonth {
        ContractMonth::new(y, m).expect("month")
    }

    fn henry() -> Hub {
        Hub::new("HH", Commodity::NaturalGas, Unit::MmBtu)
    }

    fn hh_curve(interp: CurveInterpolation) -> FuturesCurve {
        // Winter premium: Dec/Jan/Feb above the shoulders.
        let q = vec![
            FuturesQuote::new(cm(2026, 12), 4.10),
            FuturesQuote::new(cm(2027, 1), 4.40),
            FuturesQuote::new(cm(2027, 2), 4.20),
            FuturesQuote::new(cm(2027, 3), 3.60),
            FuturesQuote::new(cm(2027, 6), 3.00),
        ];
        FuturesCurve::new(henry(), ymd(2026, 11, 9), q, interp).expect("curve")
    }

    #[test]
    fn rejects_bad_quotes() {
        let r = FuturesCurve::new(henry(), ymd(2026, 11, 9), vec![], CurveInterpolation::Linear);
        assert_eq!(r, Err(CommodError::NoQuotes));
        let q = vec![FuturesQuote::new(cm(2027, 1), 4.0), FuturesQuote::new(cm(2027, 1), 4.1)];
        assert!(matches!(FuturesCurve::new(henry(), ymd(2026, 11, 9), q, CurveInterpolation::Linear), Err(CommodError::UnsortedQuotes(_))));
        let q = vec![FuturesQuote::new(cm(2027, 1), -1.0)];
        assert!(matches!(FuturesCurve::new(henry(), ymd(2026, 11, 9), q, CurveInterpolation::Linear), Err(CommodError::NonPositivePrice { .. })));
    }

    #[test]
    fn quoted_and_interpolated_prices() {
        let lin = hh_curve(CurveInterpolation::Linear);
        assert_abs_diff_eq!(lin.price(cm(2027, 1)).expect("p"), 4.40);
        // Apr-27 is 1/3 of the way from Mar (3.60) to Jun (3.00).
        assert_abs_diff_eq!(lin.price(cm(2027, 4)).expect("p"), 3.40, epsilon = 1e-12);
        let flat = hh_curve(CurveInterpolation::PreviousFlat);
        assert_abs_diff_eq!(flat.price(cm(2027, 5)).expect("p"), 3.60);
        assert!(matches!(lin.price(cm(2027, 7)), Err(CommodError::OutOfRange { .. })));
        assert!(matches!(lin.price(cm(2026, 11)), Err(CommodError::OutOfRange { .. })));
    }

    #[test]
    fn front_month_rolls_after_first_of_month() {
        let c = hh_curve(CurveInterpolation::Linear);
        assert_eq!(c.front_month(ymd(2026, 11, 9)).expect("front"), cm(2026, 12));
        assert_eq!(c.front_month(ymd(2027, 1, 1)).expect("front"), cm(2027, 1));
        assert_eq!(c.front_month(ymd(2027, 1, 2)).expect("front"), cm(2027, 2));
        assert!(matches!(c.front_month(ymd(2027, 6, 15)), Err(CommodError::NoFrontMonth(_))));
    }

    #[test]
    fn strip_and_average() {
        let c = hh_curve(CurveInterpolation::Linear);
        let q1 = DeliveryPeriod::new(ymd(2027, 1, 1), ymd(2027, 3, 31)).expect("q1");
        // Day-weighted: Jan 31d, Feb 28d, Mar 31d.
        let expected = (31.0 * 4.40 + 28.0 * 4.20 + 31.0 * 3.60) / 90.0;
        assert_abs_diff_eq!(c.strip_price(q1).expect("strip"), expected, epsilon = 1e-12);
        assert_abs_diff_eq!(c.average(cm(2027, 1), cm(2027, 3)).expect("avg"), (4.40 + 4.20 + 3.60) / 3.0, epsilon = 1e-12);
        let s = c.calendar_spread(cm(2027, 3), cm(2027, 6)).expect("spread");
        assert_abs_diff_eq!(s.spread, -0.60, epsilon = 1e-12);
        assert!(!s.is_contango());
    }

    #[test]
    fn bumps_are_by_value() {
        let c = hh_curve(CurveInterpolation::Linear);
        let up = c.shifted(0.10).expect("shift");
        assert_abs_diff_eq!(up.price(cm(2027, 1)).expect("p") - c.price(cm(2027, 1)).expect("p"), 0.10, epsilon = 1e-12);
        let one = c.bumped_month(cm(2027, 1), 0.01).expect("bump");
        assert_abs_diff_eq!(one.price(cm(2027, 1)).expect("p"), 4.41, epsilon = 1e-12);
        assert_abs_diff_eq!(one.price(cm(2027, 2)).expect("p"), 4.20, epsilon = 1e-12);
        assert!(c.bumped_month(cm(2027, 4), 0.01).is_err());
        let sc = c.scaled(1.10).expect("scale");
        assert_abs_diff_eq!(sc.price(cm(2027, 6)).expect("p"), 3.30, epsilon = 1e-12);
    }

    #[test]
    fn basis_round_trips_outrights() {
        let hh = hh_curve(CurveInterpolation::Linear);
        let z6 = Hub::new("TZ6NY", Commodity::NaturalGas, Unit::MmBtu);
        // NY winter basis blows out, summer goes negative-ish.
        let q = vec![
            FuturesQuote::new(cm(2026, 12), 6.10),
            FuturesQuote::new(cm(2027, 1), 9.40),
            FuturesQuote::new(cm(2027, 2), 7.20),
            FuturesQuote::new(cm(2027, 3), 3.90),
            FuturesQuote::new(cm(2027, 6), 2.50),
        ];
        let z6c = FuturesCurve::new(z6.clone(), ymd(2026, 11, 9), q, CurveInterpolation::Linear).expect("z6");
        let basis = BasisCurve::from_outrights(&z6c, &hh).expect("basis");
        assert_abs_diff_eq!(basis.basis(cm(2027, 1)).expect("b"), 5.00, epsilon = 1e-12);
        assert_abs_diff_eq!(basis.basis(cm(2027, 6)).expect("b"), -0.50, epsilon = 1e-12);
        // Apr-27 basis interpolates linearly between Mar (0.30) and Jun (-0.50).
        assert_abs_diff_eq!(basis.basis(cm(2027, 4)).expect("b"), 0.30 - 0.80 / 3.0, epsilon = 1e-12);
        let rebuilt = basis.apply_to(&hh).expect("rebuild");
        assert_eq!(rebuilt, z6c);
        // Unit mismatch is refused.
        let pjm = Hub::new("PJMW", Commodity::Power, Unit::MWh);
        assert!(matches!(BasisCurve::new(pjm, henry(), ymd(2026, 11, 9), vec![(cm(2027, 1), 1.0)]), Err(CommodError::CurveMismatch(..))));
    }
}
