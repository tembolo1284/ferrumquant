//! Treasury futures (conversion factors, basis, CTD, delivery option) and SOFR futures.

use pyo3::prelude::*;
use pyo3::types::PyDict;

use fq_core::time::ImmMonth;
use fq_rates::future::{
    BasisAnalytics, DeliveryMonth, DeliveryOptionConfig, DeliveryOptionResult, SofrFuture, TreasuryContract, TreasuryFuture, conversion_factor, delivery_option,
};
use fq_rates::swap::OisSwap;

use crate::bond::PyFixedRateBond;
use crate::curve::{PyDiscountCurve, PyOvernightIndex};
use crate::err;
use crate::time::{DateLike, PyDate};

fn parse_contract(s: &str) -> PyResult<TreasuryContract> {
    s.parse::<TreasuryContract>().map_err(err)
}

/// "H27", "Z26" or (year, month).
#[derive(Debug, Clone, Copy)]
pub struct DeliveryLike(DeliveryMonth);

impl<'py> FromPyObject<'py> for DeliveryLike {
    fn extract_bound(obj: &Bound<'py, PyAny>) -> PyResult<Self> {
        if let Ok((y, m)) = obj.extract::<(i32, u32)>() {
            return DeliveryMonth::new(y, m).map(Self).map_err(err);
        }
        let s: String = obj.extract()?;
        let imm = s.parse::<ImmMonth>().map_err(err)?;
        DeliveryMonth::new(imm.year(), imm.month()).map(Self).map_err(err)
    }
}

fn basis_dict<'py>(py: Python<'py>, b: &BasisAnalytics) -> PyResult<Bound<'py, PyDict>> {
    let d = PyDict::new(py);
    d.set_item("clean_price", b.clean_price)?;
    d.set_item("conversion_factor", b.conversion_factor)?;
    d.set_item("gross_basis", b.gross_basis)?;
    d.set_item("carry", b.carry)?;
    d.set_item("net_basis", b.net_basis)?;
    d.set_item("forward_clean", b.forward_clean)?;
    d.set_item("converted_forward", b.converted_forward)?;
    d.set_item("implied_repo", b.implied_repo)?;
    Ok(d)
}

/// CME conversion factor: price per 1 face at a 6% yield, coupon rounded to eighths.
#[pyfunction]
#[pyo3(name = "conversion_factor")]
fn py_conversion_factor(contract: &str, coupon: f64, maturity: DateLike, delivery: DeliveryLike) -> PyResult<f64> {
    conversion_factor(parse_contract(contract)?, coupon, maturity.0, delivery.0).map_err(err)
}

/// Treasury future with its deliverable basket.
#[pyclass(name = "TreasuryFuture", module = "ferrumquant", frozen)]
#[derive(Debug, Clone)]
pub struct PyTreasuryFuture(pub TreasuryFuture);

#[pymethods]
impl PyTreasuryFuture {
    /// `TreasuryFuture("ZN", "H27", [bond, ...])`: non-deliverable candidates are dropped.
    #[new]
    fn new(contract: &str, delivery: DeliveryLike, candidates: Vec<PyRef<'_, PyFixedRateBond>>) -> PyResult<Self> {
        TreasuryFuture::new(parse_contract(contract)?, delivery.0, candidates.iter().map(|b| b.0.clone())).map(Self).map_err(err)
    }

    #[getter]
    fn contract(&self) -> String {
        self.0.contract().to_string()
    }

    /// Delivery month as (year, month).
    #[getter]
    fn delivery_month(&self) -> (i32, u32) {
        (self.0.delivery_month().year(), self.0.delivery_month().month())
    }

    /// `[(bond, conversion_factor), ...]` in input order.
    fn basket(&self) -> Vec<(PyFixedRateBond, f64)> {
        self.0.basket().iter().map(|d| (PyFixedRateBond(d.bond.clone()), d.conversion_factor)).collect()
    }

    fn first_delivery_date(&self) -> PyResult<PyDate> {
        self.0.first_delivery_date().map(PyDate).map_err(err)
    }

    fn last_delivery_date(&self) -> PyResult<PyDate> {
        self.0.last_delivery_date().map(PyDate).map_err(err)
    }

    /// F * CF + accrued at delivery, per 100 face.
    fn invoice_price(&self, index: usize, futures_price: f64, delivery_date: DateLike) -> PyResult<f64> {
        self.0.invoice_price(index, futures_price, delivery_date.0).map_err(err)
    }

    fn forward_clean_price(&self, index: usize, clean_price: f64, settle: DateLike, delivery_date: DateLike, repo: f64) -> PyResult<f64> {
        self.0.forward_clean_price(index, clean_price, settle.0, delivery_date.0, repo).map_err(err)
    }

