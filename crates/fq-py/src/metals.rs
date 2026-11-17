//! Metals: precious forwards and vanillas off a lease curve, LME prompt
//! dates, LME forward curves, carries and third-Wednesday options.

use pyo3::prelude::*;
use pyo3::types::PyDict;

use fq_core::math::BlackResult;
use fq_metals::lme::{prompts, six_month_prompt};
use fq_metals::precious::flat_lease_curve;
use fq_metals::{
    LmeCalendar, LmeCarry, LmeCurve, LmeForward, LmeOption, Metal, PreciousForward, PreciousMarket, PreciousVanilla, PromptKind, cash_prompt, classify_prompt,
    declaration_day, is_valid_prompt, third_wednesday, three_month_prompt,
};

use crate::commod::{MonthLike, PyHub, kind_name, parse_dc, parse_kind};
use crate::curve::PyDiscountCurve;
use crate::time::{DateLike, PyDate};
use crate::{FerrumError, dict, err};

/// Accepts the LME/ISO code (`CA`, `XAU`) or the plain name (`copper`, `gold`).
pub(crate) fn parse_metal(s: &str) -> PyResult<Metal> {
    Metal::from_code(s)
        .or_else(|| Metal::PRECIOUS.into_iter().chain(Metal::BASE).find(|m| m.name().eq_ignore_ascii_case(s)))
        .or_else(|| if s.eq_ignore_ascii_case("aluminum") { Some(Metal::Aluminium) } else { None })
        .ok_or_else(|| FerrumError::new_err(format!("unknown metal '{s}'; see metals()")))
}

fn prompt_kind_name(k: PromptKind) -> &'static str {
    match k {
        PromptKind::Cash => "cash",
        PromptKind::Daily => "daily",
        PromptKind::ThreeMonth => "3m",
        PromptKind::Weekly => "weekly",
        PromptKind::Monthly => "monthly",
    }
}

fn black_dict<'py>(py: Python<'py>, b: &BlackResult) -> PyResult<Bound<'py, PyDict>> {
    Ok(dict!(py, "price" => b.price, "delta" => b.delta, "gamma" => b.gamma, "vega" => b.vega))
}

/// All metal codes with name, unit, lot size and precious flag.
#[pyfunction]
#[pyo3(name = "metals")]
fn py_metals(py: Python<'_>) -> PyResult<Bound<'_, PyDict>> {
    let d = PyDict::new(py);
    for m in Metal::PRECIOUS.into_iter().chain(Metal::BASE) {
        d.set_item(m.code(), dict!(py, "name" => m.name(), "unit" => m.unit().code(), "lot_size" => m.lot_size(), "is_precious" => m.is_precious(), "lme_horizon_months" => m.lme_horizon_months()))?;
    }
    Ok(d)
}

/// The `Hub` behind a metal code.
#[pyfunction]
#[pyo3(name = "metal_hub")]
fn py_metal_hub(code: &str) -> PyResult<PyHub> {
    parse_metal(code).map(|m| PyHub(m.hub()))
}

/// Flat lease-rate curve as a `DiscountCurve` (continuous compounding).
#[pyfunction]
#[pyo3(name = "flat_lease_curve")]
#[pyo3(signature = (reference, lease_rate, years = 10))]
fn py_flat_lease_curve(reference: DateLike, lease_rate: f64, years: u32) -> PyResult<PyDiscountCurve> {
    flat_lease_curve(reference.0, lease_rate, years).map(PyDiscountCurve).map_err(err)
}

// ---------------------------------------------------------------------------
// Precious
// ---------------------------------------------------------------------------

/// Spot, USD curve and lease curve: forwards by covered interest parity.
#[pyclass(name = "PreciousMarket", module = "ferrumquant", frozen)]
#[derive(Debug, Clone)]
pub struct PyPreciousMarket(pub PreciousMarket);

#[pymethods]
impl PyPreciousMarket {
    /// `PreciousMarket("XAU", trade_date, spot, usd_curve, lease_curve)`.
    #[new]
    fn new(metal: &str, trade_date: DateLike, spot: f64, usd: &PyDiscountCurve, lease: &PyDiscountCurve) -> PyResult<Self> {
        PreciousMarket::new(parse_metal(metal)?, trade_date.0, spot, usd.0.clone(), lease.0.clone()).map(Self).map_err(err)
    }

    #[getter]
    fn metal(&self) -> &'static str {
        self.0.metal().code()
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

    fn with_spot(&self, spot: f64) -> PyResult<Self> {
        self.0.with_spot(spot).map(Self).map_err(err)
    }

    /// Outright forward for a value date.
    fn forward(&self, value_date: DateLike) -> PyResult<f64> {
        self.0.forward(value_date.0).map_err(err)
    }

    /// Forward for the standard delivery of a contract month.
    fn forward_for_month(&self, month: MonthLike) -> PyResult<f64> {
        self.0.forward_for_month(month.0).map_err(err)
    }

    /// Simple forward rate `usd - lease`.
    fn gofo(&self, value_date: DateLike) -> PyResult<f64> {
        self.0.gofo(value_date.0).map_err(err)
    }

    fn usd_rate(&self, value_date: DateLike) -> PyResult<f64> {
        self.0.usd_rate(value_date.0).map_err(err)
    }

    /// Lease rate that reproduces a quoted forward.
    fn implied_lease_rate(&self, value_date: DateLike, quoted_forward: f64) -> PyResult<f64> {
        self.0.implied_lease_rate(value_date.0, quoted_forward).map_err(err)
    }

    fn __repr__(&self) -> String {
        format!("PreciousMarket({} spot={} as of {})", self.0.metal().code(), self.0.spot(), self.0.trade_date())
    }
}

/// Outright forward purchase.
#[pyclass(name = "PreciousForward", module = "ferrumquant", frozen)]
#[derive(Debug, Clone, Copy)]
pub struct PyPreciousForward(pub PreciousForward);

#[pymethods]
impl PyPreciousForward {
    /// `PreciousForward("XAU", ounces, strike, value_date)`; negative ounces is a sale.
    #[new]
    fn new(metal: &str, ounces: f64, strike: f64, value_date: DateLike) -> PyResult<Self> {
        PreciousForward::new(parse_metal(metal)?, ounces, strike, value_date.0).map(Self).map_err(err)
    }

    fn value<'py>(&self, py: Python<'py>, market: &PyPreciousMarket) -> PyResult<Bound<'py, PyDict>> {
        let v = self.0.value(&market.0).map_err(err)?;
        Ok(dict!(py, "forward" => v.forward, "discount" => v.discount, "pv" => v.pv))
    }

    fn __repr__(&self) -> String {
        format!("PreciousForward({} {} oz @ {} {})", self.0.metal.code(), self.0.ounces, self.0.strike, self.0.value_date)
    }
}

