//! FX: currency pairs and their date conventions, spot/forward markets on
//! two discount curves, forwards and NDFs, Garman-Kohlhagen vanillas with
//! market delta conventions, and delta-quoted vol surfaces.
//!
//! Pairs are accepted as `CurrencyPair` or a string (`"EURUSD"`,
//! `"EUR/USD"`); currencies as a string (`"USD"`). Directions are `"buy"`
//! / `"sell"` (of the base), option kinds `"call"` / `"put"` (on the base),
//! delta conventions `"spot"`, `"forward"`, `"spot_pa"`, `"forward_pa"`,
//! ATM conventions `"spot"`, `"forward"`, `"dns"`, `"dns_pa"`, premium
//! currency `"base"` / `"quote"`. Valuations come back as dicts.

use pyo3::prelude::*;
use pyo3::types::PyDict;

use fq_fx::{
    AtmConvention, Currency, CurrencyPair, DeltaConvention, FxDirection, FxForward, FxForwardValuation, FxMarket, FxNdf, FxSlice, FxVanilla,
    FxVanillaValuation, FxVolSurface, PremiumCurrency, SmileQuote, SurfaceConfig, delivery_for_expiry, expiry_and_delivery,
};

use crate::commod::{kind_name, parse_kind};
use crate::curve::PyDiscountCurve;
use crate::time::{DateLike, PeriodLike, PyDate, PyPeriod};
use crate::{FerrumError, dict, err};

// ---------------------------------------------------------------------------
// Parsers
// ---------------------------------------------------------------------------

fn parse_currency(s: &str) -> PyResult<Currency> {
    s.parse::<Currency>().map_err(err)
}

fn parse_direction(s: &str) -> PyResult<FxDirection> {
    match s.to_ascii_lowercase().as_str() {
        "buy" | "buy_base" | "long" | "b" => Ok(FxDirection::BuyBase),
        "sell" | "sell_base" | "short" | "s" => Ok(FxDirection::SellBase),
        _ => Err(FerrumError::new_err(format!("unknown direction '{s}' (buy | sell)"))),
    }
}

fn direction_name(d: FxDirection) -> &'static str {
    match d {
        FxDirection::BuyBase => "buy",
        FxDirection::SellBase => "sell",
    }
}

fn parse_delta_convention(s: &str) -> PyResult<DeltaConvention> {
    match s.to_ascii_lowercase().replace(['-', ' '], "_").as_str() {
        "spot" | "spot_delta" => Ok(DeltaConvention::Spot),
        "forward" | "fwd" | "forward_delta" => Ok(DeltaConvention::Forward),
        "spot_pa" | "spot_premium_adjusted" | "pa_spot" => Ok(DeltaConvention::SpotPremiumAdjusted),
        "forward_pa" | "fwd_pa" | "forward_premium_adjusted" | "pa_forward" => Ok(DeltaConvention::ForwardPremiumAdjusted),
        _ => Err(FerrumError::new_err(format!("unknown delta convention '{s}' (spot | forward | spot_pa | forward_pa)"))),
    }
}

fn delta_convention_name(c: DeltaConvention) -> &'static str {
    match c {
        DeltaConvention::Spot => "spot",
        DeltaConvention::Forward => "forward",
        DeltaConvention::SpotPremiumAdjusted => "spot_pa",
        DeltaConvention::ForwardPremiumAdjusted => "forward_pa",
    }
}

fn parse_atm_convention(s: &str) -> PyResult<AtmConvention> {
    match s.to_ascii_lowercase().replace(['-', ' '], "_").as_str() {
        "spot" => Ok(AtmConvention::Spot),
        "forward" | "fwd" => Ok(AtmConvention::Forward),
        "dns" | "delta_neutral" => Ok(AtmConvention::DeltaNeutral),
        "dns_pa" | "delta_neutral_pa" | "pa_dns" => Ok(AtmConvention::DeltaNeutralPremiumAdjusted),
        _ => Err(FerrumError::new_err(format!("unknown ATM convention '{s}' (spot | forward | dns | dns_pa)"))),
    }
}

fn atm_convention_name(c: AtmConvention) -> &'static str {
    match c {
        AtmConvention::Spot => "spot",
        AtmConvention::Forward => "forward",
        AtmConvention::DeltaNeutral => "dns",
        AtmConvention::DeltaNeutralPremiumAdjusted => "dns_pa",
    }
}

