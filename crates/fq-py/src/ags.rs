//! Agriculturals: CBOT/ICE/CME contracts and expiries, options, the
//! soybean crush and calendar spreads against full carry.

use pyo3::prelude::*;
use pyo3::types::PyDict;

use fq_ags::{AgCalendar, AgCalendarSpread, AgContract, AgOption, Crush, expiry, full_carry, option_expiry};
use fq_core::math::BlackResult;

use crate::commod::{MonthLike, PyContractMonth, PyFuturesCurve, PyHub, VolLike, kind_name, parse_dc, parse_kind, parse_method};
use crate::curve::PyDiscountCurve;
use crate::time::{DateLike, PyDate};
use crate::{FerrumError, dict, err};

pub(crate) fn parse_ag(s: &str) -> PyResult<AgContract> {
    AgContract::from_code(s).ok_or_else(|| FerrumError::new_err(format!("unknown ag contract '{s}'; see ag_contracts()")))
}

fn black_dict<'py>(py: Python<'py>, b: &BlackResult) -> PyResult<Bound<'py, PyDict>> {
    Ok(dict!(py, "price" => b.price, "delta" => b.delta, "gamma" => b.gamma, "vega" => b.vega))
}

/// All ag contract codes with exchange, sector, units, lot size and listed months.
#[pyfunction]
#[pyo3(name = "ag_contracts")]
fn py_ag_contracts(py: Python<'_>) -> PyResult<Bound<'_, PyDict>> {
    let d = PyDict::new(py);
    for c in AgContract::ALL {
        d.set_item(
            c.code(),
            dict!(py,
                "exchange" => format!("{:?}", c.exchange()), "sector" => format!("{:?}", c.sector()), "quote_unit" => format!("{:?}", c.quote_unit()),
                "unit" => c.unit().code(), "lot_size" => c.lot_size(), "is_storable" => c.is_storable(), "listed_months" => c.listed_months().to_vec(),
            ),
        )?;
    }
    Ok(d)
}

/// The `Hub` behind an ag contract code.
#[pyfunction]
#[pyo3(name = "ag_hub")]
fn py_ag_hub(code: &str) -> PyResult<PyHub> {
    parse_ag(code).map(|c| PyHub(c.hub()))
}

/// Dollar value of one lot at a quoted price.
#[pyfunction]
#[pyo3(name = "ag_lot_value")]
fn py_ag_lot_value(contract: &str, quoted_price: f64) -> PyResult<f64> {
    Ok(parse_ag(contract)?.lot_value(quoted_price))
}

/// Next listed delivery month on or after `month`.
#[pyfunction]
#[pyo3(name = "ag_next_listed")]
fn py_ag_next_listed(contract: &str, month: MonthLike) -> PyResult<PyContractMonth> {
    Ok(PyContractMonth(parse_ag(contract)?.next_listed(month.0)))
}

/// US exchange calendar with Good Friday, plus the futures and option expiry rules.
#[pyclass(name = "AgCalendar", module = "ferrumquant", frozen)]
#[derive(Debug, Clone)]
pub struct PyAgCalendar(pub AgCalendar);

#[pymethods]
impl PyAgCalendar {
    #[new]
    fn new() -> Self {
        Self(AgCalendar::new())
    }

    fn is_business_day(&self, d: DateLike) -> bool {
        self.0.is_business_day(d.0)
    }

    #[staticmethod]
    fn is_good_friday(d: DateLike) -> bool {
        AgCalendar::is_good_friday(d.0)
    }

    fn advance(&self, d: DateLike, n: i32) -> PyResult<PyDate> {
        self.0.advance(d.0, n).map(PyDate).map_err(err)
    }

    fn nth_business_day(&self, month: MonthLike, n: u32) -> PyResult<PyDate> {
        self.0.nth_business_day(month.0, n).map(PyDate).map_err(err)
    }

    fn last_business_day(&self, month: MonthLike) -> PyResult<PyDate> {
        self.0.last_business_day(month.0).map(PyDate).map_err(err)
    }

    /// Futures last trading day.
    fn expiry(&self, contract: &str, month: MonthLike) -> PyResult<PyDate> {
        expiry(parse_ag(contract)?, month.0, &self.0).map(PyDate).map_err(err)
    }

    /// Standard option expiry for the futures month.
    fn option_expiry(&self, contract: &str, month: MonthLike) -> PyResult<PyDate> {
        option_expiry(parse_ag(contract)?, month.0, &self.0).map(PyDate).map_err(err)
    }

    fn __repr__(&self) -> String {
        "AgCalendar()".into()
    }
}

/// Black-76 option on an ag future, strike in quotation units.
#[pyclass(name = "AgOption", module = "ferrumquant", frozen)]
#[derive(Debug, Clone, Copy)]
pub struct PyAgOption(pub AgOption);

#[pymethods]
impl PyAgOption {
    /// `AgOption("call", "ZC", month, strike, lots)`.
    #[new]
    fn new(kind: &str, contract: &str, month: MonthLike, strike: f64, lots: f64) -> PyResult<Self> {
        AgOption::new(parse_kind(kind)?, parse_ag(contract)?, month.0, strike, lots).map(Self).map_err(err)
    }

    #[pyo3(signature = (curve, vol, discount, cal, day_count = "ACT/365F"))]
    fn value<'py>(
        &self,
        py: Python<'py>,
        curve: &PyFuturesCurve,
        vol: VolLike,
        discount: &PyDiscountCurve,
        cal: &PyAgCalendar,
        day_count: &str,
    ) -> PyResult<Bound<'py, PyDict>> {
        let v = self.0.value(&curve.0, &vol.seasonal(curve.0.first_month())?, &discount.0, &cal.0, parse_dc(day_count)?).map_err(err)?;
        Ok(dict!(py,
            "expiry" => PyDate(v.expiry), "forward" => v.forward, "vol" => v.vol, "time_to_expiry" => v.time_to_expiry, "discount" => v.discount,
            "black" => black_dict(py, &v.black)?, "pv" => v.pv, "delta_dollars" => v.delta_dollars,
        ))
    }

    /// Implied vol from an undiscounted premium per quotation unit.
    #[pyo3(signature = (curve, premium, cal, day_count = "ACT/365F"))]
    fn implied_vol(&self, curve: &PyFuturesCurve, premium: f64, cal: &PyAgCalendar, day_count: &str) -> PyResult<f64> {
        self.0.implied_vol(&curve.0, premium, &cal.0, parse_dc(day_count)?).map_err(err)
    }

    fn __repr__(&self) -> String {
        format!("AgOption({} {} {} K={} x{})", kind_name(self.0.kind), self.0.contract.code(), self.0.month, self.0.strike, self.0.lots)
    }
}

