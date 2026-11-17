//! Oil and products: NYMEX/ICE dates, average-price swaps and options,
//! crack spreads and calendar spreads.

use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList};

use fq_oil::{
    AveragePriceOption, AverageSwap, CalendarSpreadOption, CalendarSpreadValuation, Crack321, CrackOption, CrackSpread, OilContract, OilMarket, contango_carry,
    expiry, front_month, pricing_days,
};

use crate::commod::{MonthLike, PyClewlowStrickland, PyContractMonth, PyFuturesCurve, PyHub, VolLike, kind_name, parse_dc, parse_kind, parse_method};
use crate::curve::PyDiscountCurve;
use crate::time::{DateLike, PyDate};
use crate::{FerrumError, dict, err};

pub(crate) fn parse_oil(s: &str) -> PyResult<OilContract> {
    OilContract::from_code(s).ok_or_else(|| FerrumError::new_err(format!("unknown oil contract '{s}' (CL, BRN, RB, HO, GAS)")))
}

/// All oil contract codes with their exchange, unit and lot size.
#[pyfunction]
#[pyo3(name = "oil_contracts")]
fn py_oil_contracts(py: Python<'_>) -> PyResult<Bound<'_, PyDict>> {
    let d = PyDict::new(py);
    for c in OilContract::ALL {
        d.set_item(c.code(), dict!(py, "exchange" => format!("{:?}", c.exchange()), "unit" => c.unit().code(), "lot_size" => c.lot_size(), "is_crude" => c.is_crude(), "to_per_barrel" => c.to_per_barrel()))?;
    }
    Ok(d)
}

/// The `Hub` behind an oil contract code.
#[pyfunction]
#[pyo3(name = "oil_hub")]
fn py_oil_hub(code: &str) -> PyResult<PyHub> {
    parse_oil(code).map(|c| PyHub(c.hub()))
}

/// Exchange calendars and expiry rules for the oil complex.
#[pyclass(name = "OilMarket", module = "ferrumquant", frozen)]
#[derive(Debug, Clone)]
pub struct PyOilMarket(pub OilMarket);

#[pymethods]
impl PyOilMarket {
    #[new]
    fn new() -> Self {
        Self(OilMarket::new())
    }

    /// Last trading day of `contract` for delivery `month`.
    fn expiry(&self, contract: &str, month: MonthLike) -> PyResult<PyDate> {
        expiry(parse_oil(contract)?, month.0, &self.0).map(PyDate).map_err(err)
    }

    /// Settlement days used for a calendar-month average.
    fn pricing_days(&self, contract: &str, month: MonthLike) -> PyResult<Vec<PyDate>> {
        pricing_days(parse_oil(contract)?, month.0, &self.0).map(|v| v.into_iter().map(PyDate).collect()).map_err(err)
    }

    /// Front contract on a date.
    fn front_month(&self, contract: &str, d: DateLike) -> PyResult<PyContractMonth> {
        front_month(parse_oil(contract)?, d.0, &self.0).map(PyContractMonth).map_err(err)
    }

    fn __repr__(&self) -> String {
        "OilMarket()".into()
    }
}

/// Calendar-month-average swap.
#[pyclass(name = "AverageSwap", module = "ferrumquant", frozen)]
#[derive(Debug, Clone)]
pub struct PyAverageSwap(pub AverageSwap);

#[pymethods]
impl PyAverageSwap {
    /// `AverageSwap("CL", start, end, quantity, fixed_price, pay_fixed=True)`.
    #[new]
    #[pyo3(signature = (contract, start, end, quantity, fixed_price, pay_fixed = true))]
    fn new(contract: &str, start: MonthLike, end: MonthLike, quantity: f64, fixed_price: f64, pay_fixed: bool) -> PyResult<Self> {
        AverageSwap::new(parse_oil(contract)?, start.0, end.0, quantity, fixed_price, pay_fixed).map(Self).map_err(err)
    }

    fn months(&self) -> Vec<PyContractMonth> {
        self.0.months().into_iter().map(PyContractMonth).collect()
    }

    fn value<'py>(&self, py: Python<'py>, curve: &PyFuturesCurve, discount: &PyDiscountCurve, market: &PyOilMarket) -> PyResult<Bound<'py, PyDict>> {
        let v = self.0.value(&curve.0, &discount.0, &market.0).map_err(err)?;
        let months = PyList::empty(py);
        for m in &v.months {
            let roll: Vec<(PyContractMonth, usize)> = m.roll.iter().map(|(c, n)| (PyContractMonth(*c), *n)).collect();
            months.append(dict!(py,
                "month" => PyContractMonth(m.month), "pricing_days" => m.pricing_days, "roll" => roll,
                "average_forward" => m.average_forward, "discount" => m.discount, "pv" => m.pv,
            ))?;
        }
        Ok(dict!(py, "months" => months, "pv" => v.pv, "par_fixed" => v.par_fixed))
    }

    fn __repr__(&self) -> String {
        format!("AverageSwap({:?})", self.0)
    }
}