fn parse_premium(s: &str) -> PyResult<PremiumCurrency> {
    match s.to_ascii_lowercase().as_str() {
        "base" => Ok(PremiumCurrency::Base),
        "quote" | "terms" => Ok(PremiumCurrency::Quote),
        _ => Err(FerrumError::new_err(format!("unknown premium currency '{s}' (base | quote)"))),
    }
}

fn premium_name(p: PremiumCurrency) -> &'static str {
    match p {
        PremiumCurrency::Base => "base",
        PremiumCurrency::Quote => "quote",
    }
}

// ---------------------------------------------------------------------------
// CurrencyPair
// ---------------------------------------------------------------------------

/// Currency pair in market order, quoted as quote-per-base.
#[pyclass(name = "CurrencyPair", module = "ferrumquant", frozen, eq, hash)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PyCurrencyPair(pub CurrencyPair);

/// A `CurrencyPair` or a string like "EURUSD" / "EUR/USD".
#[derive(Debug, Clone, Copy)]
pub struct PairLike(pub CurrencyPair);

impl<'py> FromPyObject<'py> for PairLike {
    fn extract_bound(obj: &Bound<'py, PyAny>) -> PyResult<Self> {
        if let Ok(p) = obj.downcast::<PyCurrencyPair>() {
            return Ok(Self(p.get().0));
        }
        let s: String = obj.extract()?;
        s.parse::<CurrencyPair>().map(Self).map_err(err)
    }
}

#[pymethods]
impl PyCurrencyPair {
    /// `CurrencyPair("EURUSD")` keeps the order given; `CurrencyPair("USD", "EUR")`
    /// puts the two currencies in interbank market order (EURUSD).
    #[new]
    #[pyo3(signature = (spec, quote = None))]
    fn new(spec: &str, quote: Option<&str>) -> PyResult<Self> {
        match quote {
            Some(q) => CurrencyPair::new(parse_currency(spec)?, parse_currency(q)?).map(Self).map_err(err),
            None => spec.parse::<CurrencyPair>().map(Self).map_err(err),
        }
    }

    #[getter]
    fn base(&self) -> &'static str {
        self.0.base().code()
    }

    #[getter]
    fn quote(&self) -> &'static str {
        self.0.quote().code()
    }

    /// Business days from trade to spot (1 for USDCAD/USDTRY/USDRUB, else 2).
    #[getter]
    fn spot_lag(&self) -> i32 {
        self.0.spot_lag()
    }

    /// Size of one pip in rate units (0.01 for JPY/KRW quotes, else 0.0001).
    #[getter]
    fn pip(&self) -> f64 {
        self.0.pip()
    }

    #[getter]
    fn is_market_order(&self) -> bool {
        self.0.is_market_order()
    }

    /// Market default premium currency: "quote" for xxxUSD, "base" otherwise.
    #[getter]
    fn premium_currency(&self) -> &'static str {
        premium_name(PremiumCurrency::market_default(self.0))
    }

    /// Joint calendar code of the two currencies, e.g. "TARGET+USNY".
    #[getter]
    fn calendar(&self) -> String {
        self.0.calendar().to_string()
    }

    /// Calendar a spot or forward date must be good on (both currencies and USD).
    #[getter]
    fn settlement_calendar(&self) -> String {
        self.0.settlement_calendar().to_string()
    }

    fn inverse(&self) -> Self {
        Self(self.0.inverse())
    }

    fn spot_date(&self, trade_date: DateLike) -> PyResult<PyDate> {
        self.0.spot_date(trade_date.0).map(PyDate).map_err(err)
    }

    /// Value date of an outright for a market tenor (end-of-month rule included).
    fn forward_date(&self, trade_date: DateLike, tenor: PeriodLike) -> PyResult<PyDate> {
        self.0.forward_date(trade_date.0, tenor.0).map(PyDate).map_err(err)
    }

    /// `(expiry, delivery)` of an option for a market tenor.
    fn expiry_and_delivery(&self, trade_date: DateLike, tenor: PeriodLike) -> PyResult<(PyDate, PyDate)> {
        expiry_and_delivery(self.0, trade_date.0, tenor.0).map(|(e, d)| (PyDate(e), PyDate(d))).map_err(err)
    }

    /// Delivery date for an option expiry (spot lag after expiry).
    fn delivery_for_expiry(&self, expiry: DateLike) -> PyResult<PyDate> {
        delivery_for_expiry(self.0, expiry.0).map(PyDate).map_err(err)
    }

    /// Market default delta convention for a tenor under the pair's premium currency.
    fn delta_convention(&self, tenor: PeriodLike) -> &'static str {
        delta_convention_name(DeltaConvention::market_default(PremiumCurrency::market_default(self.0), tenor.0))
    }

    fn __str__(&self) -> String {
        self.0.to_string()
    }

    fn __repr__(&self) -> String {
        format!("CurrencyPair({})", self.0)
    }
}