/// Soybean crush: beans into meal and oil.
#[pyclass(name = "Crush", module = "ferrumquant", frozen)]
#[derive(Debug, Clone, Copy)]
pub struct PyCrush(pub Crush);

#[pymethods]
impl PyCrush {
    /// `Crush(month, bushels)`.
    #[new]
    fn new(month: MonthLike, bushels: f64) -> PyResult<Self> {
        Crush::new(month.0, bushels).map(Self).map_err(err)
    }

    /// Gross processing margin off the ZS, ZM and ZL curves.
    fn value<'py>(&self, py: Python<'py>, beans: &PyFuturesCurve, meal: &PyFuturesCurve, oil: &PyFuturesCurve) -> PyResult<Bound<'py, PyDict>> {
        let v = self.0.value(&beans.0, &meal.0, &oil.0).map_err(err)?;
        Ok(dict!(py,
            "beans" => v.beans, "meal_value" => v.meal_value, "oil_value" => v.oil_value, "crush_margin" => v.crush_margin,
            "gross_margin" => v.gross_margin, "oil_share" => v.oil_share,
        ))
    }

    /// Option on the crush margin ($/bu) by basket moment matching.
    #[allow(clippy::too_many_arguments)]
    #[pyo3(signature = (kind, strike, beans, meal, oil, vols, rho_products, rho_beans, discount, cal, day_count = "ACT/365F"))]
    fn option<'py>(
        &self,
        py: Python<'py>,
        kind: &str,
        strike: f64,
        beans: &PyFuturesCurve,
        meal: &PyFuturesCurve,
        oil: &PyFuturesCurve,
        vols: (VolLike, VolLike, VolLike),
        rho_products: f64,
        rho_beans: f64,
        discount: &PyDiscountCurve,
        cal: &PyAgCalendar,
        day_count: &str,
    ) -> PyResult<Bound<'py, PyDict>> {
        let anchor = beans.0.first_month();
        let (vb, vm, vo) = (vols.0.seasonal(anchor)?, vols.1.seasonal(anchor)?, vols.2.seasonal(anchor)?);
        let v = self
            .0
            .option(parse_kind(kind)?, strike, &beans.0, &meal.0, &oil.0, (&vb, &vm, &vo), rho_products, rho_beans, &discount.0, &cal.0, parse_dc(day_count)?)
            .map_err(err)?;
        let s = &v.spot;
        Ok(dict!(py,
            "spot" => dict!(py, "beans" => s.beans, "meal_value" => s.meal_value, "oil_value" => s.oil_value, "crush_margin" => s.crush_margin, "gross_margin" => s.gross_margin, "oil_share" => s.oil_share),
            "basket_forward" => v.basket_forward, "basket_vol" => v.basket_vol, "time_to_expiry" => v.time_to_expiry, "discount" => v.discount,
            "forward_price" => v.forward_price, "pv" => v.pv,
        ))
    }

    fn __repr__(&self) -> String {
        format!("Crush({} {} bu)", self.0.month, self.0.bushels)
    }
}