/// European vanilla on the metal forward.
#[pyclass(name = "PreciousVanilla", module = "ferrumquant", frozen)]
#[derive(Debug, Clone, Copy)]
pub struct PyPreciousVanilla(pub PreciousVanilla);

#[pymethods]
impl PyPreciousVanilla {
    /// `PreciousVanilla("call", "XAU", ounces, strike, expiry)`.
    #[new]
    fn new(kind: &str, metal: &str, ounces: f64, strike: f64, expiry: DateLike) -> PyResult<Self> {
        PreciousVanilla::new(parse_kind(kind)?, parse_metal(metal)?, ounces, strike, expiry.0).map(Self).map_err(err)
    }

    #[pyo3(signature = (market, vol, day_count = "ACT/365F"))]
    fn value<'py>(&self, py: Python<'py>, market: &PyPreciousMarket, vol: f64, day_count: &str) -> PyResult<Bound<'py, PyDict>> {
        let v = self.0.value(&market.0, vol, parse_dc(day_count)?).map_err(err)?;
        Ok(dict!(py,
            "delivery" => PyDate(v.delivery), "forward" => v.forward, "time_to_expiry" => v.time_to_expiry, "discount" => v.discount,
            "black" => black_dict(py, &v.black)?, "pv" => v.pv, "spot_delta" => v.spot_delta,
        ))
    }

    fn __repr__(&self) -> String {
        format!("PreciousVanilla({} {} {} oz K={} exp {})", kind_name(self.0.kind), self.0.metal.code(), self.0.ounces, self.0.strike, self.0.expiry)
    }
}

// ---------------------------------------------------------------------------
// LME
// ---------------------------------------------------------------------------

/// LME calendar and prompt-date rules.
#[pyclass(name = "LmeCalendar", module = "ferrumquant", frozen)]
#[derive(Debug, Clone)]
pub struct PyLmeCalendar(pub LmeCalendar);

#[pymethods]
impl PyLmeCalendar {
    #[new]
    fn new() -> Self {
        Self(LmeCalendar::new())
    }

    fn is_business_day(&self, d: DateLike) -> bool {
        self.0.is_business_day(d.0)
    }

    fn cash_prompt(&self, trade_date: DateLike) -> PyResult<PyDate> {
        cash_prompt(trade_date.0, &self.0).map(PyDate).map_err(err)
    }

    fn three_month_prompt(&self, trade_date: DateLike) -> PyResult<PyDate> {
        three_month_prompt(trade_date.0, &self.0).map(PyDate).map_err(err)
    }