// ---------------------------------------------------------------------------
// FxMarket
// ---------------------------------------------------------------------------

/// Spot plus base and quote discount curves (both referenced at the trade date).
#[pyclass(name = "FxMarket", module = "ferrumquant", frozen)]
#[derive(Debug, Clone)]
pub struct PyFxMarket(pub FxMarket);

#[pymethods]
impl PyFxMarket {
    #[new]
    fn new(pair: PairLike, trade_date: DateLike, spot: f64, base_curve: &PyDiscountCurve, quote_curve: &PyDiscountCurve) -> PyResult<Self> {
        FxMarket::new(pair.0, trade_date.0, spot, base_curve.0.clone(), quote_curve.0.clone()).map(Self).map_err(err)
    }

    /// Build the quote-currency curve that reprices forward points
    /// `[(tenor, pips), ...]` off the base curve (FX-swap-implied, basis included).
    #[staticmethod]
    fn implied_quote_curve(
        pair: PairLike,
        trade_date: DateLike,
        spot: f64,
        base_curve: &PyDiscountCurve,
        points: Vec<(PeriodLike, f64)>,
    ) -> PyResult<PyDiscountCurve> {
        let pts: Vec<_> = points.into_iter().map(|(t, p)| (t.0, p)).collect();
        FxMarket::implied_quote_curve(pair.0, trade_date.0, spot, &base_curve.0, &pts).map(PyDiscountCurve).map_err(err)
    }

    #[getter]
    fn pair(&self) -> PyCurrencyPair {
        PyCurrencyPair(self.0.pair())
    }

    #[getter]
    fn trade_date(&self) -> PyDate {
        PyDate(self.0.trade_date())
    }

    #[getter]
    fn spot_date(&self) -> PyDate {
        PyDate(self.0.spot_date())
    }

    #[getter]
    fn spot(&self) -> f64 {
        self.0.spot()
    }

    #[getter]
    fn base_curve(&self) -> PyDiscountCurve {
        PyDiscountCurve(self.0.base_curve().clone())
    }

    #[getter]
    fn quote_curve(&self) -> PyDiscountCurve {
        PyDiscountCurve(self.0.quote_curve().clone())
    }

    /// Discount factor in `currency` from the trade date.
    fn discount(&self, currency: &str, date: DateLike) -> PyResult<f64> {
        self.0.discount(parse_currency(currency)?, date.0).map_err(err)
    }

    /// Outright forward for a value date on or after spot.
    fn forward(&self, value_date: DateLike) -> PyResult<f64> {
        self.0.forward(value_date.0).map_err(err)
    }

    /// Forward points in pips.
    fn forward_points(&self, value_date: DateLike) -> PyResult<f64> {
        self.0.forward_points(value_date.0).map_err(err)
    }

    /// `(value_date, outright)` for a market tenor.
    fn forward_for_tenor(&self, tenor: PeriodLike) -> PyResult<(PyDate, f64)> {
        self.0.forward_for_tenor(tenor.0).map(|(d, f)| (PyDate(d), f)).map_err(err)
    }

    /// Simple ACT/360 quote-minus-base rate differential implied by the forward.
    fn implied_rate_differential(&self, value_date: DateLike) -> PyResult<f64> {
        self.0.implied_rate_differential(value_date.0).map_err(err)
    }

    fn with_spot(&self, spot: f64) -> PyResult<Self> {
        self.0.with_spot(spot).map(Self).map_err(err)
    }

    fn with_curves(&self, base_curve: &PyDiscountCurve, quote_curve: &PyDiscountCurve) -> PyResult<Self> {
        self.0.with_curves(base_curve.0.clone(), quote_curve.0.clone()).map(Self).map_err(err)
    }