/// Asian option on the calendar-month average settlement.
#[pyclass(name = "AveragePriceOption", module = "ferrumquant", frozen)]
#[derive(Debug, Clone)]
pub struct PyAveragePriceOption(pub AveragePriceOption);

#[pymethods]
impl PyAveragePriceOption {
    /// `AveragePriceOption("call", "BRN", month, strike, quantity, observed=[(date, price), ...], roll_correlation=1.0)`.
    #[new]
    #[pyo3(signature = (kind, contract, month, strike, quantity, observed = Vec::new(), roll_correlation = 1.0))]
    fn new(kind: &str, contract: &str, month: MonthLike, strike: f64, quantity: f64, observed: Vec<(DateLike, f64)>, roll_correlation: f64) -> PyResult<Self> {
        let o = AveragePriceOption::new(parse_kind(kind)?, parse_oil(contract)?, month.0, strike, quantity)
            .map_err(err)?
            .with_observed(observed.into_iter().map(|(d, p)| (d.0, p)).collect())
            .with_roll_correlation(roll_correlation)
            .map_err(err)?;
        Ok(Self(o))
    }

    /// Remaining and observed fixings as `(date, contract_month, price_or_None)`.
    fn fixings(&self, market: &PyOilMarket) -> PyResult<Vec<(PyDate, PyContractMonth, Option<f64>)>> {
        Ok(self.0.fixings(&market.0).map_err(err)?.into_iter().map(|f| (PyDate(f.date), PyContractMonth(f.contract), f.observed)).collect())
    }

    #[pyo3(signature = (curve, vol, discount, market, day_count = "ACT/365F"))]
    fn value<'py>(
        &self,
        py: Python<'py>,
        curve: &PyFuturesCurve,
        vol: VolLike,
        discount: &PyDiscountCurve,
        market: &PyOilMarket,
        day_count: &str,
    ) -> PyResult<Bound<'py, PyDict>> {
        let v = self.0.value(&curve.0, &vol.seasonal(curve.0.first_month())?, &discount.0, &market.0, parse_dc(day_count)?).map_err(err)?;
        let a = &v.inner;
        Ok(dict!(py,
            "fixings" => v.fixings, "observed" => v.observed, "forward_average" => a.forward_average, "fixed_contribution" => a.fixed_contribution,
            "effective_strike" => a.effective_strike, "effective_vol" => a.effective_vol, "time_to_last_fixing" => a.time_to_last_fixing,
            "discount" => a.discount, "forward_price" => a.forward_price, "pv" => a.pv,
        ))
    }

    fn __repr__(&self) -> String {
        format!("AveragePriceOption({} {} {} K={})", kind_name(self.0.kind), self.0.contract.code(), self.0.month, self.0.strike)
    }
}

/// Product-minus-crude option, $/bbl.
#[pyclass(name = "CrackOption", module = "ferrumquant", frozen)]
#[derive(Debug, Clone)]
pub struct PyCrackOption(pub CrackOption);

#[pymethods]
impl PyCrackOption {
    /// `CrackOption("call", "RB", "CL", month, strike, barrels, method="bs")`.
    #[new]
    #[allow(clippy::too_many_arguments)]
    #[pyo3(signature = (kind, product, crude, month, strike, barrels, method = "bs"))]
    fn new(kind: &str, product: &str, crude: &str, month: MonthLike, strike: f64, barrels: f64, method: &str) -> PyResult<Self> {
        let method = parse_method(method)?;
        let spread = CrackSpread::new(parse_oil(product)?, parse_oil(crude)?, month.0).map_err(err)?;
        CrackOption::new(parse_kind(kind)?, spread, strike, barrels).map(|o| Self(o.with_method(method))).map_err(err)
    }

