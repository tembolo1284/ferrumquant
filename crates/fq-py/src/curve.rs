//! Discount curves, the overnight index, and the SOFR bootstrap.
//!
//! Instruments for the bootstrap are plain Python tuples so a quote sheet
//! can be typed or loaded from a file without constructing objects:
//!
//! ```text
//! ("DEPO", "1D", 0.0432)            # cash deposit, simple ACT/360
//! ("SR3",  "Z26", 95.78, 0.0002)    # SOFR future: IMM code, price, convexity (rate units, optional)
//! ("OIS",  "5Y", 0.0378)            # spot-starting USD SOFR OIS par rate
//! ```

use pyo3::prelude::*;
use pyo3::types::{PyDict, PyTuple};

use fq_core::time::{DayCount, Period};
use fq_rates::curve::{
    BootstrapConfig, BootstrapInstrument, BootstrapResult, CurveError, CurveJacobian, DiscountCurve, Interpolation, bootstrap_sofr,
    bootstrap_sofr_with_jacobian, zero_bucket_ladder,
};
use fq_rates::index::OvernightIndex;

use crate::time::{DateLike, PyDate};
use crate::{FerrumError, err};

fn parse_interpolation(s: &str) -> PyResult<Interpolation> {
    match s.to_ascii_lowercase().replace(['-', ' '], "_").as_str() {
        "log_linear" | "loglinear" | "log_linear_discount" => Ok(Interpolation::LogLinearDiscount),
        "linear_zero" | "linearzero" => Ok(Interpolation::LinearZero),
        _ => Err(FerrumError::new_err(format!("unknown interpolation '{s}' (log_linear | linear_zero)"))),
    }
}

fn interpolation_name(i: Interpolation) -> &'static str {
    match i {
        Interpolation::LogLinearDiscount => "log_linear",
        Interpolation::LinearZero => "linear_zero",
    }
}

fn parse_day_count(s: &str) -> PyResult<DayCount> {
    s.parse::<DayCount>().map_err(err)
}

// ---------------------------------------------------------------------------
// DiscountCurve
// ---------------------------------------------------------------------------

/// Discount curve on ACT/365F time from its reference date.
#[pyclass(name = "DiscountCurve", module = "ferrumquant", frozen)]
#[derive(Debug, Clone)]
pub struct PyDiscountCurve(pub DiscountCurve);

#[pymethods]
impl PyDiscountCurve {
    /// `DiscountCurve(reference, [(date, df), ...], interpolation="log_linear")`.
    #[new]
    #[pyo3(signature = (reference, nodes, interpolation = "log_linear"))]
    fn new(reference: DateLike, nodes: Vec<(DateLike, f64)>, interpolation: &str) -> PyResult<Self> {
        let nodes: Vec<_> = nodes.into_iter().map(|(d, df)| (d.0, df)).collect();
        DiscountCurve::new(reference.0, &nodes, parse_interpolation(interpolation)?).map(Self).map_err(err)
    }

    /// Build from continuously-compounded ACT/365F zero rates at the node dates.
    #[staticmethod]
    #[pyo3(signature = (reference, nodes, interpolation = "log_linear"))]
    fn from_zero_rates(reference: DateLike, nodes: Vec<(DateLike, f64)>, interpolation: &str) -> PyResult<Self> {
        let nodes: Vec<_> = nodes.into_iter().map(|(d, r)| (d.0, r)).collect();
        DiscountCurve::from_zero_rates(reference.0, &nodes, parse_interpolation(interpolation)?).map(Self).map_err(err)
    }

    /// Flat continuously-compounded curve out to `horizon`.
    #[staticmethod]
    fn flat(reference: DateLike, rate: f64, horizon: DateLike) -> PyResult<Self> {
        DiscountCurve::flat(reference.0, rate, horizon.0).map(Self).map_err(err)
    }

    #[getter]
    fn reference_date(&self) -> PyDate {
        PyDate(self.0.reference_date())
    }