    fn __repr__(&self) -> String {
        format!("FxMarket({} spot={} trade={} spot_date={})", self.0.pair(), self.0.spot(), self.0.trade_date(), self.0.spot_date())
    }
}

// ---------------------------------------------------------------------------
// Forwards and NDFs
// ---------------------------------------------------------------------------

fn forward_dict<'py>(py: Python<'py>, v: &FxForwardValuation) -> PyResult<Bound<'py, PyDict>> {
    Ok(dict!(py,
        "forward" => v.forward, "forward_points" => v.forward_points, "npv_quote" => v.npv_quote, "npv_base" => v.npv_base,
        "spot_delta" => v.spot_delta, "forward_delta" => v.forward_delta, "discount_quote" => v.discount_quote,
    ))
}

/// Deliverable outright forward on a base notional.
#[pyclass(name = "FxForward", module = "ferrumquant", frozen)]
#[derive(Debug, Clone, Copy)]
pub struct PyFxForward(pub FxForward);

#[pymethods]
impl PyFxForward {
    /// `FxForward("EURUSD", "buy", 10e6, 1.1850, "2027-04-15")`.
    #[new]
    fn new(pair: PairLike, direction: &str, notional_base: f64, strike: f64, value_date: DateLike) -> PyResult<Self> {
        FxForward::new(pair.0, parse_direction(direction)?, notional_base, strike, value_date.0).map(Self).map_err(err)
    }

    /// Forward for a market tenor from the trade date.
    #[staticmethod]
    fn from_tenor(pair: PairLike, trade_date: DateLike, tenor: PeriodLike, direction: &str, notional_base: f64, strike: f64) -> PyResult<Self> {
        FxForward::from_tenor(pair.0, trade_date.0, tenor.0, parse_direction(direction)?, notional_base, strike).map(Self).map_err(err)
    }

    #[getter]
    fn pair(&self) -> PyCurrencyPair {
        PyCurrencyPair(self.0.pair())
    }

    #[getter]
    fn direction(&self) -> &'static str {
        direction_name(self.0.direction())
    }

    #[getter]
    fn notional_base(&self) -> f64 {
        self.0.notional_base()
    }

    #[getter]
    fn notional_quote(&self) -> f64 {
        self.0.notional_quote()
    }

    #[getter]
    fn strike(&self) -> f64 {
        self.0.strike()
    }

    #[getter]
    fn value_date(&self) -> PyDate {
        PyDate(self.0.value_date())
    }

    /// Dict: forward, forward_points, npv_quote, npv_base, spot_delta, forward_delta, discount_quote.
    fn value<'py>(&self, py: Python<'py>, market: &PyFxMarket) -> PyResult<Bound<'py, PyDict>> {
        forward_dict(py, &self.0.value(&market.0).map_err(err)?)
    }

    fn npv_quote(&self, market: &PyFxMarket) -> PyResult<f64> {
        self.0.npv_quote(&market.0).map_err(err)
    }

    fn __repr__(&self) -> String {
        format!("FxForward({} {} {} @ {} value {})", direction_name(self.0.direction()), self.0.notional_base(), self.0.pair(), self.0.strike(), self.0.value_date())
    }
}

/// Non-deliverable forward: fixes on `fixing_date`, cash-settles on `settlement_date`.
#[pyclass(name = "FxNdf", module = "ferrumquant", frozen)]
#[derive(Debug, Clone, Copy)]
pub struct PyFxNdf(pub FxNdf);

#[pymethods]
impl PyFxNdf {
    #[new]
    #[allow(clippy::too_many_arguments)]
    fn new(
        pair: PairLike,
        direction: &str,
        notional_base: f64,
        strike: f64,
        fixing_date: DateLike,
        settlement_date: DateLike,
        settlement_currency: &str,
    ) -> PyResult<Self> {
        FxNdf::new(pair.0, parse_direction(direction)?, notional_base, strike, fixing_date.0, settlement_date.0, parse_currency(settlement_currency)?)
            .map(Self)
            .map_err(err)
    }