    /// Forward crack in $/bbl.
    fn forward(&self, product: &PyFuturesCurve, crude: &PyFuturesCurve) -> PyResult<f64> {
        self.0.spread.forward(&product.0, &crude.0).map_err(err)
    }

    #[allow(clippy::too_many_arguments)]
    #[pyo3(signature = (product, crude, product_vol, crude_vol, rho, discount, market, day_count = "ACT/365F"))]
    fn value<'py>(
        &self,
        py: Python<'py>,
        product: &PyFuturesCurve,
        crude: &PyFuturesCurve,
        product_vol: VolLike,
        crude_vol: VolLike,
        rho: f64,
        discount: &PyDiscountCurve,
        market: &PyOilMarket,
        day_count: &str,
    ) -> PyResult<Bound<'py, PyDict>> {
        let anchor = crude.0.first_month();
        let v = self
            .0
            .value(&product.0, &crude.0, &product_vol.seasonal(anchor)?, &crude_vol.seasonal(anchor)?, rho, &discount.0, &market.0, parse_dc(day_count)?)
            .map_err(err)?;
        Ok(dict!(py,
            "product_per_bbl" => v.product_per_bbl, "crude" => v.crude, "forward_crack" => v.forward_crack, "time_to_expiry" => v.time_to_expiry,
            "discount" => v.discount, "forward_price" => v.forward_price, "pv" => v.pv,
        ))
    }

    fn __repr__(&self) -> String {
        format!("CrackOption({:?})", self.0)
    }
}

/// 3-2-1 refinery crack: 2 gasoline + 1 distillate against 3 crude.
#[pyclass(name = "Crack321", module = "ferrumquant", frozen)]
#[derive(Debug, Clone)]
pub struct PyCrack321(pub Crack321);

#[pymethods]
impl PyCrack321 {
    /// `Crack321(month, gasoline="RB", distillate="HO", crude="CL")`.
    #[new]
    #[pyo3(signature = (month, gasoline = "RB", distillate = "HO", crude = "CL"))]
    fn new(month: MonthLike, gasoline: &str, distillate: &str, crude: &str) -> PyResult<Self> {
        Crack321::new(parse_oil(gasoline)?, parse_oil(distillate)?, parse_oil(crude)?, month.0).map(Self).map_err(err)
    }

    /// Forward crack and matched basket; pass `kind`/`strike`/`barrels` for an option on it.
    #[allow(clippy::too_many_arguments)]
    #[pyo3(signature = (gasoline, distillate, crude, vols, rho_products, rho_crude, discount, market, kind = None, strike = None, barrels = None, day_count = "ACT/365F"))]
    fn value<'py>(
        &self,
        py: Python<'py>,
        gasoline: &PyFuturesCurve,
        distillate: &PyFuturesCurve,
        crude: &PyFuturesCurve,
        vols: (VolLike, VolLike, VolLike),
        rho_products: f64,
        rho_crude: f64,
        discount: &PyDiscountCurve,
        market: &PyOilMarket,
        kind: Option<&str>,
        strike: Option<f64>,
        barrels: Option<f64>,
        day_count: &str,
    ) -> PyResult<Bound<'py, PyDict>> {
        let anchor = crude.0.first_month();
        let (vg, vd, vc) = (vols.0.seasonal(anchor)?, vols.1.seasonal(anchor)?, vols.2.seasonal(anchor)?);
        let option = match (kind, strike, barrels) {
            (Some(k), Some(s), Some(b)) => Some((parse_kind(k)?, s, b)),
            (None, None, None) => None,
            _ => return Err(FerrumError::new_err("give kind, strike and barrels together or none of them")),
        };
        let v = self
            .0
            .value(&gasoline.0, &distillate.0, &crude.0, (&vg, &vd, &vc), rho_products, rho_crude, option, &discount.0, &market.0, parse_dc(day_count)?)
            .map_err(err)?;
        Ok(dict!(py,
            "forward_crack" => v.forward_crack, "basket_forward" => v.basket_forward, "basket_vol" => v.basket_vol, "time_to_expiry" => v.time_to_expiry,
            "discount" => v.discount, "option_forward_price" => v.option_forward_price, "option_pv" => v.option_pv,
        ))
    }

    fn __repr__(&self) -> String {
        format!("Crack321({:?})", self.0)
    }
}

