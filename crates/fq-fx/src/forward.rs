//! FX market snapshot, outright forwards and non-deliverable forwards.
//!
//! Covered interest parity on two discount curves referenced at the trade
//! date: for a value date `T` and spot date `s`,
//!
//! ```text
//! F(T) = S * [DF_b(T) / DF_b(s)] / [DF_q(T) / DF_q(s)]
//! ```
//!
//! where `b` is the base currency (one unit of which the rate prices) and
//! `q` the quote currency: holding the base earns its rate, so the higher
//! the quote-currency rate the higher the forward. Forward points are `(F - S) / pip`. A forward
//! bought at strike `K` is worth `N_b (F - K) DF_q(T)` in quote currency.
//! The curves are plain `DiscountCurve`s: a cross-currency basis is carried
//! by whichever curve was bootstrapped from FX swaps, see
//! `FxMarket::implied_quote_curve`.

use thiserror::Error;

use fq_core::time::{BusinessDayConvention, Date, DateError, Period, PeriodError};
use fq_rates::curve::{CurveError, DiscountCurve, Interpolation};

use crate::currency::{Currency, CurrencyError, CurrencyPair};

#[derive(Debug, Error, Clone, PartialEq)]
pub enum FxError {
    #[error(transparent)]
    Currency(#[from] CurrencyError),
    #[error(transparent)]
    Curve(#[from] CurveError),
    #[error(transparent)]
    Date(#[from] DateError),
    #[error(transparent)]
    Period(#[from] PeriodError),
    #[error("spot {0} must be positive")]
    NonPositiveSpot(f64),
    #[error("{which} curve is referenced at {reference}, expected the trade date {trade_date}")]
    CurveReference { which: &'static str, reference: Date, trade_date: Date },
    #[error("value date {value_date} is before spot {spot}")]
    ValueBeforeSpot { value_date: Date, spot: Date },
    #[error("settlement {settlement} is before fixing {fixing}")]
    SettlementBeforeFixing { fixing: Date, settlement: Date },
    #[error("notional must be positive, got {0}")]
    NonPositiveNotional(f64),
    #[error("currency {0} is not part of {1}")]
    CurrencyNotInPair(Currency, CurrencyPair),
    #[error("need at least one forward point quote")]
    NoQuotes,
}

/// Spot and the two discount curves for a pair, as of one trade date.
#[derive(Debug, Clone, PartialEq)]
pub struct FxMarket {
    pair: CurrencyPair,
    trade_date: Date,
    spot_date: Date,
    spot: f64,
    base_curve: DiscountCurve,
    quote_curve: DiscountCurve,
}

impl FxMarket {
    /// Curves must be referenced at `trade_date`; `spot` is the rate for the
    /// pair's spot date.
    pub fn new(pair: CurrencyPair, trade_date: Date, spot: f64, base_curve: DiscountCurve, quote_curve: DiscountCurve) -> Result<Self, FxError> {
        if spot <= 0.0 || !spot.is_finite() {
            return Err(FxError::NonPositiveSpot(spot));
        }
        for (which, c) in [("base", &base_curve), ("quote", &quote_curve)] {
            if c.reference_date() != trade_date {
                return Err(FxError::CurveReference { which, reference: c.reference_date(), trade_date });
            }
        }
        let spot_date = pair.spot_date(trade_date)?;
        Ok(Self { pair, trade_date, spot_date, spot, base_curve, quote_curve })
    }

    pub const fn pair(&self) -> CurrencyPair {
        self.pair
    }

    pub const fn trade_date(&self) -> Date {
        self.trade_date
    }

    pub const fn spot_date(&self) -> Date {
        self.spot_date
    }

    pub const fn spot(&self) -> f64 {
        self.spot
    }

    pub const fn base_curve(&self) -> &DiscountCurve {
        &self.base_curve
    }

    pub const fn quote_curve(&self) -> &DiscountCurve {
        &self.quote_curve
    }

    /// Same market with a different spot (for spot bumps).
    pub fn with_spot(&self, spot: f64) -> Result<Self, FxError> {
        Self::new(self.pair, self.trade_date, spot, self.base_curve.clone(), self.quote_curve.clone())
    }

    /// Same market with replaced curves (for rate bumps).
    pub fn with_curves(&self, base_curve: DiscountCurve, quote_curve: DiscountCurve) -> Result<Self, FxError> {
        Self::new(self.pair, self.trade_date, self.spot, base_curve, quote_curve)
    }

    /// Discount factor in `ccy` from the trade date.
    pub fn discount(&self, ccy: Currency, d: Date) -> Result<f64, FxError> {
        let curve = self.curve_for(ccy)?;
        Ok(curve.discount(d)?)
    }

    /// Discount factor in `ccy` from the spot date (the FX market's natural numeraire).
    pub fn discount_from_spot(&self, ccy: Currency, d: Date) -> Result<f64, FxError> {
        let curve = self.curve_for(ccy)?;
        Ok(curve.discount(d)? / curve.discount(self.spot_date)?)
    }

    fn curve_for(&self, ccy: Currency) -> Result<&DiscountCurve, FxError> {
        if ccy == self.pair.base() {
            Ok(&self.base_curve)
        } else if ccy == self.pair.quote() {
            Ok(&self.quote_curve)
        } else {
            Err(FxError::CurrencyNotInPair(ccy, self.pair))
        }
    }

    /// Outright forward for a value date on or after spot.
    pub fn forward(&self, value_date: Date) -> Result<f64, FxError> {
        if value_date < self.spot_date {
            return Err(FxError::ValueBeforeSpot { value_date, spot: self.spot_date });
        }
        let dq = self.discount_from_spot(self.pair.quote(), value_date)?;
        let db = self.discount_from_spot(self.pair.base(), value_date)?;
        Ok(self.spot * db / dq)
    }

    /// Forward points in pips for a value date.
    pub fn forward_points(&self, value_date: Date) -> Result<f64, FxError> {
        Ok((self.forward(value_date)? - self.spot) / self.pair.pip())
    }

    /// (value date, outright) for a market tenor from the trade date.
    pub fn forward_for_tenor(&self, tenor: Period) -> Result<(Date, f64), FxError> {
        let value_date = self.pair.forward_date(self.trade_date, tenor)?;
        Ok((value_date, self.forward(value_date)?))
    }

    /// Simple interest differential (quote minus base, ACT/360) implied by a forward.
    pub fn implied_rate_differential(&self, value_date: Date) -> Result<f64, FxError> {
        let tau = f64::from(value_date - self.spot_date) / 360.0;
        if tau <= 0.0 {
            return Err(FxError::ValueBeforeSpot { value_date, spot: self.spot_date });
        }
        Ok((self.forward(value_date)? / self.spot - 1.0) / tau)
    }

    /// Builds the quote-currency curve that reprices a strip of forward
    /// points off the base curve (the FX-swap-implied curve, basis included).
    /// `DF_q(spot)` is taken equal to `DF_b(spot)`: no basis inside the spot
    /// window. Quotes are `(tenor, points in pips)`.
    pub fn implied_quote_curve(
        pair: CurrencyPair,
        trade_date: Date,
        spot: f64,
        base_curve: &DiscountCurve,
        points: &[(Period, f64)],
    ) -> Result<DiscountCurve, FxError> {
        if points.is_empty() {
            return Err(FxError::NoQuotes);
        }
        if base_curve.reference_date() != trade_date {
            return Err(FxError::CurveReference { which: "base", reference: base_curve.reference_date(), trade_date });
        }
        let spot_date = pair.spot_date(trade_date)?;
        let df_b_spot = base_curve.discount(spot_date)?;
        let mut nodes: Vec<(Date, f64)> = Vec::with_capacity(points.len() + 1);
        if spot_date > trade_date {
            nodes.push((spot_date, df_b_spot));
        }
        for &(tenor, pts) in points {
            let value_date = pair.forward_date(trade_date, tenor)?;
            let forward = spot + pts * pair.pip();
            let df_b = base_curve.discount(value_date)? / df_b_spot;
            let df_q = df_b_spot * df_b * spot / forward;
            nodes.push((value_date, df_q));
        }
        nodes.sort_by_key(|(d, _)| *d);
        nodes.dedup_by_key(|(d, _)| *d);
        Ok(DiscountCurve::new(trade_date, &nodes, Interpolation::LogLinearDiscount)?)
    }
}

/// Which way the base currency goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FxDirection {
    /// Receive base, pay quote at the strike.
    BuyBase,
    /// Pay base, receive quote at the strike.
    SellBase,
}

impl FxDirection {
    pub const fn sign(self) -> f64 {
        match self {
            Self::BuyBase => 1.0,
            Self::SellBase => -1.0,
        }
    }
}

/// Valuation of a forward or NDF, from the holder's side.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FxForwardValuation {
    /// Outright forward to the value (or fixing) date.
    pub forward: f64,
    pub forward_points: f64,
    /// PV in quote currency at the trade date.
    pub npv_quote: f64,
    /// PV in base currency at the trade date (`npv_quote / spot`).
    pub npv_base: f64,
    /// dPV_quote / dSpot: forward delta scaled by the carry ratio.
    pub spot_delta: f64,
    /// dPV_quote / dForward: signed notional times the quote discount factor.
    pub forward_delta: f64,
    /// Quote-currency discount factor to the payment date.
    pub discount_quote: f64,
}

/// Deliverable outright forward.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FxForward {
    pair: CurrencyPair,
    direction: FxDirection,
    notional_base: f64,
    strike: f64,
    value_date: Date,
}

impl FxForward {
    pub fn new(pair: CurrencyPair, direction: FxDirection, notional_base: f64, strike: f64, value_date: Date) -> Result<Self, FxError> {
        if notional_base <= 0.0 {
            return Err(FxError::NonPositiveNotional(notional_base));
        }
        if strike <= 0.0 {
            return Err(FxError::NonPositiveSpot(strike));
        }
        Ok(Self { pair, direction, notional_base, strike, value_date })
    }

    /// Forward for a market tenor from the trade date.
    pub fn from_tenor(pair: CurrencyPair, trade_date: Date, tenor: Period, direction: FxDirection, notional_base: f64, strike: f64) -> Result<Self, FxError> {
        Self::new(pair, direction, notional_base, strike, pair.forward_date(trade_date, tenor)?)
    }

    pub const fn pair(&self) -> CurrencyPair {
        self.pair
    }

    pub const fn direction(&self) -> FxDirection {
        self.direction
    }

    pub const fn notional_base(&self) -> f64 {
        self.notional_base
    }

    /// Quote-currency amount exchanged at the value date.
    pub fn notional_quote(&self) -> f64 {
        self.notional_base * self.strike
    }

    pub const fn strike(&self) -> f64 {
        self.strike
    }

    pub const fn value_date(&self) -> Date {
        self.value_date
    }

    pub fn value(&self, market: &FxMarket) -> Result<FxForwardValuation, FxError> {
        let forward = market.forward(self.value_date)?;
        let df_q = market.discount(self.pair.quote(), self.value_date)?;
        let sign = self.direction.sign();
        let forward_delta = sign * self.notional_base * df_q;
        let npv_quote = forward_delta * (forward - self.strike);
        Ok(FxForwardValuation {
            forward,
            forward_points: (forward - market.spot()) / self.pair.pip(),
            npv_quote,
            npv_base: npv_quote / market.spot(),
            spot_delta: forward_delta * forward / market.spot(),
            forward_delta,
            discount_quote: df_q,
        })
    }

    pub fn npv_quote(&self, market: &FxMarket) -> Result<f64, FxError> {
        Ok(self.value(market)?.npv_quote)
    }
}

/// Non-deliverable forward: fixes against the outright on `fixing_date`,
/// cash-settles the difference on `settlement_date` in `settlement_currency`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FxNdf {
    pair: CurrencyPair,
    direction: FxDirection,
    notional_base: f64,
    strike: f64,
    fixing_date: Date,
    settlement_date: Date,
    settlement_currency: Currency,
}

impl FxNdf {
    pub fn new(
        pair: CurrencyPair,
        direction: FxDirection,
        notional_base: f64,
        strike: f64,
        fixing_date: Date,
        settlement_date: Date,
        settlement_currency: Currency,
    ) -> Result<Self, FxError> {
        if notional_base <= 0.0 {
            return Err(FxError::NonPositiveNotional(notional_base));
        }
        if strike <= 0.0 {
            return Err(FxError::NonPositiveSpot(strike));
        }
        if settlement_date < fixing_date {
            return Err(FxError::SettlementBeforeFixing { fixing: fixing_date, settlement: settlement_date });
        }
        if !pair.contains(settlement_currency) {
            return Err(FxError::CurrencyNotInPair(settlement_currency, pair));
        }
        Ok(Self { pair, direction, notional_base, strike, fixing_date, settlement_date, settlement_currency })
    }