    fn six_month_prompt(&self, trade_date: DateLike) -> PyResult<PyDate> {
        six_month_prompt(trade_date.0, &self.0).map(PyDate).map_err(err)
    }

    fn third_wednesday(&self, year: i32, month: u32) -> PyResult<PyDate> {
        third_wednesday(year, month, &self.0).map(PyDate).map_err(err)
    }

    /// Option declaration day for a third-Wednesday prompt.
    fn declaration_day(&self, year: i32, month: u32) -> PyResult<PyDate> {
        declaration_day(year, month, &self.0).map(PyDate).map_err(err)
    }

    /// `"cash" | "daily" | "3m" | "weekly" | "monthly"` or `None` if not a valid prompt.
    fn classify_prompt(&self, metal: &str, trade_date: DateLike, d: DateLike) -> PyResult<Option<&'static str>> {
        Ok(classify_prompt(parse_metal(metal)?, trade_date.0, d.0, &self.0).map_err(err)?.map(prompt_kind_name))
    }

    fn is_valid_prompt(&self, metal: &str, trade_date: DateLike, d: DateLike) -> PyResult<bool> {
        is_valid_prompt(parse_metal(metal)?, trade_date.0, d.0, &self.0).map_err(err)
    }

    /// All valid prompts out to `months`, as `(date, kind)`.
    fn prompts(&self, metal: &str, trade_date: DateLike, months: i32) -> PyResult<Vec<(PyDate, &'static str)>> {
        Ok(prompts(parse_metal(metal)?, trade_date.0, months, &self.0).map_err(err)?.into_iter().map(|(d, k)| (PyDate(d), prompt_kind_name(k))).collect())
    }

    fn __repr__(&self) -> String {
        "LmeCalendar()".into()
    }
}

/// Forward curve on prompt dates.
#[pyclass(name = "LmeCurve", module = "ferrumquant", frozen)]
#[derive(Debug, Clone)]
pub struct PyLmeCurve(pub LmeCurve);

#[pymethods]
impl PyLmeCurve {
    /// `LmeCurve("CA", trade_date, [(prompt_date, price), ...], cal)`.
    #[new]
    fn new(metal: &str, trade_date: DateLike, quotes: Vec<(DateLike, f64)>, cal: &PyLmeCalendar) -> PyResult<Self> {
        LmeCurve::new(parse_metal(metal)?, trade_date.0, quotes.into_iter().map(|(d, p)| (d.0, p)).collect(), &cal.0).map(Self).map_err(err)
    }

    #[getter]
    fn metal(&self) -> &'static str {
        self.0.metal().code()
    }

    #[getter]
    fn trade_date(&self) -> PyDate {
        PyDate(self.0.trade_date())
    }

    #[getter]
    fn cash_date(&self) -> PyDate {
        PyDate(self.0.cash_date())
    }

    #[getter]
    fn three_month_date(&self) -> PyDate {
        PyDate(self.0.three_month_date())
    }

    #[getter]
    fn quotes(&self) -> Vec<(PyDate, f64)> {
        self.0.quotes().iter().map(|(d, p)| (PyDate(*d), *p)).collect()
    }

    fn price(&self, d: DateLike) -> PyResult<f64> {
        self.0.price(d.0).map_err(err)
    }

    fn cash(&self) -> PyResult<f64> {
        self.0.cash().map_err(err)
    }

    fn three_month(&self) -> PyResult<f64> {
        self.0.three_month().map_err(err)
    }

    /// `price(to) - price(from)`.
    fn carry(&self, from: DateLike, to: DateLike) -> PyResult<f64> {
        self.0.carry(from.0, to.0).map_err(err)
    }

    fn cash_to_3m(&self) -> PyResult<f64> {
        self.0.cash_to_3m().map_err(err)
    }

    /// Annualised simple rate implied by the carry.
    fn implied_carry_rate(&self, from: DateLike, to: DateLike) -> PyResult<f64> {
        self.0.implied_carry_rate(from.0, to.0).map_err(err)
    }

    fn is_contango(&self) -> PyResult<bool> {
        self.0.is_contango().map_err(err)
    }

    fn shifted(&self, bump: f64, cal: &PyLmeCalendar) -> PyResult<Self> {
        self.0.shifted(bump, &cal.0).map(Self).map_err(err)
    }

    /// Steepen the curve: bump grows linearly from cash to the last prompt.
    fn carry_bumped(&self, bump: f64, cal: &PyLmeCalendar) -> PyResult<Self> {
        self.0.carry_bumped(bump, &cal.0).map(Self).map_err(err)
    }

    fn __repr__(&self) -> String {
        format!("LmeCurve({} as of {}, {} prompts)", self.0.metal().code(), self.0.trade_date(), self.0.quotes().len())
    }
}

