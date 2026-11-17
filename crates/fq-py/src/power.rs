//! Power: ISO calendars, block curves, swaps, heat-rate options, tolling,
//! hourly shaping and unit commitment.

use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList};

use fq_commod_core::DeliveryPeriod;
use fq_power::{
    Block, BlockSwap, HeatRateOption, HeatRateValuation, HourlyProfile, Iso, PowerCurve, PowerHub, ThermalUnit, TollingAgreement, block_hours, dispatch,
    hours_in_day, is_nerc_holiday, is_peak_day, is_peak_hour, iso::nerc_holidays, shape_month,
};

use crate::commod::{MonthLike, PyContractMonth, PyFuturesCurve, PyHub, VolLike, parse_dc, parse_kind, parse_method};
use crate::curve::PyDiscountCurve;
use crate::time::{DateLike, PyDate};
use crate::{FerrumError, dict, err};

pub(crate) fn parse_power_hub(s: &str) -> PyResult<PowerHub> {
    PowerHub::from_code(s).ok_or_else(|| FerrumError::new_err(format!("unknown power hub '{s}'; see power_hubs()")))
}

pub(crate) fn parse_iso(s: &str) -> PyResult<Iso> {
    Iso::ALL.into_iter().find(|i| i.code().eq_ignore_ascii_case(s)).ok_or_else(|| FerrumError::new_err(format!("unknown ISO '{s}'")))
}

pub(crate) fn parse_block(s: &str) -> PyResult<Block> {
    Ok(match s.to_ascii_lowercase().as_str() {
        "pk" | "peak" | "5x16" | "6x16" | "onpeak" | "on_peak" => Block::Peak,
        "op" | "offpeak" | "off_peak" => Block::OffPeak,
        "atc" | "7x24" | "flat" | "baseload" => Block::Atc,
        "2x16" | "wrap" => Block::Wrap2x16,
        "7x8" | "nights" => Block::Nights7x8,
        _ => return Err(FerrumError::new_err(format!("unknown block '{s}' (PK, OP, ATC, 2x16, 7x8)"))),
    })
}