    /// Standard NDF: settles on the forward date for `tenor`, fixes `spot_lag`
    /// business days earlier, settlement in the deliverable currency (USD
    /// when the pair has it, otherwise the quote currency).
    pub fn standard(pair: CurrencyPair, trade_date: Date, tenor: Period, direction: FxDirection, notional_base: f64, strike: f64) -> Result<Self, FxError> {
        let settlement = pair.forward_date(trade_date, tenor)?;
        let fixing = Self::fixing_for_settlement(pair, settlement)?;
        let ccy = if pair.contains(Currency::USD) { Currency::USD } else { pair.quote() };
        Self::new(pair, direction, notional_base, strike, fixing, settlement, ccy)
    }

    /// Fixing date: `spot_lag` business days before settlement on the pair's calendar.
    pub fn fixing_for_settlement(pair: CurrencyPair, settlement: Date) -> Result<Date, FxError> {
        let cal = pair.calendar();
        let adjusted = cal.adjust(settlement, BusinessDayConvention::Preceding)?;
        Ok(cal.advance(adjusted, -pair.spot_lag())?)
    }

    pub const fn pair(&self) -> CurrencyPair {
        self.pair
    }

    pub const fn direction(&self) -> FxDirection {
        self.direction
    }

    pub const fn notional_base(&self) -> f64 {
        self.notional_base
    }