    #[getter]
    fn interpolation(&self) -> &'static str {
        interpolation_name(self.0.interpolation())
    }

    #[getter]
    fn node_count(&self) -> usize {
        self.0.node_count()
    }

    /// `[(date, df), ...]` excluding the reference node.
    fn nodes(&self) -> Vec<(PyDate, f64)> {
        self.0.nodes().into_iter().map(|(d, df)| (PyDate(d), df)).collect()
    }

    fn node_date(&self, i: usize) -> PyResult<PyDate> {
        self.0.node_date(i).map(PyDate).map_err(err)
    }

    fn node_zero_rate(&self, i: usize) -> PyResult<f64> {
        self.0.node_zero_rate(i).map_err(err)
    }

    /// ACT/365F year fraction from the reference date.
    fn time(&self, d: DateLike) -> f64 {
        self.0.time(d.0)
    }

    fn discount(&self, d: DateLike) -> PyResult<f64> {
        self.0.discount(d.0).map_err(err)
    }

    /// Discount factor at an ACT/365F time (no range check beyond t >= 0).
    fn discount_t(&self, t: f64) -> f64 {
        self.0.discount_t(t)
    }

    /// Continuously-compounded ACT/365F zero rate.
    fn zero_rate(&self, d: DateLike) -> PyResult<f64> {
        self.0.zero_rate(d.0).map_err(err)
    }

    /// Simple forward rate between two dates in the given day count.
    #[pyo3(signature = (d1, d2, day_count = "ACT/360"))]
    fn forward_rate(&self, d1: DateLike, d2: DateLike, day_count: &str) -> PyResult<f64> {
        self.0.forward_rate(d1.0, d2.0, parse_day_count(day_count)?).map_err(err)
    }

    /// Parallel shift of all zero rates by `shift` (rate units: 1e-4 = 1bp).
    fn shifted(&self, shift: f64) -> Self {
        Self(self.0.shifted(shift))
    }

    /// Shift one node's zero rate.
    fn with_node_shift(&self, i: usize, shift: f64) -> PyResult<Self> {
        self.0.with_node_shift(i, shift).map(Self).map_err(err)
    }

    fn __repr__(&self) -> String {
        format!(
            "DiscountCurve(reference={}, nodes={}, interpolation={})",
            self.0.reference_date(),
            self.0.node_count(),
            interpolation_name(self.0.interpolation())
        )
    }
}

// ---------------------------------------------------------------------------
// OvernightIndex
// ---------------------------------------------------------------------------

/// Overnight index with its fixing history (SOFR: USGS calendar, ACT/360).
#[pyclass(name = "OvernightIndex", module = "ferrumquant")]
#[derive(Debug, Clone)]
pub struct PyOvernightIndex(pub OvernightIndex);

#[pymethods]
impl PyOvernightIndex {
    #[staticmethod]
    fn sofr() -> Self {
        Self(OvernightIndex::sofr())
    }

    #[getter]
    fn name(&self) -> String {
        self.0.name().to_owned()
    }

    fn add_fixing(&mut self, date: DateLike, rate: f64) {
        self.0.add_fixing(date.0, rate);
    }

    fn add_fixings(&mut self, fixings: Vec<(DateLike, f64)>) {
        self.0.add_fixings(fixings.into_iter().map(|(d, r)| (d.0, r)));
    }

    fn fixing(&self, date: DateLike) -> Option<f64> {
        self.0.fixing(date.0)
    }

    fn fixings(&self) -> Vec<(PyDate, f64)> {
        self.0.fixings().iter().map(|(&d, &r)| (PyDate(d), r)).collect()
    }

    fn __repr__(&self) -> String {
        format!("OvernightIndex({}, fixings={})", self.0.name(), self.0.fixings().len())
    }
}

// ---------------------------------------------------------------------------
// Bootstrap
// ---------------------------------------------------------------------------

