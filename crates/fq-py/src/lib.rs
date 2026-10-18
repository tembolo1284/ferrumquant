//! Python bindings for ferrumquant.
//!
//! The module is deliberately thin: every class wraps a `fq-core`/`fq-rates`
//! type by value and forwards to it, errors become `FerrumError`, and dates
//! are accepted as `ferrumquant.Date`, `datetime.date`, `"YYYY-MM-DD"` or an
//! Excel serial. Nothing is priced here that is not priced in Rust.
//!
//! Scope so far: dates and fixed-rate (Treasury) bonds. Curves, swaps,
//! futures, swaptions and caps follow the same pattern.

use std::fmt::Display;

use pyo3::create_exception;
use pyo3::exceptions::PyException;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyType};

use fq_core::time::Date;
use fq_rates::bond::{Coupon, FixedRateBond, YieldAnalytics, format_32nds, parse_32nds};

create_exception!(ferrumquant, FerrumError, PyException, "Error raised by the ferrumquant library.");

fn err<E: Display>(e: E) -> PyErr {
    FerrumError::new_err(e.to_string())
}

// ---------------------------------------------------------------------------
// Date
// ---------------------------------------------------------------------------

/// Calendar date, stored as an Excel serial (days since 1899-12-30).
#[pyclass(name = "Date", module = "ferrumquant", frozen, eq, ord, hash)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PyDate(Date);

/// Anything we accept where a date is expected.
#[derive(Debug, Clone, Copy)]
pub struct DateLike(Date);

impl<'py> FromPyObject<'py> for DateLike {
    fn extract_bound(obj: &Bound<'py, PyAny>) -> PyResult<Self> {
        if let Ok(d) = obj.downcast::<PyDate>() {
            return Ok(Self(d.get().0));
        }
        if let Ok(s) = obj.extract::<String>() {
            return s.parse::<Date>().map(Self).map_err(err);
        }
        if let Ok(serial) = obj.extract::<i32>() {
            return Date::from_serial(serial).map(Self).map_err(err);
        }
        // datetime.date / datetime.datetime / pandas.Timestamp all expose these.
        if obj.hasattr("year")? && obj.hasattr("month")? && obj.hasattr("day")? {
            let y: i32 = obj.getattr("year")?.extract()?;
            let m: u32 = obj.getattr("month")?.extract()?;
            let d: u32 = obj.getattr("day")?.extract()?;
            return Date::from_ymd(y, m, d).map(Self).map_err(err);
        }
        Err(FerrumError::new_err(format!("cannot interpret {} as a date", obj.get_type().name()?)))
    }
}

#[pymethods]
impl PyDate {
    #[new]
    fn new(year: i32, month: u32, day: u32) -> PyResult<Self> {
        Date::from_ymd(year, month, day).map(Self).map_err(err)
    }

    /// Build from a `datetime.date`, ISO string, Excel serial or `Date`.
    #[classmethod]
    fn from_any(_cls: &Bound<'_, PyType>, value: DateLike) -> Self {
        Self(value.0)
    }

    #[classmethod]
    fn from_serial(_cls: &Bound<'_, PyType>, serial: i32) -> PyResult<Self> {
        Date::from_serial(serial).map(Self).map_err(err)
    }

    #[getter]
    fn year(&self) -> i32 {
        self.0.year()
    }

    #[getter]
    fn month(&self) -> u32 {
        self.0.month()
    }

    #[getter]
    fn day(&self) -> u32 {
        self.0.day()
    }

    /// Excel serial number.
    #[getter]
    fn serial(&self) -> i32 {
        self.0.serial()
    }

    #[getter]
    fn weekday(&self) -> String {
        self.0.weekday().to_string()
    }

    fn is_end_of_month(&self) -> bool {
        self.0.is_end_of_month()
    }

    fn end_of_month(&self) -> Self {
        Self(self.0.end_of_month())
    }

    fn add_days(&self, n: i32) -> PyResult<Self> {
        self.0.checked_add_days(n).map(Self).map_err(err)
    }

    fn add_months(&self, n: i32) -> PyResult<Self> {
        self.0.add_months(n).map(Self).map_err(err)
    }

    fn add_years(&self, n: i32) -> PyResult<Self> {
        self.0.add_years(n).map(Self).map_err(err)
    }

    /// Convert to `datetime.date`.
    fn to_pydate<'py>(self_: PyRef<'py, Self>, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let (y, m, d) = self_.0.ymd();
        py.import("datetime")?.getattr("date")?.call1((y, m, d))
    }

    fn __add__(&self, days: i32) -> PyResult<Self> {
        self.0.checked_add_days(days).map(Self).map_err(err)
    }

    fn __radd__(&self, days: i32) -> PyResult<Self> {
        self.__add__(days)
    }

    /// `Date - int` is a date; `Date - Date` is a day count.
    fn __sub__<'py>(&self, py: Python<'py>, rhs: &Bound<'py, PyAny>) -> PyResult<PyObject> {
        if let Ok(other) = rhs.extract::<DateLike>() {
            if rhs.extract::<i32>().is_err() {
                return Ok((self.0 - other.0).into_pyobject(py)?.into_any().unbind());
            }
        }
        let days: i32 = rhs.extract()?;
        Ok(Py::new(py, Self(self.0.checked_add_days(-days).map_err(err)?))?.into_any())
    }

    fn __str__(&self) -> String {
        self.0.to_string()
    }

    fn __repr__(&self) -> String {
        format!("Date({})", self.0)
    }
}

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
            self.yield_rate * 100.0, self.clean_price, self.dirty_price, self.accrued, self.modified_duration, self.convexity, self.dv01
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
pub struct PyFixedRateBond(FixedRateBond);

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

    fn __repr__(&self) -> String {
        format!("FixedRateBond(coupon={:.3}%, maturity={}, face={})", self.0.coupon_rate() * 100.0, self.0.maturity(), self.0.face())
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

#[pymodule]
fn ferrumquant(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    m.add("FerrumError", m.py().get_type::<FerrumError>())?;
    m.add_class::<PyDate>()?;
    m.add_class::<PyYieldAnalytics>()?;
    m.add_class::<PyFixedRateBond>()?;
    m.add_function(wrap_pyfunction!(py_parse_32nds, m)?)?;
    m.add_function(wrap_pyfunction!(py_format_32nds, m)?)?;
    Ok(())
}