    /// Standard NDF for a tenor: settles on the forward date, fixes `spot_lag`
    /// business days earlier, settled in USD when the pair has it.
    #[staticmethod]
    fn standard(pair: PairLike, trade_date: DateLike, tenor: PeriodLike, direction: &str, notional_base: f64, strike: f64) -> PyResult<Self> {
        FxNdf::standard(pair.0, trade_date.0, tenor.0, parse_direction(direction)?, notional_base, strike).map(Self).map_err(err)
    }

    #[getter]
    fn pair(&self) -> PyCurrencyPair {
        PyCurrencyPair(self.0.pair())
    }

    #[getter]
    fn direction(&self) -> &'static str {
        direction_name(self.0.direction())
    }

    #[getter]
    fn notional_base(&self) -> f64 {
        self.0.notional_base()
    }

    #[getter]
    fn strike(&self) -> f64 {
        self.0.strike()
    }

    #[getter]
    fn fixing_date(&self) -> PyDate {
        PyDate(self.0.fixing_date())
    }

    #[getter]
    fn settlement_date(&self) -> PyDate {
        PyDate(self.0.settlement_date())
    }

    #[getter]
    fn settlement_currency(&self) -> &'static str {
        self.0.settlement_currency().code()
    }

    /// Settlement amount in the settlement currency for a given fixing.
    fn settlement_amount(&self, fixing: f64) -> f64 {
        self.0.settlement_amount(fixing)
    }

    fn value<'py>(&self, py: Python<'py>, market: &PyFxMarket) -> PyResult<Bound<'py, PyDict>> {
        forward_dict(py, &self.0.value(&market.0).map_err(err)?)
    }

    fn __repr__(&self) -> String {
        format!(
            "FxNdf({} {} {} @ {} fix {} settle {} in {})",
            direction_name(self.0.direction()),
            self.0.notional_base(),
            self.0.pair(),
            self.0.strike(),
            self.0.fixing_date(),
            self.0.settlement_date(),
            self.0.settlement_currency()
        )
    }
}

// ---------------------------------------------------------------------------
// Vanillas
// ---------------------------------------------------------------------------

/// Forward, quote discount factor and time to expiry for one expiry, with
/// the delta/strike conversions a smile needs.
#[pyclass(name = "FxSlice", module = "ferrumquant", frozen, get_all)]
#[derive(Debug, Clone, Copy)]
pub struct PyFxSlice {
    spot: f64,
    forward: f64,
    discount_quote: f64,
    time_to_expiry: f64,
}

impl PyFxSlice {
    fn inner(&self) -> FxSlice {
        FxSlice { spot: self.spot, forward: self.forward, discount_quote: self.discount_quote, time_to_expiry: self.time_to_expiry }
    }
}

impl From<FxSlice> for PyFxSlice {
    fn from(s: FxSlice) -> Self {
        Self { spot: s.spot, forward: s.forward, discount_quote: s.discount_quote, time_to_expiry: s.time_to_expiry }
    }
}

#[pymethods]
impl PyFxSlice {
    #[new]
    fn new(spot: f64, forward: f64, discount_quote: f64, time_to_expiry: f64) -> Self {
        Self { spot, forward, discount_quote, time_to_expiry }
    }

    /// `DF_q * F / S`: turns a forward delta into a spot delta.
    fn spot_factor(&self) -> f64 {
        self.inner().spot_factor()
    }

    /// Delta of a `kind` option at `strike` with flat `vol` under `convention`.
    fn delta(&self, kind: &str, strike: f64, vol: f64, convention: &str) -> PyResult<f64> {
        self.inner().delta(parse_kind(kind)?, strike, vol, parse_delta_convention(convention)?).map_err(err)
    }

    /// Strike with the given signed delta (puts negative) under `convention`.
    fn strike_from_delta(&self, kind: &str, delta: f64, vol: f64, convention: &str) -> PyResult<f64> {
        self.inner().strike_from_delta(parse_kind(kind)?, delta, vol, parse_delta_convention(convention)?).map_err(err)
    }

    /// ATM strike under `convention` ("spot" | "forward" | "dns" | "dns_pa").
    fn atm_strike(&self, convention: &str, vol: f64) -> PyResult<f64> {
        Ok(self.inner().atm_strike(parse_atm_convention(convention)?, vol))
    }

    fn __repr__(&self) -> String {
        format!("FxSlice(spot={}, forward={}, discount_quote={}, time_to_expiry={})", self.spot, self.forward, self.discount_quote, self.time_to_expiry)
    }
}

