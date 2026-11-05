//! OIS swaps and European swaptions.

use pyo3::prelude::*;
use pyo3::types::PyDict;

use fq_rates::swap::{OisSwap, SwapCashflow, SwapPeriod, SwapSide, SwapValuation};
use fq_rates::swaption::{Swaption, SwaptionPricing, SwaptionType, VolQuote};

use crate::curve::{PyDiscountCurve, PyOvernightIndex};
use crate::time::{DateLike, PeriodLike, PyDate};
use crate::{FerrumError, err};

fn parse_side(s: &str) -> PyResult<SwapSide> {
    match s.to_ascii_lowercase().as_str() {
        "pay" | "payer" | "pay_fixed" | "payfixed" => Ok(SwapSide::PayFixed),
        "receive" | "receiver" | "rec" | "receive_fixed" | "receivefixed" => Ok(SwapSide::ReceiveFixed),
        _ => Err(FerrumError::new_err(format!("unknown swap side '{s}' (pay | receive)"))),
    }
}

fn side_name(s: SwapSide) -> &'static str {
    match s {
        SwapSide::PayFixed => "pay",
        SwapSide::ReceiveFixed => "receive",
    }
}

fn parse_kind(s: &str) -> PyResult<SwaptionType> {
    match s.to_ascii_lowercase().as_str() {
        "payer" | "pay" | "call" => Ok(SwaptionType::Payer),
        "receiver" | "receive" | "rec" | "put" => Ok(SwaptionType::Receiver),
        _ => Err(FerrumError::new_err(format!("unknown swaption type '{s}' (payer | receiver)"))),
    }
}

fn kind_name(k: SwaptionType) -> &'static str {
    match k {
        SwaptionType::Payer => "payer",
        SwaptionType::Receiver => "receiver",
    }
}

fn period_dict<'py>(py: Python<'py>, p: &SwapPeriod) -> PyResult<Bound<'py, PyDict>> {
    let d = PyDict::new(py);
    d.set_item("accrual_start", PyDate(p.accrual_start))?;
    d.set_item("accrual_end", PyDate(p.accrual_end))?;
    d.set_item("payment_date", PyDate(p.payment_date))?;
    d.set_item("fixed_year_fraction", p.fixed_year_fraction)?;
    d.set_item("fixed_amount", p.fixed_amount)?;
    Ok(d)
}

fn cashflow_dict<'py>(py: Python<'py>, c: &SwapCashflow) -> PyResult<Bound<'py, PyDict>> {
    let d = PyDict::new(py);
    d.set_item("accrual_start", PyDate(c.accrual_start))?;
    d.set_item("accrual_end", PyDate(c.accrual_end))?;
    d.set_item("payment_date", PyDate(c.payment_date))?;
    d.set_item("rate", c.rate)?;
    d.set_item("amount", c.amount)?;
    Ok(d)
}

// ---------------------------------------------------------------------------
// OisSwap
// ---------------------------------------------------------------------------

/// Swap valuation from the holder's side (receive-float minus pay-fixed for a payer).
#[pyclass(name = "SwapValuation", module = "ferrumquant", frozen, get_all)]
#[derive(Debug, Clone, Copy)]
pub struct PySwapValuation {
    npv: f64,
    fixed_leg_pv: f64,
    float_leg_pv: f64,
    /// PV of 1 unit of fixed rate on the notional (PV01 base).
    annuity: f64,
    par_rate: f64,
}

impl From<SwapValuation> for PySwapValuation {
    fn from(v: SwapValuation) -> Self {
        Self { npv: v.npv, fixed_leg_pv: v.fixed_leg_pv, float_leg_pv: v.float_leg_pv, annuity: v.annuity, par_rate: v.par_rate }
    }
}

