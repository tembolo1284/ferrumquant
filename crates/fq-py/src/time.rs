//! Dates and periods.

use pyo3::prelude::*;
use pyo3::types::PyType;

use fq_core::time::{Date, Period};

use crate::{FerrumError, err};

// ---------------------------------------------------------------------------
// Date
// ---------------------------------------------------------------------------

/// Calendar date, stored as an Excel serial (days since 1899-12-30).
#[pyclass(name = "Date", module = "ferrumquant", frozen, eq, ord, hash)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PyDate(pub Date);

/// Anything we accept where a date is expected.
#[derive(Debug, Clone, Copy)]
pub struct DateLike(pub Date);

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
// Period
// ---------------------------------------------------------------------------

/// Tenor such as 1D, 2W, 6M, 10Y (12M normalises to 1Y, 7D to 1W).
#[pyclass(name = "Period", module = "ferrumquant", frozen, eq, hash)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PyPeriod(pub Period);

/// A `Period` or a string like "5Y".
#[derive(Debug, Clone, Copy)]
pub struct PeriodLike(pub Period);

impl<'py> FromPyObject<'py> for PeriodLike {
    fn extract_bound(obj: &Bound<'py, PyAny>) -> PyResult<Self> {
        if let Ok(p) = obj.downcast::<PyPeriod>() {
            return Ok(Self(p.get().0));
        }
        let s: String = obj.extract()?;
        s.parse::<Period>().map(Self).map_err(err)
    }
}

#[pymethods]
impl PyPeriod {
    /// `Period("5Y")` or `Period(5, "Y")`.
    #[new]
    #[pyo3(signature = (spec, unit = None))]
    fn new(spec: &Bound<'_, PyAny>, unit: Option<&str>) -> PyResult<Self> {
        match unit {
            Some(u) => {
                let n: i32 = spec.extract()?;
                format!("{n}{u}").parse::<Period>().map(Self).map_err(err)
            }
            None => spec.extract::<PeriodLike>().map(|p| Self(p.0)),
        }
    }

    #[getter]
    fn length(&self) -> i32 {
        self.0.length()
    }

    /// "D", "W", "M" or "Y".
    #[getter]
    fn unit(&self) -> &'static str {
        match self.0.unit() {
            fq_core::time::TimeUnit::Days => "D",
            fq_core::time::TimeUnit::Weeks => "W",
            fq_core::time::TimeUnit::Months => "M",
            fq_core::time::TimeUnit::Years => "Y",
        }
    }

    /// Approximate length in years (365.25-day years), for sorting/plotting.
    #[getter]
    fn years(&self) -> f64 {
        match (self.0.in_days(), self.0.in_months()) {
            (Some(d), _) => f64::from(d) / 365.25,
            (_, Some(m)) => f64::from(m) / 12.0,
            _ => f64::NAN,
        }
    }

    /// Calendar-add this period to a date (no business-day adjustment).
    fn add_to(&self, d: DateLike) -> PyResult<PyDate> {
        self.0.add_to(d.0).map(PyDate).map_err(err)
    }

    fn __str__(&self) -> String {
        self.0.to_string()
    }

    fn __repr__(&self) -> String {
        format!("Period({})", self.0)
    }
}

pub(crate) fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyDate>()?;
    m.add_class::<PyPeriod>()?;
    Ok(())
}