fn vanilla_dict<'py>(py: Python<'py>, v: &FxVanillaValuation) -> PyResult<Bound<'py, PyDict>> {
    Ok(dict!(py,
        "forward" => v.forward, "discount_quote" => v.discount_quote, "time_to_expiry" => v.time_to_expiry, "vol" => v.vol,
        "pv_quote" => v.pv_quote, "pv_base" => v.pv_base, "pct_base" => v.pct_base, "quote_pips" => v.quote_pips,
        "spot_delta" => v.spot_delta, "forward_delta" => v.forward_delta, "spot_delta_pa" => v.spot_delta_pa, "forward_delta_pa" => v.forward_delta_pa,
        "gamma" => v.gamma, "vega" => v.vega, "vanna" => v.vanna, "volga" => v.volga,
    ))
}

/// European vanilla on one unit of base currency (Garman-Kohlhagen).
#[pyclass(name = "FxVanilla", module = "ferrumquant", frozen)]
#[derive(Debug, Clone, Copy)]
pub struct PyFxVanilla(pub FxVanilla);

#[pymethods]
impl PyFxVanilla {
    /// `FxVanilla("EURUSD", "call", 1.20, expiry, delivery, 1e6)`.
    #[new]
    #[pyo3(signature = (pair, kind, strike, expiry, delivery, notional_base, premium = None))]
    fn new(pair: PairLike, kind: &str, strike: f64, expiry: DateLike, delivery: DateLike, notional_base: f64, premium: Option<&str>) -> PyResult<Self> {
        let mut v = FxVanilla::new(pair.0, parse_kind(kind)?, strike, expiry.0, delivery.0, notional_base).map_err(err)?;
        if let Some(p) = premium {
            v = v.with_premium_currency(parse_premium(p)?);
        }
        Ok(Self(v))
    }

    /// Market-tenor option: delivery on the forward date, expiry `spot_lag` business days before.
    #[staticmethod]
    #[pyo3(signature = (pair, trade_date, tenor, kind, strike, notional_base, premium = None))]
    fn from_tenor(pair: PairLike, trade_date: DateLike, tenor: PeriodLike, kind: &str, strike: f64, notional_base: f64, premium: Option<&str>) -> PyResult<Self> {
        let mut v = FxVanilla::from_tenor(pair.0, trade_date.0, tenor.0, parse_kind(kind)?, strike, notional_base).map_err(err)?;
        if let Some(p) = premium {
            v = v.with_premium_currency(parse_premium(p)?);
        }
        Ok(Self(v))
    }

    #[getter]
    fn pair(&self) -> PyCurrencyPair {
        PyCurrencyPair(self.0.pair())
    }

    #[getter]
    fn kind(&self) -> &'static str {
        kind_name(self.0.kind())
    }

    #[getter]
    fn strike(&self) -> f64 {
        self.0.strike()
    }

    #[getter]
    fn expiry(&self) -> PyDate {
        PyDate(self.0.expiry())
    }

    #[getter]
    fn delivery(&self) -> PyDate {
        PyDate(self.0.delivery())
    }

    #[getter]
    fn notional_base(&self) -> f64 {
        self.0.notional_base()
    }

    #[getter]
    fn premium_currency(&self) -> &'static str {
        premium_name(self.0.premium_currency())
    }

    /// Forward, quote DF and time to expiry on a market.
    fn slice(&self, market: &PyFxMarket) -> PyResult<PyFxSlice> {
        self.0.slice(&market.0).map(Into::into).map_err(err)
    }

    /// Dict: pv_quote, pv_base, pct_base, quote_pips, spot/forward deltas
    /// (plain and premium-adjusted), gamma, vega, vanna, volga, forward,
    /// discount_quote, time_to_expiry, vol.
    fn value<'py>(&self, py: Python<'py>, market: &PyFxMarket, vol: f64) -> PyResult<Bound<'py, PyDict>> {
        vanilla_dict(py, &self.0.value(&market.0, vol).map_err(err)?)
    }

    /// Delta per unit base notional under a convention.
    fn delta(&self, market: &PyFxMarket, vol: f64, convention: &str) -> PyResult<f64> {
        self.0.delta(&market.0, vol, parse_delta_convention(convention)?).map_err(err)
    }

    /// Flat vol reproducing a quote-currency premium.
    fn implied_vol(&self, market: &PyFxMarket, pv_quote: f64) -> PyResult<f64> {
        self.0.implied_vol(&market.0, pv_quote).map_err(err)
    }

    fn __repr__(&self) -> String {
        format!(
            "FxVanilla({} {} {} @ {} expiry {} delivery {}, premium {})",
            self.0.notional_base(),
            self.0.pair(),
            kind_name(self.0.kind()),
            self.0.strike(),
            self.0.expiry(),
            self.0.delivery(),
            premium_name(self.0.premium_currency())
        )
    }
}

