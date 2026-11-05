//! Fixed-rate bonds.

use pyo3::prelude::*;
use pyo3::types::PyDict;

use fq_rates::bond::{BondSpreads, Coupon, FixedRateBond, YieldAnalytics, format_32nds, parse_32nds, spreads};

use crate::curve::{PyDiscountCurve, PyOvernightIndex};
use crate::err;
use crate::time::{DateLike, PyDate};

// ---------------------------------------------------------------------------
// Bonds
// ---------------------------------------------------------------------------

/// Yield-based price and risk measures per 100 face.
#[pyclass(name = "YieldAnalytics", module = "ferrumquant", frozen, get_all)]
#[derive(Debug, Clone, Copy)]
pub struct PyYieldAnalytics {
    /// Street yield, semiannual bond basis, as a decimal.
    yield_rate: f64,
    clean_price: f64,
    dirty_price: f64,
    accrued: f64,
    macaulay_duration: f64,
    modified_duration: f64,
    convexity: f64,
    /// Dirty-price change per 100 face for a 1bp fall in yield.
    dv01: f64,
}

impl From<YieldAnalytics> for PyYieldAnalytics {
    fn from(a: YieldAnalytics) -> Self {
        Self {
            yield_rate: a.yield_rate,
            clean_price: a.clean_price,
            dirty_price: a.dirty_price,
            accrued: a.accrued,
            macaulay_duration: a.macaulay_duration,
            modified_duration: a.modified_duration,
            convexity: a.convexity,
            dv01: a.dv01,
        }
    }
}

#[pymethods]
impl PyYieldAnalytics {
    fn to_dict<'py>(self_: PyRef<'py, Self>, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let d = PyDict::new(py);
        d.set_item("yield_rate", self_.yield_rate)?;
        d.set_item("clean_price", self_.clean_price)?;
        d.set_item("dirty_price", self_.dirty_price)?;
        d.set_item("accrued", self_.accrued)?;
        d.set_item("macaulay_duration", self_.macaulay_duration)?;
        d.set_item("modified_duration", self_.modified_duration)?;
        d.set_item("convexity", self_.convexity)?;
        d.set_item("dv01", self_.dv01)?;
        Ok(d)
    }

    fn __repr__(&self) -> String {
        format!(
            "YieldAnalytics(yield={:.6}%, clean={:.6}, dirty={:.6}, accrued={:.6}, mod_dur={:.4}, convexity={:.4}, dv01={:.6})",
            self.yield_rate * 100.0,
            self.clean_price,
            self.dirty_price,
            self.accrued,
            self.modified_duration,
            self.convexity,
            self.dv01
        )
    }
}

fn coupon_dict<'py>(py: Python<'py>, c: &Coupon) -> PyResult<Bound<'py, PyDict>> {
    let d = PyDict::new(py);
    d.set_item("accrual_start", PyDate(c.accrual_start))?;
    d.set_item("accrual_end", PyDate(c.accrual_end))?;
    d.set_item("payment_date", PyDate(c.payment_date))?;
    d.set_item("amount", c.amount)?;
    Ok(d)
}

/// Fixed-rate bullet bond.
#[pyclass(name = "FixedRateBond", module = "ferrumquant", frozen)]
#[derive(Debug, Clone)]
pub struct PyFixedRateBond(pub FixedRateBond);

#[pymethods]
impl PyFixedRateBond {
    /// US Treasury note/bond: semiannual, ACT/ACT ICMA, unadjusted
    /// accrual dates, T+1 settlement on the US government bond calendar.
    /// `first_coupon` is only needed for an odd first period.
    #[staticmethod]
    #[pyo3(signature = (dated, maturity, coupon, first_coupon = None))]
    fn us_treasury(dated: DateLike, maturity: DateLike, coupon: f64, first_coupon: Option<DateLike>) -> PyResult<Self> {
        FixedRateBond::us_treasury(dated.0, maturity.0, coupon, first_coupon.map(|d| d.0)).map(Self).map_err(err)
    }

    #[getter]
    fn coupon_rate(&self) -> f64 {
        self.0.coupon_rate()
    }

    #[getter]
    fn face(&self) -> f64 {
        self.0.face()
    }

    #[getter]
    fn maturity(&self) -> PyDate {
        PyDate(self.0.maturity())
    }

    #[getter]
    fn dated_date(&self) -> Option<PyDate> {
        self.0.coupons().first().map(|c| PyDate(c.accrual_start))
    }