#[pymethods]
impl PySwapValuation {
    fn to_dict<'py>(self_: PyRef<'py, Self>, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let d = PyDict::new(py);
        d.set_item("npv", self_.npv)?;
        d.set_item("fixed_leg_pv", self_.fixed_leg_pv)?;
        d.set_item("float_leg_pv", self_.float_leg_pv)?;
        d.set_item("annuity", self_.annuity)?;
        d.set_item("par_rate", self_.par_rate)?;
        Ok(d)
    }

    fn __repr__(&self) -> String {
        format!("SwapValuation(npv={:.2}, par_rate={:.6}%, annuity={:.2})", self.npv, self.par_rate * 100.0, self.annuity)
    }
}

/// USD SOFR OIS: annual fixed ACT/360 vs SOFR compounded in arrears, T+2 payment lag.
#[pyclass(name = "OisSwap", module = "ferrumquant", frozen)]
#[derive(Debug, Clone)]
pub struct PyOisSwap(pub OisSwap);

#[pymethods]
impl PyOisSwap {
    /// Swap between explicit effective and maturity dates.
    #[staticmethod]
    fn usd_sofr(effective: DateLike, maturity: DateLike, side: &str, notional: f64, fixed_rate: f64) -> PyResult<Self> {
        OisSwap::usd_sofr(effective.0, maturity.0, parse_side(side)?, notional, fixed_rate).map(Self).map_err(err)
    }

    /// Spot-starting (T+2) swap of a given tenor from the trade date.
    #[staticmethod]
    fn usd_sofr_spot(trade_date: DateLike, tenor: PeriodLike, side: &str, notional: f64, fixed_rate: f64) -> PyResult<Self> {
        OisSwap::usd_sofr_spot(trade_date.0, tenor.0, parse_side(side)?, notional, fixed_rate).map(Self).map_err(err)
    }

    #[getter]
    fn side(&self) -> &'static str {
        side_name(self.0.side())
    }

    #[getter]
    fn notional(&self) -> f64 {
        self.0.notional()
    }

    #[getter]
    fn fixed_rate(&self) -> f64 {
        self.0.fixed_rate()
    }

    #[getter]
    fn effective_date(&self) -> PyDate {
        PyDate(self.0.effective_date())
    }

    #[getter]
    fn maturity_date(&self) -> PyDate {
        PyDate(self.0.maturity_date())
    }

    #[getter]
    fn final_payment_date(&self) -> PyDate {
        PyDate(self.0.final_payment_date())
    }

    #[getter]
    fn payment_lag(&self) -> i32 {
        self.0.payment_lag()
    }

    /// Accrual periods: accrual_start, accrual_end, payment_date, fixed_year_fraction, fixed_amount.
    fn periods<'py>(&self, py: Python<'py>) -> PyResult<Vec<Bound<'py, PyDict>>> {
        self.0.periods().iter().map(|p| period_dict(py, p)).collect()
    }

    fn fixed_cashflows<'py>(&self, py: Python<'py>) -> PyResult<Vec<Bound<'py, PyDict>>> {
        self.0.fixed_cashflows().iter().map(|c| cashflow_dict(py, c)).collect()
    }

    /// Projected floating cashflows (compounded SOFR per period) off `projection`.
    fn float_cashflows<'py>(&self, py: Python<'py>, index: &PyOvernightIndex, projection: &PyDiscountCurve) -> PyResult<Vec<Bound<'py, PyDict>>> {
        self.0.float_cashflows(&index.0, &projection.0).map_err(err)?.iter().map(|c| cashflow_dict(py, c)).collect()
    }

    /// Full valuation. `discount` defaults to `projection` (single curve).
    #[pyo3(signature = (index, projection, discount = None))]
    fn value(&self, index: &PyOvernightIndex, projection: &PyDiscountCurve, discount: Option<&PyDiscountCurve>) -> PyResult<PySwapValuation> {
        let disc = discount.map_or(&projection.0, |d| &d.0);
        self.0.value(&index.0, &projection.0, disc).map(Into::into).map_err(err)
    }

    /// Single-curve NPV.
    fn npv(&self, index: &PyOvernightIndex, curve: &PyDiscountCurve) -> PyResult<f64> {
        self.0.npv(&index.0, &curve.0).map_err(err)
    }

    /// Single-curve par rate.
    fn par_rate(&self, index: &PyOvernightIndex, curve: &PyDiscountCurve) -> PyResult<f64> {
        Ok(self.0.value(&index.0, &curve.0, &curve.0).map_err(err)?.par_rate)
    }

    /// NPV change for a +1bp parallel zero shift (central difference).
    fn dv01(&self, index: &PyOvernightIndex, curve: &PyDiscountCurve) -> PyResult<f64> {
        self.0.dv01(&index.0, &curve.0).map_err(err)
    }

    fn __repr__(&self) -> String {
        format!(
            "OisSwap({} fixed {:.4}%, notional={}, {} -> {})",
            side_name(self.0.side()),
            self.0.fixed_rate() * 100.0,
            self.0.notional(),
            self.0.effective_date(),
            self.0.maturity_date()
        )
    }
}