// ---------------------------------------------------------------------------
// Vol surface
// ---------------------------------------------------------------------------

/// A `SmileQuote` from a dict `{"tenor", "atm", "rr25", "bf25"[, "rr10", "bf10"]}`
/// or a tuple `(tenor, atm, rr25, bf25[, rr10, bf10])`.
#[derive(Debug, Clone, Copy)]
pub struct SmileQuoteLike(SmileQuote);

impl<'py> FromPyObject<'py> for SmileQuoteLike {
    fn extract_bound(obj: &Bound<'py, PyAny>) -> PyResult<Self> {
        if let Ok(d) = obj.downcast::<PyDict>() {
            let get = |k: &str| -> PyResult<Option<f64>> { d.get_item(k)?.map(|v| v.extract::<f64>()).transpose() };
            let need = |k: &str| -> PyResult<f64> { get(k)?.ok_or_else(|| FerrumError::new_err(format!("smile quote is missing '{k}'"))) };
            let tenor: PeriodLike = d.get_item("tenor")?.ok_or_else(|| FerrumError::new_err("smile quote is missing 'tenor'"))?.extract()?;
            let mut q = SmileQuote::new(tenor.0, need("atm")?, need("rr25")?, need("bf25")?);
            q.rr10 = get("rr10")?;
            q.bf10 = get("bf10")?;
            return Ok(Self(q));
        }
        if let Ok((t, atm, rr25, bf25, rr10, bf10)) = obj.extract::<(PeriodLike, f64, f64, f64, f64, f64)>() {
            return Ok(Self(SmileQuote::new(t.0, atm, rr25, bf25).with_10d(rr10, bf10)));
        }
        let (t, atm, rr25, bf25): (PeriodLike, f64, f64, f64) = obj
            .extract()
            .map_err(|_| FerrumError::new_err("smile quote must be (tenor, atm, rr25, bf25[, rr10, bf10]) or a dict with those keys"))?;
        Ok(Self(SmileQuote::new(t.0, atm, rr25, bf25)))
    }
}

/// Smiles on market tenors from ATM/RR/BF quotes, SABR per pillar,
/// total-variance interpolation in time.
#[pyclass(name = "FxVolSurface", module = "ferrumquant", frozen)]
#[derive(Debug, Clone)]
pub struct PyFxVolSurface(pub FxVolSurface);

#[pymethods]
impl PyFxVolSurface {
    /// `FxVolSurface(market, quotes, premium=None, atm=None, delta=None, beta=1.0)`;
    /// conventions default to the pair's market rules (`CurrencyPair.premium_currency`,
    /// delta-neutral ATM, spot delta to 1Y then forward delta).
    #[new]
    #[pyo3(signature = (market, quotes, premium = None, atm = None, delta = None, beta = 1.0))]
    fn new(market: &PyFxMarket, quotes: Vec<SmileQuoteLike>, premium: Option<&str>, atm: Option<&str>, delta: Option<&str>, beta: f64) -> PyResult<Self> {
        let mut config = SurfaceConfig::market_default(market.0.pair());
        if let Some(p) = premium {
            config.premium = parse_premium(p)?;
            config.atm = AtmConvention::market_default(config.premium);
        }
        if let Some(a) = atm {
            config.atm = parse_atm_convention(a)?;
        }
        if let Some(d) = delta {
            config.delta = Some(parse_delta_convention(d)?);
        }
        config.beta = beta;
        let q: Vec<SmileQuote> = quotes.into_iter().map(|s| s.0).collect();
        FxVolSurface::with_config(&market.0, &q, config).map(Self).map_err(err)
    }

    #[getter]
    fn market(&self) -> PyFxMarket {
        PyFxMarket(self.0.market().clone())
    }