/// All power hub codes with their ISO.
#[pyfunction]
#[pyo3(name = "power_hubs")]
fn py_power_hubs(py: Python<'_>) -> PyResult<Bound<'_, PyDict>> {
    let d = PyDict::new(py);
    for h in PowerHub::ALL {
        d.set_item(h.code(), h.iso().code())?;
    }
    Ok(d)
}

/// The `Hub` behind a power hub code.
#[pyfunction]
#[pyo3(name = "power_hub")]
fn py_power_hub(code: &str) -> PyResult<PyHub> {
    parse_power_hub(code).map(|h| PyHub(h.hub()))
}

#[pyfunction]
#[pyo3(name = "nerc_holidays")]
fn py_nerc_holidays(year: i32) -> Vec<PyDate> {
    nerc_holidays(year).into_iter().map(PyDate).collect()
}

#[pyfunction]
#[pyo3(name = "is_nerc_holiday")]
fn py_is_nerc_holiday(d: DateLike) -> bool {
    is_nerc_holiday(d.0)
}

#[pyfunction]
#[pyo3(name = "is_peak_day")]
fn py_is_peak_day(iso: &str, d: DateLike) -> PyResult<bool> {
    Ok(is_peak_day(parse_iso(iso)?, d.0))
}

#[pyfunction]
#[pyo3(name = "is_peak_hour")]
fn py_is_peak_hour(hour_ending: u32) -> bool {
    is_peak_hour(hour_ending)
}

/// 23, 24 or 25 on DST transition days.
#[pyfunction]
#[pyo3(name = "hours_in_day")]
fn py_hours_in_day(d: DateLike) -> u32 {
    hours_in_day(d.0)
}

/// Hours in `block` between two dates inclusive for an ISO.
#[pyfunction]
#[pyo3(name = "block_hours")]
fn py_block_hours(iso: &str, block: &str, start: DateLike, end: DateLike) -> PyResult<u32> {
    let period = DeliveryPeriod::new(start.0, end.0).map_err(err)?;
    Ok(block_hours(parse_iso(iso)?, parse_block(block)?, period))
}

/// Peak and off-peak monthly curves for one hub.
#[pyclass(name = "PowerCurve", module = "ferrumquant", frozen)]
#[derive(Debug, Clone)]
pub struct PyPowerCurve(pub PowerCurve);

#[pymethods]
impl PyPowerCurve {
    /// `PowerCurve("PJMW", peak_curve, offpeak_curve)`.
    #[new]
    fn new(hub: &str, peak: &PyFuturesCurve, offpeak: &PyFuturesCurve) -> PyResult<Self> {
        PowerCurve::new(parse_power_hub(hub)?, peak.0.clone(), offpeak.0.clone()).map(Self).map_err(err)
    }

    #[getter]
    fn hub(&self) -> &'static str {
        self.0.hub().code()
    }

    #[getter]
    fn iso(&self) -> &'static str {
        self.0.hub().iso().code()
    }

    #[getter]
    fn reference_date(&self) -> PyDate {
        PyDate(self.0.reference_date())
    }

    #[getter]
    fn peak(&self) -> PyFuturesCurve {
        PyFuturesCurve(self.0.peak().clone())
    }

    #[getter]
    fn offpeak(&self) -> PyFuturesCurve {
        PyFuturesCurve(self.0.offpeak().clone())
    }

    fn hours(&self, month: MonthLike, block: &str) -> PyResult<u32> {
        self.0.hours(month.0, parse_block(block)?).map_err(err)
    }

    /// `{"month", "block", "hours", "price"}` for any block (ATC and wraps are hour-weighted).
    fn block_price<'py>(&self, py: Python<'py>, month: MonthLike, block: &str) -> PyResult<Bound<'py, PyDict>> {
        let b = self.0.block_price(month.0, parse_block(block)?).map_err(err)?;
        Ok(dict!(py, "month" => PyContractMonth(b.month), "block" => b.block.code(), "hours" => b.hours, "price" => b.price))
    }

    /// `(hours, price)` over a date range.
    fn period_price(&self, start: DateLike, end: DateLike, block: &str) -> PyResult<(u32, f64)> {
        let period = DeliveryPeriod::new(start.0, end.0).map_err(err)?;
        self.0.period_price(period, parse_block(block)?).map_err(err)
    }

    fn shifted(&self, bump: f64) -> PyResult<Self> {
        self.0.shifted(bump).map(Self).map_err(err)
    }

    fn shifted_block(&self, block: &str, bump: f64) -> PyResult<Self> {
        self.0.shifted_block(parse_block(block)?, bump).map(Self).map_err(err)
    }

    /// Hourly prices for one month from a 24-hour profile; returns a list of dicts.
    #[pyo3(signature = (month, peak_profile = None, nonpeak_profile = None))]
    fn shape<'py>(
        &self,
        py: Python<'py>,
        month: MonthLike,
        peak_profile: Option<[f64; 24]>,
        nonpeak_profile: Option<[f64; 24]>,
    ) -> PyResult<Bound<'py, PyList>> {
        let profile = match (peak_profile, nonpeak_profile) {
            (Some(p), Some(n)) => HourlyProfile::new(p, n).map_err(err)?,
            (None, None) => HourlyProfile::typical(),
            _ => return Err(FerrumError::new_err("give both peak_profile and nonpeak_profile or neither")),
        };
        let hp = shape_month(&self.0, month.0, &profile).map_err(err)?;
        let out = PyList::empty(py);
        for h in &hp.hours {
            out.append(dict!(py, "date" => PyDate(h.date), "hour_ending" => h.hour_ending, "peak" => h.peak, "price" => h.price))?;
        }
        Ok(out)
    }

    fn __repr__(&self) -> String {
        format!("PowerCurve({} as of {})", self.0.hub().code(), self.0.reference_date())
    }
}

/// Fixed-for-floating block swap.
#[pyclass(name = "BlockSwap", module = "ferrumquant", frozen)]
#[derive(Debug, Clone)]
pub struct PyBlockSwap(pub BlockSwap);

#[pymethods]
impl PyBlockSwap {
    /// `BlockSwap(hub, block, start, end, mw, fixed_price, pay_fixed=True)`.
    #[new]
    #[pyo3(signature = (hub, block, start, end, mw, fixed_price, pay_fixed = true))]
    fn new(hub: &str, block: &str, start: MonthLike, end: MonthLike, mw: f64, fixed_price: f64, pay_fixed: bool) -> PyResult<Self> {
        BlockSwap::new(parse_power_hub(hub)?, parse_block(block)?, start.0, end.0, mw, fixed_price, pay_fixed).map(Self).map_err(err)
    }

    fn months(&self) -> Vec<PyContractMonth> {
        self.0.months().into_iter().map(PyContractMonth).collect()
    }