// ---------------------------------------------------------------------------
// Swaption
// ---------------------------------------------------------------------------

/// Swaption price and Greeks in currency units.
#[pyclass(name = "SwaptionPricing", module = "ferrumquant", frozen, get_all)]
#[derive(Debug, Clone, Copy)]
pub struct PySwaptionPricing {
    pv: f64,
    forward_rate: f64,
    strike: f64,
    annuity: f64,
    expiry_time: f64,
    /// dPV/dForward per unit rate; divide by 1e4 for 1bp.
    delta: f64,
    gamma: f64,
    /// dPV/dVol in the quote's units; times 1e-4 gives vega per bp of normal vol.
    vega: f64,
    /// Model price per unit annuity.
    model_price: f64,
}

impl From<SwaptionPricing> for PySwaptionPricing {
    fn from(p: SwaptionPricing) -> Self {
        Self {
            pv: p.pv,
            forward_rate: p.forward_rate,
            strike: p.strike,
            annuity: p.annuity,
            expiry_time: p.expiry_time,
            delta: p.delta,
            gamma: p.gamma,
            vega: p.vega,
            model_price: p.model.price,
        }
    }
}

#[pymethods]
impl PySwaptionPricing {
    fn to_dict<'py>(self_: PyRef<'py, Self>, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let d = PyDict::new(py);
        d.set_item("pv", self_.pv)?;
        d.set_item("forward_rate", self_.forward_rate)?;
        d.set_item("strike", self_.strike)?;
        d.set_item("annuity", self_.annuity)?;
        d.set_item("expiry_time", self_.expiry_time)?;
        d.set_item("delta", self_.delta)?;
        d.set_item("gamma", self_.gamma)?;
        d.set_item("vega", self_.vega)?;
        d.set_item("model_price", self_.model_price)?;
        Ok(d)
    }

    fn __repr__(&self) -> String {
        format!(
            "SwaptionPricing(pv={:.2}, forward={:.4}%, strike={:.4}%, annuity={:.2}, vega={:.2})",
            self.pv,
            self.forward_rate * 100.0,
            self.strike * 100.0,
            self.annuity,
            self.vega
        )
    }
}

/// European swaption on a USD SOFR OIS, priced under the annuity measure.
#[pyclass(name = "Swaption", module = "ferrumquant", frozen)]
#[derive(Debug, Clone)]
pub struct PySwaption(pub Swaption);

fn vol_quote(normal_vol: Option<f64>, lognormal_vol: Option<f64>, shift: f64) -> PyResult<VolQuote> {
    match (normal_vol, lognormal_vol) {
        (Some(v), None) => Ok(VolQuote::Normal(v)),
        (None, Some(v)) => Ok(VolQuote::ShiftedLognormal { vol: v, shift }),
        _ => Err(FerrumError::new_err("give exactly one of normal_vol or lognormal_vol")),
    }
}