    pub const fn strike(&self) -> f64 {
        self.strike
    }

    pub const fn fixing_date(&self) -> Date {
        self.fixing_date
    }

    pub const fn settlement_date(&self) -> Date {
        self.settlement_date
    }

    pub const fn settlement_currency(&self) -> Currency {
        self.settlement_currency
    }

    /// Expected settlement amount in the settlement currency for a given fixing.
    pub fn settlement_amount(&self, fixing: f64) -> f64 {
        let quote_amount = self.direction.sign() * self.notional_base * (fixing - self.strike);
        if self.settlement_currency == self.pair.quote() { quote_amount } else { quote_amount / fixing }
    }

    /// Values with the forward to the fixing date as the expected fixing.
    /// `npv_quote`/`npv_base` are the settlement amount discounted in the
    /// settlement currency and converted at spot where needed.
    pub fn value(&self, market: &FxMarket) -> Result<FxForwardValuation, FxError> {
        let forward = market.forward(self.fixing_date)?;
        let df_settle = market.discount(self.settlement_currency, self.settlement_date)?;
        let sign = self.direction.sign();
        let amount = self.settlement_amount(forward);
        let pv_settle = amount * df_settle;
        let (npv_quote, npv_base) =
            if self.settlement_currency == self.pair.quote() { (pv_settle, pv_settle / market.spot()) } else { (pv_settle * market.spot(), pv_settle) };
        // Quote-settled: PV_q = sN (F - K) DF, linear in F and F is proportional to S.
        // Base-settled: PV_q = sN (1 - K/F) DF S, so dPV_q/dF at fixed S is sN K/F^2 DF S
        // and the total spot delta collapses to sN DF.
        let (forward_delta, spot_delta) = if self.settlement_currency == self.pair.quote() {
            let fd = sign * self.notional_base * df_settle;
            (fd, fd * forward / market.spot())
        } else {
            (sign * self.notional_base * self.strike / (forward * forward) * df_settle * market.spot(), sign * self.notional_base * df_settle)
        };
        Ok(FxForwardValuation {
            forward,
            forward_points: (forward - market.spot()) / self.pair.pip(),
            npv_quote,
            npv_base,
            spot_delta,
            forward_delta,
            discount_quote: market.discount(self.pair.quote(), self.settlement_date)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_abs_diff_eq;

    type R = Result<(), FxError>;

    fn d(y: i32, m: u32, dd: u32) -> Date {
        Date::from_ymd(y, m, dd).expect("valid test date")
    }

    /// EURUSD on 13 Oct 2026: EUR 2%, USD 4% flat continuous.
    fn eurusd() -> Result<FxMarket, FxError> {
        let today = d(2026, 10, 13);
        let eur = DiscountCurve::flat(today, 0.02, d(2040, 1, 1))?;
        let usd = DiscountCurve::flat(today, 0.04, d(2040, 1, 1))?;
        FxMarket::new("EURUSD".parse()?, today, 1.1750, eur, usd)
    }

    #[test]
    fn covered_interest_parity_on_flat_curves() -> R {
        let m = eurusd()?;
        assert_eq!(m.spot_date(), d(2026, 10, 15));
        assert_abs_diff_eq!(m.forward(m.spot_date())?, 1.1750, epsilon = 1e-15);
        let (vd, f) = m.forward_for_tenor(Period::years(1))?;
        assert_eq!(vd, d(2027, 10, 15));
        let t = f64::from(vd - m.spot_date()) / 365.0;
        assert_abs_diff_eq!(f, 1.1750 * ((0.04 - 0.02) * t).exp(), epsilon = 1e-12);
        // Higher USD rates: EURUSD forward points are positive.
        assert!(m.forward_points(vd)? > 0.0);
        assert_abs_diff_eq!(m.forward_points(vd)?, (f - 1.1750) / 1e-4, epsilon = 1e-9);
        let diff = m.implied_rate_differential(vd)?;
        assert!((0.019..0.021).contains(&diff), "{diff}");
        assert!(matches!(m.forward(d(2026, 10, 14)), Err(FxError::ValueBeforeSpot { .. })));
        Ok(())
    }

    #[test]
    fn construction_errors() -> R {
        let today = d(2026, 10, 13);
        let eur = DiscountCurve::flat(today, 0.02, d(2040, 1, 1))?;
        let usd_wrong = DiscountCurve::flat(d(2026, 10, 14), 0.04, d(2040, 1, 1))?;
        let pair: CurrencyPair = "EURUSD".parse()?;
        assert!(matches!(FxMarket::new(pair, today, 1.175, eur.clone(), usd_wrong), Err(FxError::CurveReference { which: "quote", .. })));
        assert!(matches!(FxMarket::new(pair, today, -1.0, eur.clone(), eur.clone()), Err(FxError::NonPositiveSpot(_))));
        let m = eurusd()?;
        assert!(matches!(m.discount(Currency::JPY, today), Err(FxError::CurrencyNotInPair(Currency::JPY, _))));
        assert!(matches!(FxForward::new(pair, FxDirection::BuyBase, 0.0, 1.2, today), Err(FxError::NonPositiveNotional(_))));
        Ok(())
    }

    #[test]
    fn forward_valuation() -> R {
        let m = eurusd()?;
        let pair = m.pair();
        let (vd, f) = m.forward_for_tenor(Period::months(6))?;
        // Struck at the forward: zero value; long gains if struck below.
        let at_fwd = FxForward::new(pair, FxDirection::BuyBase, 10e6, f, vd)?;
        assert_abs_diff_eq!(at_fwd.npv_quote(&m)?, 0.0, epsilon = 1e-6);
        let long = FxForward::new(pair, FxDirection::BuyBase, 10e6, f - 0.01, vd)?;
        let short = FxForward::new(pair, FxDirection::SellBase, 10e6, f - 0.01, vd)?;
        let v = long.value(&m)?;
        let df_q = m.discount(Currency::USD, vd)?;
        assert_abs_diff_eq!(v.npv_quote, 10e6 * 0.01 * df_q, epsilon = 1e-6);
        assert_abs_diff_eq!(v.npv_base, v.npv_quote / 1.175, epsilon = 1e-9);
        assert_abs_diff_eq!(short.value(&m)?.npv_quote, -v.npv_quote, epsilon = 1e-9);
        assert_abs_diff_eq!(v.forward_delta, 10e6 * df_q, epsilon = 1e-9);
        assert_abs_diff_eq!(long.notional_quote(), 10e6 * (f - 0.01), epsilon = 1e-9);
        // Spot delta by finite difference.
        let h = 1e-4;
        let up = long.npv_quote(&m.with_spot(1.175 + h)?)?;
        let dn = long.npv_quote(&m.with_spot(1.175 - h)?)?;
        assert_abs_diff_eq!((up - dn) / (2.0 * h), v.spot_delta, epsilon = 1e-3);
        // from_tenor agrees with explicit dates.
        let ft = FxForward::from_tenor(pair, m.trade_date(), Period::months(6), FxDirection::BuyBase, 10e6, f - 0.01)?;
        assert_eq!(ft.value_date(), vd);
        Ok(())
    }

    #[test]
    fn implied_quote_curve_reprices_points() -> R {
        let today = d(2026, 10, 13);
        let pair: CurrencyPair = "EURUSD".parse()?;
        let eur = DiscountCurve::flat(today, 0.02, d(2040, 1, 1))?;
        let points = [(Period::weeks(1), 4.2), (Period::months(1), 18.5), (Period::months(3), 56.0), (Period::months(6), 112.0), (Period::years(1), 225.0)];
        let usd = FxMarket::implied_quote_curve(pair, today, 1.175, &eur, &points)?;
        let m = FxMarket::new(pair, today, 1.175, eur, usd)?;
        for (tenor, pts) in points {
            let (vd, _) = m.forward_for_tenor(tenor)?;
            assert_abs_diff_eq!(m.forward_points(vd)?, pts, epsilon = 1e-9);
        }
        assert!(matches!(FxMarket::implied_quote_curve(pair, today, 1.175, m.base_curve(), &[]), Err(FxError::NoQuotes)));
        Ok(())
    }

    #[test]
    fn ndf_settlement_conventions() -> R {
        let today = d(2026, 10, 13);
        // USDBRL-style: base USD deliverable, quote non-deliverable; standard NDF settles USD.
        let pair = CurrencyPair::new(Currency::USD, Currency::BRL)?;
        assert_eq!(pair.to_string(), "USDBRL");
        let usd = DiscountCurve::flat(today, 0.04, d(2040, 1, 1))?;
        let brl = DiscountCurve::flat(today, 0.11, d(2040, 1, 1))?;
        let m = FxMarket::new(pair, today, 5.40, usd, brl)?;
        let ndf = FxNdf::standard(pair, today, Period::months(3), FxDirection::BuyBase, 10e6, 5.45)?;
        assert_eq!(ndf.settlement_currency(), Currency::USD);
        assert_eq!(ndf.settlement_date(), pair.forward_date(today, Period::months(3))?);
        assert_eq!(ndf.fixing_date(), FxNdf::fixing_for_settlement(pair, ndf.settlement_date())?);
        assert!(ndf.fixing_date() < ndf.settlement_date());
        assert_eq!(pair.calendar().business_days_between(ndf.fixing_date(), ndf.settlement_date()), 2);
        let f = m.forward(ndf.fixing_date())?;
        assert!(f > 5.40); // BRL rates higher: USDBRL forward above spot
        // Settlement amount in USD = N (F - K) / F; in BRL it would be N (F - K).
        assert_abs_diff_eq!(ndf.settlement_amount(f), 10e6 * (f - 5.45) / f, epsilon = 1e-9);
        let v = ndf.value(&m)?;
        let df_usd = m.discount(Currency::USD, ndf.settlement_date())?;
        assert_abs_diff_eq!(v.npv_base, 10e6 * (f - 5.45) / f * df_usd, epsilon = 1e-6);
        assert_abs_diff_eq!(v.npv_quote, v.npv_base * 5.40, epsilon = 1e-6);
        // Quote-settled version of the same trade differs only by the F conversion and discounting currency.
        let brl_settled = FxNdf::new(pair, FxDirection::BuyBase, 10e6, 5.45, ndf.fixing_date(), ndf.settlement_date(), Currency::BRL)?;
        let vb = brl_settled.value(&m)?;
        let df_brl = m.discount(Currency::BRL, ndf.settlement_date())?;
        assert_abs_diff_eq!(vb.npv_quote, 10e6 * (f - 5.45) * df_brl, epsilon = 1e-6);
        // Forward delta by finite difference on the base-settled NDF.
        let h = 1e-4;
        let up = ndf.value(&m.with_spot(5.40 + h)?)?.npv_quote;
        let dn = ndf.value(&m.with_spot(5.40 - h)?)?.npv_quote;
        assert_abs_diff_eq!((up - dn) / (2.0 * h), v.spot_delta, epsilon = 1e-2);
        assert!(matches!(FxNdf::new(pair, FxDirection::BuyBase, 1e6, 5.4, today, d(2026, 10, 12), Currency::USD), Err(FxError::SettlementBeforeFixing { .. })));
        assert!(matches!(FxNdf::new(pair, FxDirection::BuyBase, 1e6, 5.4, today, today, Currency::EUR), Err(FxError::CurrencyNotInPair(..))));
        Ok(())
    }
}