    /// Conventions in force: premium, atm, delta (None = market rule by tenor), beta.
    fn config<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let c = self.0.config();
        Ok(dict!(py,
            "premium" => premium_name(c.premium), "atm" => atm_convention_name(c.atm),
            "delta" => c.delta.map(delta_convention_name), "beta" => c.beta,
        ))
    }

    #[getter]
    fn tenors(&self) -> Vec<PyPeriod> {
        self.0.smiles().iter().map(|s| PyPeriod(s.tenor)).collect()
    }

    /// One dict per pillar: tenor, expiry, delivery, forward, discount_quote,
    /// time_to_expiry, delta_convention, atm_strike, atm_vol, points
    /// `[(delta, strike, vol), ...]` (puts negative, ATM 0), sabr (alpha, beta, rho, nu), rmse.
    fn smiles<'py>(&self, py: Python<'py>) -> PyResult<Vec<Bound<'py, PyDict>>> {
        self.0.smiles().iter().map(|s| smile_dict(py, s)).collect()
    }

    fn smile<'py>(&self, py: Python<'py>, tenor: PeriodLike) -> PyResult<Bound<'py, PyDict>> {
        let s = self.0.smile(tenor.0).ok_or_else(|| FerrumError::new_err(format!("no smile pillar for tenor {}", tenor.0)))?;
        smile_dict(py, s)
    }

    /// Lognormal vol at an expiry date and strike.
    fn vol(&self, expiry: DateLike, strike: f64) -> PyResult<f64> {
        self.0.vol(expiry.0, strike).map_err(err)
    }

    /// `(strike, vol)` at an expiry for a signed delta under the convention
    /// the surface applies to that expiry.
    fn vol_for_delta(&self, expiry: DateLike, kind: &str, delta: f64) -> PyResult<(f64, f64)> {
        self.0.vol_for_delta(expiry.0, parse_kind(kind)?, delta).map_err(err)
    }

    /// Forward, quote DF and time for an arbitrary expiry.
    fn slice_at(&self, expiry: DateLike) -> PyResult<PyFxSlice> {
        self.0.slice_at(expiry.0).map(Into::into).map_err(err)
    }

    /// Price a vanilla with the surface vol at its expiry and strike.
    fn value<'py>(&self, py: Python<'py>, option: &PyFxVanilla) -> PyResult<Bound<'py, PyDict>> {
        vanilla_dict(py, &self.0.value(&option.0).map_err(err)?)
    }

    /// Surface with every pillar's ATM vol shifted by `shift` (RR/BF kept).
    fn with_parallel_bump(&self, shift: f64) -> PyResult<Self> {
        self.0.with_parallel_bump(shift).map(Self).map_err(err)
    }

    fn __len__(&self) -> usize {
        self.0.smiles().len()
    }

    fn __repr__(&self) -> String {
        let tenors: Vec<String> = self.0.smiles().iter().map(|s| s.tenor.to_string()).collect();
        format!("FxVolSurface({} pillars [{}])", self.0.market().pair(), tenors.join(", "))
    }
}

fn smile_dict<'py>(py: Python<'py>, s: &fq_fx::FxSmile) -> PyResult<Bound<'py, PyDict>> {
    let sabr = dict!(py, "alpha" => s.sabr.alpha(), "beta" => s.sabr.beta(), "rho" => s.sabr.rho(), "nu" => s.sabr.nu());
    Ok(dict!(py,
        "tenor" => PyPeriod(s.tenor), "expiry" => PyDate(s.expiry), "delivery" => PyDate(s.delivery),
        "forward" => s.slice.forward, "discount_quote" => s.slice.discount_quote, "time_to_expiry" => s.slice.time_to_expiry,
        "delta_convention" => delta_convention_name(s.delta_convention), "atm_strike" => s.atm_strike, "atm_vol" => s.atm_vol,
        "points" => s.points.clone(), "sabr" => sabr, "rmse" => s.rmse,
    ))
}

pub(crate) fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyCurrencyPair>()?;
    m.add_class::<PyFxMarket>()?;
    m.add_class::<PyFxForward>()?;
    m.add_class::<PyFxNdf>()?;
    m.add_class::<PyFxSlice>()?;
    m.add_class::<PyFxVanilla>()?;
    m.add_class::<PyFxVolSurface>()?;
    Ok(())
}