    /// Basis measures for one deliverable: gross/net basis, carry, implied repo.
    #[allow(clippy::too_many_arguments)]
    fn basis<'py>(
        &self,
        py: Python<'py>,
        index: usize,
        clean_price: f64,
        futures_price: f64,
        settle: DateLike,
        delivery_date: DateLike,
        repo: f64,
    ) -> PyResult<Bound<'py, PyDict>> {
        basis_dict(py, &self.0.basis(index, clean_price, futures_price, settle.0, delivery_date.0, repo).map_err(err)?)
    }

    /// (ctd index, basis dict per deliverable); CTD = highest implied repo.
    fn cheapest_to_deliver<'py>(
        &self,
        py: Python<'py>,
        clean_prices: Vec<f64>,
        futures_price: f64,
        settle: DateLike,
        delivery_date: DateLike,
        repo: f64,
    ) -> PyResult<(usize, Vec<Bound<'py, PyDict>>)> {
        let (ctd, all) = self.0.cheapest_to_deliver(&clean_prices, futures_price, settle.0, delivery_date.0, repo).map_err(err)?;
        Ok((ctd, all.iter().map(|b| basis_dict(py, b)).collect::<PyResult<_>>()?))
    }

    /// (ctd index, fair futures price) = min over the basket of forward clean / CF.
    fn fair_price(&self, clean_prices: Vec<f64>, settle: DateLike, delivery_date: DateLike, repo: f64) -> PyResult<(usize, f64)> {
        self.0.fair_price(&clean_prices, settle.0, delivery_date.0, repo).map_err(err)
    }

    /// Switch-option value under a parallel normal yield shift.
    #[pyo3(signature = (clean_prices, settle, delivery_date, repo, yield_vol = 0.009, std_devs = 6.0, points = 241))]
    #[allow(clippy::too_many_arguments)]
    fn delivery_option(
        &self,
        clean_prices: Vec<f64>,
        settle: DateLike,
        delivery_date: DateLike,
        repo: f64,
        yield_vol: f64,
        std_devs: f64,
        points: usize,
    ) -> PyResult<PyDeliveryOption> {
        let cfg = DeliveryOptionConfig { yield_vol, std_devs, points };
        delivery_option(&self.0, &clean_prices, settle.0, delivery_date.0, repo, cfg).map(PyDeliveryOption).map_err(err)
    }

    fn __len__(&self) -> usize {
        self.0.basket().len()
    }

    fn __repr__(&self) -> String {
        format!(
            "TreasuryFuture({} {}{}, basket={})",
            self.0.contract(),
            self.0.delivery_month().month_code(),
            self.0.delivery_month().year() % 100,
            self.0.basket().len()
        )
    }
}

/// Delivery-option result.
#[pyclass(name = "DeliveryOption", module = "ferrumquant", frozen)]
#[derive(Debug, Clone)]
pub struct PyDeliveryOption(DeliveryOptionResult);

#[pymethods]
impl PyDeliveryOption {
    #[getter]
    fn ctd_only_price(&self) -> f64 {
        self.0.ctd_only_price
    }

    #[getter]
    fn fair_price(&self) -> f64 {
        self.0.fair_price
    }

    /// In price points (>= 0).
    #[getter]
    fn switch_option(&self) -> f64 {
        self.0.switch_option
    }

    #[getter]
    fn expected_converted(&self) -> Vec<f64> {
        self.0.expected_converted.clone()
    }

    #[getter]
    fn ctd_probability(&self) -> Vec<f64> {
        self.0.ctd_probability.clone()
    }

    #[getter]
    fn forward_yields(&self) -> Vec<f64> {
        self.0.forward_yields.clone()
    }

    fn most_likely_ctd(&self) -> usize {
        self.0.most_likely_ctd()
    }

    fn __repr__(&self) -> String {
        format!("DeliveryOption(fair={:.6}, ctd_only={:.6}, switch_option={:.6})", self.0.fair_price, self.0.ctd_only_price, self.0.switch_option)
    }
}

// ---------------------------------------------------------------------------
// SOFR futures
// ---------------------------------------------------------------------------

/// Three-month SOFR future (SR3): 100 - compounded rate over the IMM quarter.
#[pyclass(name = "SofrFuture", module = "ferrumquant", frozen)]
#[derive(Debug, Clone)]
pub struct PySofrFuture(pub SofrFuture);

#[pymethods]
impl PySofrFuture {
    /// `SofrFuture("Z26", 95.78, convexity=0.0002)`; convexity in rate units.
    #[new]
    #[pyo3(signature = (contract, price, convexity = 0.0))]
    fn new(contract: &str, price: f64, convexity: f64) -> PyResult<Self> {
        let imm = contract.parse::<ImmMonth>().map_err(err)?;
        SofrFuture::new(imm, price, convexity, &OisSwap::usd_sofr_calendar()).map(Self).map_err(err)
    }

    #[getter]
    fn contract(&self) -> String {
        self.0.contract().to_string()
    }

    #[getter]
    fn start(&self) -> PyDate {
        PyDate(self.0.start())
    }

    #[getter]
    fn end(&self) -> PyDate {
        PyDate(self.0.end())
    }

    #[getter]
    fn price(&self) -> f64 {
        self.0.price()
    }

    #[getter]
    fn convexity_adjustment(&self) -> f64 {
        self.0.convexity_adjustment()
    }

    /// (100 - price) / 100.
    #[getter]
    fn futures_rate(&self) -> f64 {
        self.0.futures_rate()
    }

    /// Futures rate less the convexity adjustment: the forward the curve should show.
    #[getter]
    fn implied_forward(&self) -> f64 {
        self.0.implied_forward()
    }

    /// Compounded SOFR over the contract period off the curve.
    fn curve_rate(&self, index: &PyOvernightIndex, curve: &PyDiscountCurve) -> PyResult<f64> {
        self.0.curve_rate(&index.0, &curve.0).map_err(err)
    }

    fn fair_price(&self, index: &PyOvernightIndex, curve: &PyDiscountCurve) -> PyResult<f64> {
        self.0.fair_price(&index.0, &curve.0).map_err(err)
    }

    fn __repr__(&self) -> String {
        format!("SofrFuture({} @ {:.4}, {} -> {})", self.0.contract(), self.0.price(), self.0.start(), self.0.end())
    }
}

pub(crate) fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyTreasuryFuture>()?;
    m.add_class::<PyDeliveryOption>()?;
    m.add_class::<PySofrFuture>()?;
    m.add_function(wrap_pyfunction!(py_conversion_factor, m)?)?;
    Ok(())
}