    /// Coupon rows as dicts: accrual_start, accrual_end, payment_date, amount.
    fn coupons<'py>(&self, py: Python<'py>) -> PyResult<Vec<Bound<'py, PyDict>>> {
        self.0.coupons().iter().map(|c| coupon_dict(py, c)).collect()
    }

    /// Settlement date for a trade date (T+1 on the bond's calendar).
    fn settlement_date(&self, trade_date: DateLike) -> PyResult<PyDate> {
        self.0.settlement_date(trade_date.0).map(PyDate).map_err(err)
    }

    fn accrued(&self, settle: DateLike) -> PyResult<f64> {
        self.0.accrued(settle.0).map_err(err)
    }

    /// Clean price per 100 for a street yield (decimal).
    fn clean_price(&self, yield_rate: f64, settle: DateLike) -> PyResult<f64> {
        self.0.clean_price(yield_rate, settle.0).map_err(err)
    }

    fn dirty_price(&self, yield_rate: f64, settle: DateLike) -> PyResult<f64> {
        self.0.dirty_price(yield_rate, settle.0).map_err(err)
    }

    /// Street yield (decimal) from a clean price per 100.
    fn yield_from_clean(&self, clean_price: f64, settle: DateLike) -> PyResult<f64> {
        self.0.yield_from_clean(clean_price, settle.0).map_err(err)
    }

    /// Full analytics at a given yield.
    fn analytics(&self, yield_rate: f64, settle: DateLike) -> PyResult<PyYieldAnalytics> {
        self.0.analytics(yield_rate, settle.0).map(Into::into).map_err(err)
    }

    /// Full analytics from a clean price (solves the yield first).
    fn analytics_from_price(&self, clean_price: f64, settle: DateLike) -> PyResult<PyYieldAnalytics> {
        let y = self.0.yield_from_clean(clean_price, settle.0).map_err(err)?;
        self.0.analytics(y, settle.0).map(Into::into).map_err(err)
    }

    /// Dirty PV per 100 of the remaining cashflows on a discount curve (from its reference date).
    fn npv(&self, curve: &PyDiscountCurve) -> PyResult<f64> {
        self.0.npv(&curve.0).map_err(err)
    }

    /// PV change for a +1bp parallel curve shift (curve convention: negative for a long).
    fn curve_dv01(&self, curve: &PyDiscountCurve) -> PyResult<f64> {
        self.0.dv01(&curve.0).map_err(err)
    }

    /// Z-spread, I-spread and asset-swap spread of a clean price over the SOFR curve.
    fn spreads(&self, clean_price: f64, settle: DateLike, index: &PyOvernightIndex, curve: &PyDiscountCurve) -> PyResult<PyBondSpreads> {
        spreads(&self.0, clean_price, settle.0, &index.0, &curve.0).map(Into::into).map_err(err)
    }

    fn __repr__(&self) -> String {
        format!("FixedRateBond(coupon={:.3}%, maturity={}, face={})", self.0.coupon_rate() * 100.0, self.0.maturity(), self.0.face())
    }
}

/// Spreads of a bond price over the swap curve.
#[pyclass(name = "BondSpreads", module = "ferrumquant", frozen, get_all)]
#[derive(Debug, Clone, Copy)]
pub struct PyBondSpreads {
    clean_price: f64,
    dirty_price: f64,
    yield_rate: f64,
    /// Dirty PV per 100 of the cashflows on the curve, from settlement.
    curve_pv: f64,
    /// Continuously-compounded spread over the curve that reprices the bond.
    z_spread: f64,
    /// Yield minus the matched-maturity swap rate.
    i_spread: f64,
    /// Par-par asset swap spread.
    asset_swap_spread: f64,
}

impl From<BondSpreads> for PyBondSpreads {
    fn from(s: BondSpreads) -> Self {
        Self {
            clean_price: s.clean_price,
            dirty_price: s.dirty_price,
            yield_rate: s.yield_rate,
            curve_pv: s.curve_pv,
            z_spread: s.z_spread,
            i_spread: s.i_spread,
            asset_swap_spread: s.asset_swap_spread,
        }
    }
}

#[pymethods]
impl PyBondSpreads {
    fn to_dict<'py>(self_: PyRef<'py, Self>, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let d = PyDict::new(py);
        d.set_item("clean_price", self_.clean_price)?;
        d.set_item("dirty_price", self_.dirty_price)?;
        d.set_item("yield_rate", self_.yield_rate)?;
        d.set_item("curve_pv", self_.curve_pv)?;
        d.set_item("z_spread", self_.z_spread)?;
        d.set_item("i_spread", self_.i_spread)?;
        d.set_item("asset_swap_spread", self_.asset_swap_spread)?;
        Ok(d)
    }

    fn __repr__(&self) -> String {
        format!("BondSpreads(z={:.2}bp, i={:.2}bp, asw={:.2}bp)", self.z_spread * 1e4, self.i_spread * 1e4, self.asset_swap_spread * 1e4)
    }
}

/// Parse a Treasury price quote in 32nds: "99-16", "99-16+", "99-162", "99-16 3/8".
#[pyfunction]
#[pyo3(name = "parse_32nds")]
fn py_parse_32nds(quote: &str) -> PyResult<f64> {
    parse_32nds(quote).map_err(err)
}

/// Format a decimal price in 32nds with eighths: 99.515625 -> "99-16+".
#[pyfunction]
#[pyo3(name = "format_32nds")]
fn py_format_32nds(price: f64) -> String {
    format_32nds(price)
}

pub(crate) fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyYieldAnalytics>()?;
    m.add_class::<PyFixedRateBond>()?;
    m.add_class::<PyBondSpreads>()?;
    m.add_function(wrap_pyfunction!(py_parse_32nds, m)?)?;
    m.add_function(wrap_pyfunction!(py_format_32nds, m)?)?;
    Ok(())
}