fn parse_instrument(obj: &Bound<'_, PyAny>) -> PyResult<BootstrapInstrument> {
    let t = obj.downcast::<PyTuple>().map_err(|_| FerrumError::new_err("instrument must be a tuple like ('OIS', '5Y', 0.0378)"))?;
    let kind: String = t.get_item(0)?.extract()?;
    let key: String = t.get_item(1)?.extract()?;
    let bad = |msg: &str| FerrumError::new_err(format!("instrument {kind} {key}: {msg}"));
    match kind.to_ascii_uppercase().as_str() {
        "DEPO" | "DEPOSIT" => {
            if t.len() != 3 {
                return Err(bad("expected (\"DEPO\", tenor, rate)"));
            }
            Ok(BootstrapInstrument::deposit(key.parse::<Period>().map_err(err)?, t.get_item(2)?.extract()?))
        }
        "SR3" | "FUT" | "FUTURE" => {
            if t.len() != 3 && t.len() != 4 {
                return Err(bad("expected (\"SR3\", imm_code, price[, convexity])"));
            }
            let convexity = if t.len() == 4 { t.get_item(3)?.extract()? } else { 0.0 };
            Ok(BootstrapInstrument::sofr_future(key.parse().map_err(err)?, t.get_item(2)?.extract()?, convexity))
        }
        "OIS" | "SWAP" => {
            if t.len() != 3 {
                return Err(bad("expected (\"OIS\", tenor, rate)"));
            }
            Ok(BootstrapInstrument::ois(key.parse::<Period>().map_err(err)?, t.get_item(2)?.extract()?))
        }
        _ => Err(bad("unknown kind (DEPO | SR3 | OIS)")),
    }
}

fn parse_instruments(items: &Bound<'_, PyAny>) -> PyResult<Vec<BootstrapInstrument>> {
    items.try_iter()?.map(|o| parse_instrument(&o?)).collect()
}

fn config(interpolation: &str) -> PyResult<BootstrapConfig> {
    Ok(BootstrapConfig { interpolation: parse_interpolation(interpolation)?, ..BootstrapConfig::default() })
}

/// Bootstrapped curve plus its nodes, in node order.
#[pyclass(name = "BootstrapResult", module = "ferrumquant", frozen)]
#[derive(Debug, Clone)]
pub struct PyBootstrapResult {
    inner: BootstrapResult,
    index: OvernightIndex,
}

#[pymethods]
impl PyBootstrapResult {
    #[getter]
    fn curve(&self) -> PyDiscountCurve {
        PyDiscountCurve(self.inner.curve.clone())
    }

    /// Instrument keys as strings, e.g. "DEPO 1D", "SR3 Z26", "OIS 5Y".
    #[getter]
    fn keys(&self) -> Vec<String> {
        self.inner.keys().iter().map(ToString::to_string).collect()
    }

    /// `[(key, pillar_date, discount_factor), ...]`.
    fn nodes(&self) -> Vec<(String, PyDate, f64)> {
        self.inner.nodes.iter().map(|(k, d, df)| (k.to_string(), PyDate(*d), *df)).collect()
    }

    /// Repricing error of each input instrument on the built curve (should be ~0).
    fn repricing_errors(&self) -> PyResult<Vec<f64>> {
        self.inner.instruments.iter().map(|b| b.npv(&self.index, &self.inner.curve).map_err(err)).collect()
    }

    /// One dict per node: key, pillar, df, zero (cc ACT/365F), time.
    fn table<'py>(&self, py: Python<'py>) -> PyResult<Vec<Bound<'py, PyDict>>> {
        let errors = self.repricing_errors()?;
        self.inner
            .nodes
            .iter()
            .zip(errors)
            .map(|((k, d, df), e)| {
                let row = PyDict::new(py);
                row.set_item("key", k.to_string())?;
                row.set_item("pillar", PyDate(*d))?;
                row.set_item("time", self.inner.curve.time(*d))?;
                row.set_item("df", *df)?;
                row.set_item("zero", self.inner.curve.zero_rate(*d).map_err(err)?)?;
                row.set_item("repricing_error", e)?;
                Ok(row)
            })
            .collect()
    }

    fn __len__(&self) -> usize {
        self.inner.nodes.len()
    }

    fn __repr__(&self) -> String {
        format!("BootstrapResult(reference={}, nodes={})", self.inner.curve.reference_date(), self.inner.nodes.len())
    }
}

/// Bumped-quote sensitivities of a bootstrapped curve.
#[pyclass(name = "CurveJacobian", module = "ferrumquant", frozen)]
#[derive(Debug, Clone)]
pub struct PyCurveJacobian(pub CurveJacobian);

#[pymethods]
impl PyCurveJacobian {
    #[getter]
    fn keys(&self) -> Vec<String> {
        self.0.keys().iter().map(ToString::to_string).collect()
    }

    /// `matrix[i][j]` = d(zero_i) / d(quote_j), rate units.
    #[getter]
    fn matrix(&self) -> Vec<Vec<f64>> {
        self.0.matrix().to_vec()
    }