fn cso_dict<'py>(py: Python<'py>, v: &CalendarSpreadValuation) -> PyResult<Bound<'py, PyDict>> {
    Ok(dict!(py,
        "near_forward" => v.near_forward, "far_forward" => v.far_forward, "forward_spread" => v.forward_spread, "correlation" => v.correlation,
        "time_to_expiry" => v.time_to_expiry, "discount" => v.discount, "forward_price" => v.forward_price, "pv" => v.pv,
    ))
}

/// Option on `near - far` for one contract.
#[pyclass(name = "OilCalendarSpreadOption", module = "ferrumquant", frozen)]
#[derive(Debug, Clone)]
pub struct PyOilCalendarSpreadOption(pub CalendarSpreadOption);

#[pymethods]
impl PyOilCalendarSpreadOption {
    /// `OilCalendarSpreadOption("call", "CL", near, far, strike, quantity, method="bs")`.
    #[new]
    #[allow(clippy::too_many_arguments)]
    #[pyo3(signature = (kind, contract, near, far, strike, quantity, method = "bs"))]
    fn new(kind: &str, contract: &str, near: MonthLike, far: MonthLike, strike: f64, quantity: f64, method: &str) -> PyResult<Self> {
        let method = parse_method(method)?;
        CalendarSpreadOption::new(parse_kind(kind)?, parse_oil(contract)?, near.0, far.0, strike, quantity).map(|o| Self(o.with_method(method))).map_err(err)
    }

    /// Spread-option price with an explicit inter-month correlation.
    #[allow(clippy::too_many_arguments)]
    #[pyo3(signature = (curve, vol, rho, discount, market, day_count = "ACT/365F"))]
    fn value<'py>(
        &self,
        py: Python<'py>,
        curve: &PyFuturesCurve,
        vol: VolLike,
        rho: f64,
        discount: &PyDiscountCurve,
        market: &PyOilMarket,
        day_count: &str,
    ) -> PyResult<Bound<'py, PyDict>> {
        let v =
            self.0.value_with_correlation(&curve.0, &vol.seasonal(curve.0.first_month())?, rho, &discount.0, &market.0, parse_dc(day_count)?).map_err(err)?;
        cso_dict(py, &v)
    }

    /// Vols and correlation implied by a Clewlow-Strickland model.
    #[pyo3(signature = (curve, model, discount, market, day_count = "ACT/365F"))]
    fn value_with_model<'py>(
        &self,
        py: Python<'py>,
        curve: &PyFuturesCurve,
        model: &PyClewlowStrickland,
        discount: &PyDiscountCurve,
        market: &PyOilMarket,
        day_count: &str,
    ) -> PyResult<Bound<'py, PyDict>> {
        let v = self.0.value_with_model(&curve.0, &model.0, &discount.0, &market.0, parse_dc(day_count)?).map_err(err)?;
        cso_dict(py, &v)
    }

    fn __repr__(&self) -> String {
        format!("OilCalendarSpreadOption({:?})", self.0)
    }
}

/// Cash-and-carry economics of a contango spread.
#[pyfunction]
#[pyo3(name = "contango_carry")]
#[allow(clippy::too_many_arguments)]
fn py_contango_carry<'py>(
    py: Python<'py>,
    contract: &str,
    curve: &PyFuturesCurve,
    near: MonthLike,
    far: MonthLike,
    storage_per_unit_month: f64,
    discount: &PyDiscountCurve,
    market: &PyOilMarket,
) -> PyResult<Bound<'py, PyDict>> {
    let r = contango_carry(parse_oil(contract)?, &curve.0, near.0, far.0, storage_per_unit_month, &discount.0, &market.0).map_err(err)?;
    Ok(dict!(py,
        "near_forward" => r.near_forward, "far_forward" => r.far_forward, "spread" => r.spread, "months" => r.months, "storage_cost" => r.storage_cost,
        "financing_cost" => r.financing_cost, "net_carry" => r.net_carry, "breakeven_spread" => r.breakeven_spread,
    ))
}

pub(crate) fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyOilMarket>()?;
    m.add_class::<PyAverageSwap>()?;
    m.add_class::<PyAveragePriceOption>()?;
    m.add_class::<PyCrackOption>()?;
    m.add_class::<PyCrack321>()?;
    m.add_class::<PyOilCalendarSpreadOption>()?;
    m.add_function(wrap_pyfunction!(py_oil_contracts, m)?)?;
    m.add_function(wrap_pyfunction!(py_oil_hub, m)?)?;
    m.add_function(wrap_pyfunction!(py_contango_carry, m)?)?;
    Ok(())
}