/// Spread against the full-carry benchmark.
#[pyfunction]
#[pyo3(name = "full_carry")]
#[allow(clippy::too_many_arguments)]
fn py_full_carry<'py>(
    py: Python<'py>,
    contract: &str,
    curve: &PyFuturesCurve,
    near: MonthLike,
    far: MonthLike,
    storage_per_unit_day: f64,
    discount: &PyDiscountCurve,
    cal: &PyAgCalendar,
) -> PyResult<Bound<'py, PyDict>> {
    let r = full_carry(parse_ag(contract)?, &curve.0, near.0, far.0, storage_per_unit_day, &discount.0, &cal.0).map_err(err)?;
    Ok(dict!(py,
        "near" => r.near, "far" => r.far, "spread" => r.spread, "days" => r.days, "storage" => r.storage, "interest" => r.interest,
        "full_carry" => r.full_carry, "percent_of_full_carry" => r.percent_of_full_carry,
    ))
}

/// Option on `near - far` for one ag contract.
#[pyclass(name = "AgCalendarSpread", module = "ferrumquant", frozen)]
#[derive(Debug, Clone, Copy)]
pub struct PyAgCalendarSpread(pub AgCalendarSpread);

#[pymethods]
impl PyAgCalendarSpread {
    /// `AgCalendarSpread("call", "ZC", near, far, strike, lots, method="bs")`.
    #[new]
    #[allow(clippy::too_many_arguments)]
    #[pyo3(signature = (kind, contract, near, far, strike, lots, method = "bs"))]
    fn new(kind: &str, contract: &str, near: MonthLike, far: MonthLike, strike: f64, lots: f64, method: &str) -> PyResult<Self> {
        let method = parse_method(method)?;
        AgCalendarSpread::new(parse_kind(kind)?, parse_ag(contract)?, near.0, far.0, strike, lots).map(|o| Self(o.with_method(method))).map_err(err)
    }

    #[allow(clippy::too_many_arguments)]
    #[pyo3(signature = (curve, vol, rho, discount, cal, day_count = "ACT/365F"))]
    fn value<'py>(
        &self,
        py: Python<'py>,
        curve: &PyFuturesCurve,
        vol: VolLike,
        rho: f64,
        discount: &PyDiscountCurve,
        cal: &PyAgCalendar,
        day_count: &str,
    ) -> PyResult<Bound<'py, PyDict>> {
        let v = self.0.value(&curve.0, &vol.seasonal(curve.0.first_month())?, rho, &discount.0, &cal.0, parse_dc(day_count)?).map_err(err)?;
        Ok(dict!(py,
            "near_forward" => v.near_forward, "far_forward" => v.far_forward, "forward_spread" => v.forward_spread, "time_to_expiry" => v.time_to_expiry,
            "discount" => v.discount, "forward_price" => v.forward_price, "pv" => v.pv,
        ))
    }

    fn __repr__(&self) -> String {
        format!("AgCalendarSpread({:?})", self.0)
    }
}

pub(crate) fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyAgCalendar>()?;
    m.add_class::<PyAgOption>()?;
    m.add_class::<PyCrush>()?;
    m.add_class::<PyAgCalendarSpread>()?;
    m.add_function(wrap_pyfunction!(py_ag_contracts, m)?)?;
    m.add_function(wrap_pyfunction!(py_ag_hub, m)?)?;
    m.add_function(wrap_pyfunction!(py_ag_lot_value, m)?)?;
    m.add_function(wrap_pyfunction!(py_ag_next_listed, m)?)?;
    m.add_function(wrap_pyfunction!(py_full_carry, m)?)?;
    Ok(())
}