/// Forward to a prompt date.
#[pyclass(name = "LmeForward", module = "ferrumquant", frozen)]
#[derive(Debug, Clone, Copy)]
pub struct PyLmeForward(pub LmeForward);

#[pymethods]
impl PyLmeForward {
    /// `LmeForward("CA", tonnes, price, prompt)`; negative tonnes is a sale.
    #[new]
    fn new(metal: &str, tonnes: f64, price: f64, prompt: DateLike) -> PyResult<Self> {
        LmeForward::new(parse_metal(metal)?, tonnes, price, prompt.0).map(Self).map_err(err)
    }

    fn value<'py>(&self, py: Python<'py>, curve: &PyLmeCurve, discount: &PyDiscountCurve, cal: &PyLmeCalendar) -> PyResult<Bound<'py, PyDict>> {
        let v = self.0.value(&curve.0, &discount.0, &cal.0).map_err(err)?;
        Ok(dict!(py, "forward" => v.forward, "discount" => v.discount, "pv" => v.pv))
    }

    fn __repr__(&self) -> String {
        format!("LmeForward({:?})", self.0)
    }
}

/// Borrow (buy near, sell far) or lend between two prompts.
#[pyclass(name = "LmeCarry", module = "ferrumquant", frozen)]
#[derive(Debug, Clone, Copy)]
pub struct PyLmeCarry(pub LmeCarry);

#[pymethods]
impl PyLmeCarry {
    /// `LmeCarry("AH", tonnes, near, near_price, far, far_price)`; positive tonnes borrows.
    #[new]
    fn new(metal: &str, tonnes: f64, near: DateLike, near_price: f64, far: DateLike, far_price: f64) -> PyResult<Self> {
        LmeCarry::new(parse_metal(metal)?, tonnes, near.0, near_price, far.0, far_price).map(Self).map_err(err)
    }

    #[getter]
    fn traded_carry(&self) -> f64 {
        self.0.traded_carry()
    }

    fn value(&self, curve: &PyLmeCurve, discount: &PyDiscountCurve, cal: &PyLmeCalendar) -> PyResult<f64> {
        self.0.value(&curve.0, &discount.0, &cal.0).map_err(err)
    }

    fn __repr__(&self) -> String {
        format!("LmeCarry({:?})", self.0)
    }
}

/// Option declared on the first Wednesday, delivering on the third Wednesday.
#[pyclass(name = "LmeOption", module = "ferrumquant", frozen)]
#[derive(Debug, Clone, Copy)]
pub struct PyLmeOption(pub LmeOption);

#[pymethods]
impl PyLmeOption {
    /// `LmeOption("call", "CA", tonnes, strike, year, month)`.
    #[new]
    fn new(kind: &str, metal: &str, tonnes: f64, strike: f64, year: i32, month: u32) -> PyResult<Self> {
        LmeOption::new(parse_kind(kind)?, parse_metal(metal)?, tonnes, strike, year, month).map(Self).map_err(err)
    }

    #[pyo3(signature = (curve, vol, discount, cal, day_count = "ACT/365F"))]
    fn value<'py>(
        &self,
        py: Python<'py>,
        curve: &PyLmeCurve,
        vol: f64,
        discount: &PyDiscountCurve,
        cal: &PyLmeCalendar,
        day_count: &str,
    ) -> PyResult<Bound<'py, PyDict>> {
        let v = self.0.value(&curve.0, vol, &discount.0, &cal.0, parse_dc(day_count)?).map_err(err)?;
        Ok(dict!(py,
            "expiry" => PyDate(v.expiry), "prompt" => PyDate(v.prompt), "forward" => v.forward, "time_to_expiry" => v.time_to_expiry,
            "discount" => v.discount, "black" => black_dict(py, &v.black)?, "pv" => v.pv,
        ))
    }

    fn __repr__(&self) -> String {
        format!("LmeOption({} {} {} t K={} {}-{:02})", kind_name(self.0.kind), self.0.metal.code(), self.0.tonnes, self.0.strike, self.0.year, self.0.month)
    }
}

pub(crate) fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyPreciousMarket>()?;
    m.add_class::<PyPreciousForward>()?;
    m.add_class::<PyPreciousVanilla>()?;
    m.add_class::<PyLmeCalendar>()?;
    m.add_class::<PyLmeCurve>()?;
    m.add_class::<PyLmeForward>()?;
    m.add_class::<PyLmeCarry>()?;
    m.add_class::<PyLmeOption>()?;
    m.add_function(wrap_pyfunction!(py_metals, m)?)?;
    m.add_function(wrap_pyfunction!(py_metal_hub, m)?)?;
    m.add_function(wrap_pyfunction!(py_flat_lease_curve, m)?)?;
    Ok(())
}