    fn value<'py>(&self, py: Python<'py>, curve: &PyPowerCurve, discount: &PyDiscountCurve) -> PyResult<Bound<'py, PyDict>> {
        let v = self.0.value(&curve.0, &discount.0).map_err(err)?;
        let months = PyList::empty(py);
        for m in &v.months {
            months.append(dict!(py,
                "month" => PyContractMonth(m.month), "hours" => m.hours, "mwh" => m.mwh, "forward" => m.forward, "discount" => m.discount, "pv" => m.pv,
            ))?;
        }
        Ok(dict!(py, "months" => months, "pv" => v.pv, "total_mwh" => v.total_mwh, "par_fixed" => v.par_fixed))
    }

    fn __repr__(&self) -> String {
        format!("BlockSwap({:?})", self.0)
    }
}

fn heat_rate_dict<'py>(py: Python<'py>, v: &HeatRateValuation) -> PyResult<Bound<'py, PyDict>> {
    let months = PyList::empty(py);
    for m in &v.months {
        months.append(dict!(py,
            "month" => PyContractMonth(m.month), "hours" => m.hours, "power_forward" => m.power_forward, "gas_forward" => m.gas_forward,
            "forward_spark" => m.forward_spark, "implied_heat_rate" => m.implied_heat_rate, "time_to_expiry" => m.time_to_expiry,
            "discount" => m.discount, "intrinsic_pv" => m.intrinsic_pv, "option_pv" => m.option_pv,
        ))?;
    }
    Ok(dict!(py, "months" => months, "intrinsic_pv" => v.intrinsic_pv, "option_pv" => v.option_pv, "extrinsic_pv" => v.extrinsic_pv))
}

/// Strip of monthly spark-spread options.
#[pyclass(name = "HeatRateOption", module = "ferrumquant", frozen)]
#[derive(Debug, Clone)]
pub struct PyHeatRateOption(pub HeatRateOption);

#[pymethods]
impl PyHeatRateOption {
    /// `HeatRateOption("call", hub, block, start, end, mw, heat_rate, vom, method="bs")`.
    #[new]
    #[allow(clippy::too_many_arguments)]
    #[pyo3(signature = (kind, hub, block, start, end, mw, heat_rate, vom, method = "bs"))]
    fn new(kind: &str, hub: &str, block: &str, start: MonthLike, end: MonthLike, mw: f64, heat_rate: f64, vom: f64, method: &str) -> PyResult<Self> {
        let method = parse_method(method)?;
        HeatRateOption::new(parse_kind(kind)?, parse_power_hub(hub)?, parse_block(block)?, start.0, end.0, mw, heat_rate, vom)
            .map(|o| Self(o.with_method(method)))
            .map_err(err)
    }

    fn months(&self) -> Vec<PyContractMonth> {
        self.0.months().into_iter().map(PyContractMonth).collect()
    }

    #[allow(clippy::too_many_arguments)]
    #[pyo3(signature = (power, gas, power_vol, gas_vol, rho, discount, day_count = "ACT/365F"))]
    fn value<'py>(
        &self,
        py: Python<'py>,
        power: &PyPowerCurve,
        gas: &PyFuturesCurve,
        power_vol: VolLike,
        gas_vol: VolLike,
        rho: f64,
        discount: &PyDiscountCurve,
        day_count: &str,
    ) -> PyResult<Bound<'py, PyDict>> {
        let anchor = gas.0.first_month();
        let v =
            self.0.value(&power.0, &gas.0, &power_vol.seasonal(anchor)?, &gas_vol.seasonal(anchor)?, rho, &discount.0, parse_dc(day_count)?).map_err(err)?;
        heat_rate_dict(py, &v)
    }

    fn __repr__(&self) -> String {
        format!("HeatRateOption({:?})", self.0)
    }
}

/// Tolling agreement: heat-rate call strip less capacity payments.
#[pyclass(name = "TollingAgreement", module = "ferrumquant", frozen)]
#[derive(Debug, Clone)]
pub struct PyTollingAgreement(pub TollingAgreement);

#[pymethods]
impl PyTollingAgreement {
    /// `TollingAgreement(hub, block, start, end, mw, heat_rate, vom, capacity_payment)`; capacity payment is $/kW-month.
    #[new]
    #[allow(clippy::too_many_arguments)]
    fn new(hub: &str, block: &str, start: MonthLike, end: MonthLike, mw: f64, heat_rate: f64, vom: f64, capacity_payment: f64) -> PyResult<Self> {
        TollingAgreement::new(parse_power_hub(hub)?, parse_block(block)?, start.0, end.0, mw, heat_rate, vom, capacity_payment).map(Self).map_err(err)
    }