#[pymethods]
impl PySwaption {
    /// Wrap a forward-starting swap; its side must match `kind`, its fixed rate is the strike.
    #[new]
    fn new(expiry: DateLike, kind: &str, underlying: &PyOisSwap) -> PyResult<Self> {
        Swaption::new(expiry.0, parse_kind(kind)?, underlying.0.clone()).map(Self).map_err(err)
    }

    /// e.g. 1Y-into-10Y payer: `Swaption.usd_sofr("2027-09-23", "10Y", "payer", 1e6, 0.04)`.
    /// The underlying starts two business days after expiry.
    #[staticmethod]
    fn usd_sofr(expiry: DateLike, tenor: PeriodLike, kind: &str, notional: f64, strike: f64) -> PyResult<Self> {
        Swaption::usd_sofr(expiry.0, tenor.0, parse_kind(kind)?, notional, strike).map(Self).map_err(err)
    }

    #[getter]
    fn expiry(&self) -> PyDate {
        PyDate(self.0.expiry())
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
    fn underlying(&self) -> PyOisSwap {
        PyOisSwap(self.0.underlying().clone())
    }

    /// (forward swap rate, annuity in currency units).
    #[pyo3(signature = (index, projection, discount = None))]
    fn forward_and_annuity(&self, index: &PyOvernightIndex, projection: &PyDiscountCurve, discount: Option<&PyDiscountCurve>) -> PyResult<(f64, f64)> {
        let disc = discount.map_or(&projection.0, |d| &d.0);
        self.0.forward_and_annuity(&index.0, &projection.0, disc).map_err(err)
    }

    /// Price with a normal (Bachelier) vol in rate units, or a shifted-lognormal vol.
    #[pyo3(signature = (index, projection, discount = None, *, normal_vol = None, lognormal_vol = None, shift = 0.0))]
    fn price(
        &self,
        index: &PyOvernightIndex,
        projection: &PyDiscountCurve,
        discount: Option<&PyDiscountCurve>,
        normal_vol: Option<f64>,
        lognormal_vol: Option<f64>,
        shift: f64,
    ) -> PyResult<PySwaptionPricing> {
        let disc = discount.map_or(&projection.0, |d| &d.0);
        self.0.price(&index.0, &projection.0, disc, vol_quote(normal_vol, lognormal_vol, shift)?).map(Into::into).map_err(err)
    }

    #[pyo3(signature = (index, projection, pv, discount = None))]
    fn implied_normal_vol(&self, index: &PyOvernightIndex, projection: &PyDiscountCurve, pv: f64, discount: Option<&PyDiscountCurve>) -> PyResult<f64> {
        let disc = discount.map_or(&projection.0, |d| &d.0);
        self.0.implied_normal_vol(&index.0, &projection.0, disc, pv).map_err(err)
    }

    #[pyo3(signature = (index, projection, pv, shift = 0.0, discount = None))]
    fn implied_lognormal_vol(
        &self,
        index: &PyOvernightIndex,
        projection: &PyDiscountCurve,
        pv: f64,
        shift: f64,
        discount: Option<&PyDiscountCurve>,
    ) -> PyResult<f64> {
        let disc = discount.map_or(&projection.0, |d| &d.0);
        self.0.implied_lognormal_vol(&index.0, &projection.0, disc, shift, pv).map_err(err)
    }

    fn __repr__(&self) -> String {
        let u = self.0.underlying();
        format!(
            "Swaption({} expiry {} strike {:.4}% on {} -> {}, notional={})",
            kind_name(self.0.kind()),
            self.0.expiry(),
            self.0.strike() * 100.0,
            u.effective_date(),
            u.maturity_date(),
            u.notional()
        )
    }
}

pub(crate) fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PySwapValuation>()?;
    m.add_class::<PyOisSwap>()?;
    m.add_class::<PySwaptionPricing>()?;
    m.add_class::<PySwaption>()?;
    Ok(())
}