    #[getter]
    fn bump(&self) -> f64 {
        self.0.bump()
    }

    #[getter]
    fn node_count(&self) -> usize {
        self.0.node_count()
    }

    /// Convert a zero-rate bucket ladder (dPV/dzero_i) to a par-quote ladder (dPV/dquote_j).
    fn to_par_ladder(&self, zero_ladder: Vec<f64>) -> PyResult<Vec<f64>> {
        self.0.to_par_ladder(&zero_ladder).map_err(err)
    }

    fn __repr__(&self) -> String {
        format!("CurveJacobian(nodes={}, bump={})", self.0.node_count(), self.0.bump())
    }
}

/// Bootstrap a SOFR discount curve from deposits, SR3 futures and OIS quotes.
#[pyfunction]
#[pyo3(name = "bootstrap_sofr", signature = (trade_date, index, instruments, interpolation = "log_linear"))]
fn py_bootstrap_sofr(trade_date: DateLike, index: &PyOvernightIndex, instruments: &Bound<'_, PyAny>, interpolation: &str) -> PyResult<PyBootstrapResult> {
    let inst = parse_instruments(instruments)?;
    let inner = bootstrap_sofr(trade_date.0, &index.0, &inst, config(interpolation)?).map_err(err)?;
    Ok(PyBootstrapResult { inner, index: index.0.clone() })
}

/// Bootstrap and build the quote Jacobian (2n+1 bootstraps, central differences).
#[pyfunction]
#[pyo3(name = "bootstrap_sofr_with_jacobian", signature = (trade_date, index, instruments, bump = 1e-4, interpolation = "log_linear"))]
fn py_bootstrap_sofr_with_jacobian(
    trade_date: DateLike,
    index: &PyOvernightIndex,
    instruments: &Bound<'_, PyAny>,
    bump: f64,
    interpolation: &str,
) -> PyResult<(PyBootstrapResult, PyCurveJacobian)> {
    let inst = parse_instruments(instruments)?;
    let (inner, jac) = bootstrap_sofr_with_jacobian(trade_date.0, &index.0, &inst, config(interpolation)?, bump).map_err(err)?;
    Ok((PyBootstrapResult { inner, index: index.0.clone() }, PyCurveJacobian(jac)))
}

/// Zero-rate bucket ladder for any Python pricer: `price_fn(curve) -> float`
/// is called with each node bumped up and down; returns dPV/dzero_i per node
/// (per unit rate; multiply by 1e-4 for per-bp).
#[pyfunction]
#[pyo3(name = "zero_bucket_ladder", signature = (curve, price_fn, bump = 1e-6))]
fn py_zero_bucket_ladder(py: Python<'_>, curve: &PyDiscountCurve, price_fn: &Bound<'_, PyAny>, bump: f64) -> PyResult<Vec<f64>> {
    zero_bucket_ladder::<PyErrWrapper>(&curve.0, bump, |c| {
        let bumped = Py::new(py, PyDiscountCurve(c.clone()))?;
        Ok(price_fn.call1((bumped,))?.extract::<f64>()?)
    })
    .map_err(PyErr::from)
}

impl From<CurveError> for PyErrWrapper {
    fn from(e: CurveError) -> Self {
        Self(err(e))
    }
}

/// `zero_bucket_ladder` needs `E: From<CurveError>`; `PyErr` is foreign, so
/// route through a newtype.
#[derive(Debug)]
pub struct PyErrWrapper(PyErr);

impl From<PyErr> for PyErrWrapper {
    fn from(e: PyErr) -> Self {
        Self(e)
    }
}

impl From<PyErrWrapper> for PyErr {
    fn from(w: PyErrWrapper) -> Self {
        w.0
    }
}

pub(crate) fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyDiscountCurve>()?;
    m.add_class::<PyOvernightIndex>()?;
    m.add_class::<PyBootstrapResult>()?;
    m.add_class::<PyCurveJacobian>()?;
    m.add_function(wrap_pyfunction!(py_bootstrap_sofr, m)?)?;
    m.add_function(wrap_pyfunction!(py_bootstrap_sofr_with_jacobian, m)?)?;
    m.add_function(wrap_pyfunction!(py_zero_bucket_ladder, m)?)?;
    Ok(())
}