    #[allow(clippy::too_many_arguments)]
    #[pyo3(signature = (power, gas, power_vol, gas_vol, rho, discount, day_count = "ACT/365F"))]
    fn value<'py>(
        &self,
        py: Python<'py>,
        power: &PyPowerCurve,
        gas: &PyFuturesCurve,
        power_vol: VolLike,
        gas_vol: VolLike,
        rho: f64,
        discount: &PyDiscountCurve,
        day_count: &str,
    ) -> PyResult<Bound<'py, PyDict>> {
        let anchor = gas.0.first_month();
        let v =
            self.0.value(&power.0, &gas.0, &power_vol.seasonal(anchor)?, &gas_vol.seasonal(anchor)?, rho, &discount.0, parse_dc(day_count)?).map_err(err)?;
        Ok(dict!(py, "spark" => heat_rate_dict(py, &v.spark)?, "capacity_pv" => v.capacity_pv, "net_pv" => v.net_pv))
    }

    fn __repr__(&self) -> String {
        format!("TollingAgreement({:?})", self.0)
    }
}

/// Thermal unit for the hourly commitment DP.
#[pyclass(name = "ThermalUnit", module = "ferrumquant", frozen)]
#[derive(Debug, Clone)]
pub struct PyThermalUnit(pub ThermalUnit);

#[pymethods]
impl PyThermalUnit {
    /// `ThermalUnit(capacity_mw, min_load_mw, heat_rate, vom, start_cost, min_up_hours, min_down_hours, on=False, hours_in_state=0)`.
    #[new]
    #[allow(clippy::too_many_arguments)]
    #[pyo3(signature = (capacity_mw, min_load_mw, heat_rate, vom, start_cost, min_up_hours, min_down_hours, on = false, hours_in_state = 0))]
    fn new(
        capacity_mw: f64,
        min_load_mw: f64,
        heat_rate: f64,
        vom: f64,
        start_cost: f64,
        min_up_hours: u32,
        min_down_hours: u32,
        on: bool,
        hours_in_state: u32,
    ) -> PyResult<Self> {
        ThermalUnit::new(capacity_mw, min_load_mw, heat_rate, vom, start_cost, min_up_hours, min_down_hours)
            .map(|u| Self(u.starting(on, hours_in_state)))
            .map_err(err)
    }

    /// Margin per MWh for given power ($/MWh) and gas ($/MMBtu).
    fn margin(&self, power: f64, gas: f64) -> f64 {
        self.0.margin(power, gas)
    }

    /// Optimal commitment against hourly `power` and `gas` (gas may be a single value).
    fn dispatch<'py>(&self, py: Python<'py>, power: Vec<f64>, gas: Vec<f64>) -> PyResult<Bound<'py, PyDict>> {
        let r = dispatch(&self.0, &power, &gas).map_err(err)?;
        let schedule = PyList::empty(py);
        for h in &r.schedule {
            schedule.append(dict!(py, "on" => h.on, "mw" => h.mw, "started" => h.started, "profit" => h.profit))?;
        }
        Ok(dict!(py,
            "value" => r.value, "schedule" => schedule, "starts" => r.starts, "hours_on" => r.hours_on, "frictionless_value" => r.frictionless_value,
        ))
    }

    fn __repr__(&self) -> String {
        format!("ThermalUnit({:?})", self.0)
    }
}

pub(crate) fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyPowerCurve>()?;
    m.add_class::<PyBlockSwap>()?;
    m.add_class::<PyHeatRateOption>()?;
    m.add_class::<PyTollingAgreement>()?;
    m.add_class::<PyThermalUnit>()?;
    m.add_function(wrap_pyfunction!(py_power_hubs, m)?)?;
    m.add_function(wrap_pyfunction!(py_power_hub, m)?)?;
    m.add_function(wrap_pyfunction!(py_nerc_holidays, m)?)?;
    m.add_function(wrap_pyfunction!(py_is_nerc_holiday, m)?)?;
    m.add_function(wrap_pyfunction!(py_is_peak_day, m)?)?;
    m.add_function(wrap_pyfunction!(py_is_peak_hour, m)?)?;
    m.add_function(wrap_pyfunction!(py_hours_in_day, m)?)?;
    m.add_function(wrap_pyfunction!(py_block_hours, m)?)?;
    Ok(())
}
