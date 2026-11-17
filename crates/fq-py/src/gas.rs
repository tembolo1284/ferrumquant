//! Natural gas: hubs, NYMEX dates, swaps, transport, storage and swing.

use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList};

use fq_commod_core::McConfig;
use fq_gas::{
    BasisSwap, FixedFloatSwap, GasHub, NymexCalendar, StorageFacility, SwapValuation, SwingOption, TransportCapacity, bidweek, intrinsic_storage, nymex_expiry,
    rolling_intrinsic_storage,
};

use crate::commod::{MonthLike, PyBasisCurve, PyClewlowStrickland, PyContractMonth, PyFuturesCurve, PyHub, VolLike, parse_dc, parse_method};
use crate::curve::PyDiscountCurve;
use crate::time::PyDate;
use crate::{FerrumError, dict, err};

pub(crate) fn parse_gas_hub(s: &str) -> PyResult<GasHub> {
    GasHub::from_code(s).ok_or_else(|| FerrumError::new_err(format!("unknown gas hub '{s}'; see gas_hubs()")))
}

fn swap_dict<'py>(py: Python<'py>, v: &SwapValuation) -> PyResult<Bound<'py, PyDict>> {
    let months = PyList::empty(py);
    for m in &v.months {
        months.append(dict!(py,
            "month" => PyContractMonth(m.month), "days" => m.days, "quantity" => m.quantity,
            "forward" => m.forward, "discount" => m.discount, "pv" => m.pv,
        ))?;
    }
    Ok(dict!(py, "months" => months, "pv" => v.pv, "par_fixed" => v.par_fixed, "total_quantity" => v.total_quantity))
}

/// NYMEX trading calendar (US settlement).
#[pyclass(name = "NymexCalendar", module = "ferrumquant", frozen)]
#[derive(Debug, Clone)]
pub struct PyNymexCalendar(pub NymexCalendar);

#[pymethods]
impl PyNymexCalendar {
    #[new]
    fn new() -> Self {
        Self(NymexCalendar::new())
    }

    /// Last trading day of the Henry Hub future for `month`.
    fn expiry(&self, month: MonthLike) -> PyResult<PyDate> {
        nymex_expiry(month.0, &self.0).map(PyDate).map_err(err)
    }

    /// Bidweek trading days for `month`.
    fn bidweek(&self, month: MonthLike) -> PyResult<Vec<PyDate>> {
        bidweek(month.0, &self.0).map(|v| v.into_iter().map(PyDate).collect()).map_err(err)
    }

    fn __repr__(&self) -> String {
        "NymexCalendar()".into()
    }
}

/// All gas hub codes with names.
#[pyfunction]
#[pyo3(name = "gas_hubs")]
fn py_gas_hubs(py: Python<'_>) -> PyResult<Bound<'_, PyDict>> {
    let d = PyDict::new(py);
    for h in GasHub::ALL {
        d.set_item(h.code(), h.name())?;
    }
    Ok(d)
}

/// The `Hub` behind a gas hub code (for building a `FuturesCurve`).
#[pyfunction]
#[pyo3(name = "gas_hub")]
fn py_gas_hub(code: &str) -> PyResult<PyHub> {
    parse_gas_hub(code).map(|h| PyHub(h.hub()))
}

/// Fixed-for-floating monthly index swap.
#[pyclass(name = "GasSwap", module = "ferrumquant", frozen)]
#[derive(Debug, Clone)]
pub struct PyGasSwap(pub FixedFloatSwap);

#[pymethods]
impl PyGasSwap {
    /// `GasSwap("HH", start, end, daily_quantity, fixed_price, pay_fixed=True)`.
    #[new]
    #[pyo3(signature = (hub, start, end, daily_quantity, fixed_price, pay_fixed = true))]
    fn new(hub: &str, start: MonthLike, end: MonthLike, daily_quantity: f64, fixed_price: f64, pay_fixed: bool) -> PyResult<Self> {
        FixedFloatSwap::new(parse_gas_hub(hub)?, start.0, end.0, daily_quantity, fixed_price, pay_fixed).map(Self).map_err(err)
    }

    fn months(&self) -> Vec<PyContractMonth> {
        self.0.months().into_iter().map(PyContractMonth).collect()
    }

    fn value<'py>(&self, py: Python<'py>, curve: &PyFuturesCurve, discount: &PyDiscountCurve, cal: &PyNymexCalendar) -> PyResult<Bound<'py, PyDict>> {
        swap_dict(py, &self.0.value(&curve.0, &discount.0, &cal.0).map_err(err)?)
    }

    fn __repr__(&self) -> String {
        format!("GasSwap({:?})", self.0)
    }
}

/// Fixed basis against Henry Hub.
#[pyclass(name = "GasBasisSwap", module = "ferrumquant", frozen)]
#[derive(Debug, Clone)]
pub struct PyGasBasisSwap(pub BasisSwap);

#[pymethods]
impl PyGasBasisSwap {
    #[new]
    #[pyo3(signature = (hub, start, end, daily_quantity, fixed_basis, receive_basis = true))]
    fn new(hub: &str, start: MonthLike, end: MonthLike, daily_quantity: f64, fixed_basis: f64, receive_basis: bool) -> PyResult<Self> {
        BasisSwap::new(parse_gas_hub(hub)?, start.0, end.0, daily_quantity, fixed_basis, receive_basis).map(Self).map_err(err)
    }

    fn months(&self) -> Vec<PyContractMonth> {
        self.0.months().into_iter().map(PyContractMonth).collect()
    }

    fn value<'py>(&self, py: Python<'py>, basis: &PyBasisCurve, discount: &PyDiscountCurve, cal: &PyNymexCalendar) -> PyResult<Bound<'py, PyDict>> {
        swap_dict(py, &self.0.value(&basis.0, &discount.0, &cal.0).map_err(err)?)
    }

    fn __repr__(&self) -> String {
        format!("GasBasisSwap({:?})", self.0)
    }
}

/// Firm transport capacity as a strip of locational spread options.
#[pyclass(name = "TransportCapacity", module = "ferrumquant", frozen)]
#[derive(Debug, Clone)]
pub struct PyTransportCapacity(pub TransportCapacity);

#[pymethods]
impl PyTransportCapacity {
    /// `TransportCapacity(receipt, delivery, start, end, mdq, fuel_retention, commodity_charge, demand_charge, method="bs")`.
    #[new]
    #[allow(clippy::too_many_arguments)]
    #[pyo3(signature = (receipt, delivery, start, end, mdq, fuel_retention, commodity_charge, demand_charge, method = "bs"))]
    fn new(
        receipt: &str,
        delivery: &str,
        start: MonthLike,
        end: MonthLike,
        mdq: f64,
        fuel_retention: f64,
        commodity_charge: f64,
        demand_charge: f64,
        method: &str,
    ) -> PyResult<Self> {
        let method = parse_method(method)?;
        TransportCapacity::new(parse_gas_hub(receipt)?, parse_gas_hub(delivery)?, start.0, end.0, mdq, fuel_retention, commodity_charge, demand_charge)
            .map(|t| Self(t.with_method(method)))
            .map_err(err)
    }

    fn months(&self) -> Vec<PyContractMonth> {
        self.0.months().into_iter().map(PyContractMonth).collect()
    }

    /// Vols are `SeasonalVol` or flat numbers.
    #[allow(clippy::too_many_arguments)]
    #[pyo3(signature = (receipt_curve, delivery_curve, receipt_vol, delivery_vol, rho, discount, cal, day_count = "ACT/365F"))]
    fn value<'py>(
        &self,
        py: Python<'py>,
        receipt_curve: &PyFuturesCurve,
        delivery_curve: &PyFuturesCurve,
        receipt_vol: VolLike,
        delivery_vol: VolLike,
        rho: f64,
        discount: &PyDiscountCurve,
        cal: &PyNymexCalendar,
        day_count: &str,
    ) -> PyResult<Bound<'py, PyDict>> {
        let anchor = receipt_curve.0.first_month();
        let v = self
            .0
            .value(
                &receipt_curve.0,
                &delivery_curve.0,
                &receipt_vol.seasonal(anchor)?,
                &delivery_vol.seasonal(anchor)?,
                rho,
                &discount.0,
                &cal.0,
                parse_dc(day_count)?,
            )
            .map_err(err)?;
        let months = PyList::empty(py);
        for m in &v.months {
            months.append(dict!(py,
                "month" => PyContractMonth(m.month), "days" => m.days, "receipt_forward" => m.receipt_forward, "delivery_forward" => m.delivery_forward,
                "forward_margin" => m.forward_margin, "discount" => m.discount, "intrinsic_pv" => m.intrinsic_pv, "option_pv" => m.option_pv, "demand_pv" => m.demand_pv,
            ))?;
        }
        Ok(dict!(py,
            "months" => months, "intrinsic_pv" => v.intrinsic_pv, "option_pv" => v.option_pv, "extrinsic_pv" => v.extrinsic_pv,
            "demand_pv" => v.demand_pv, "net_pv" => v.net_pv,
        ))
    }

    fn __repr__(&self) -> String {
        format!("TransportCapacity({:?})", self.0)
    }
}

/// Storage lease: intrinsic DP and rolling-intrinsic Monte Carlo.
#[pyclass(name = "StorageFacility", module = "ferrumquant", frozen)]
#[derive(Debug, Clone)]
pub struct PyStorageFacility(pub StorageFacility);

fn storage_dict<'py>(py: Python<'py>, v: &fq_gas::StorageValuation) -> PyResult<Bound<'py, PyDict>> {
    let schedule = PyList::empty(py);
    for s in &v.schedule {
        schedule.append(dict!(py,
            "month" => PyContractMonth(s.month), "price" => s.price, "inventory_start" => s.inventory_start, "change" => s.change, "cash_pv" => s.cash_pv,
        ))?;
    }
    Ok(dict!(py,
        "intrinsic_pv" => v.intrinsic_pv, "schedule" => schedule, "rolling_pv" => v.rolling_pv,
        "rolling_std_err" => v.rolling_std_err, "extrinsic_pv" => v.extrinsic_pv,
    ))
}

#[pymethods]
impl PyStorageFacility {
    /// `StorageFacility(hub, start, end, capacity, max_injection, max_withdrawal, injection_cost, withdrawal_cost, fuel_in=0.0, initial_inventory=0.0, final_min_inventory=0.0, grid_levels=None)`.
    #[new]
    #[allow(clippy::too_many_arguments)]
    #[pyo3(signature = (hub, start, end, capacity, max_injection, max_withdrawal, injection_cost, withdrawal_cost, fuel_in = 0.0, initial_inventory = 0.0, final_min_inventory = 0.0, grid_levels = None))]
    fn new(
        hub: &str,
        start: MonthLike,
        end: MonthLike,
        capacity: f64,
        max_injection: f64,
        max_withdrawal: f64,
        injection_cost: f64,
        withdrawal_cost: f64,
        fuel_in: f64,
        initial_inventory: f64,
        final_min_inventory: f64,
        grid_levels: Option<usize>,
    ) -> PyResult<Self> {
        let mut f =
            StorageFacility::new(parse_gas_hub(hub)?, start.0, end.0, capacity, max_injection, max_withdrawal, injection_cost, withdrawal_cost, fuel_in)
                .map_err(err)?;
        if initial_inventory != 0.0 || final_min_inventory != 0.0 {
            f = f.with_inventory(initial_inventory, final_min_inventory).map_err(err)?;
        }
        if let Some(n) = grid_levels {
            f = f.with_grid_levels(n).map_err(err)?;
        }
        Ok(Self(f))
    }

    #[getter]
    fn capacity(&self) -> f64 {
        self.0.capacity
    }

    fn months(&self) -> Vec<PyContractMonth> {
        self.0.months().into_iter().map(PyContractMonth).collect()
    }

    /// Optimal injection/withdrawal schedule against today's curve.
    fn intrinsic<'py>(&self, py: Python<'py>, curve: &PyFuturesCurve, discount: &PyDiscountCurve, cal: &PyNymexCalendar) -> PyResult<Bound<'py, PyDict>> {
        storage_dict(py, &intrinsic_storage(&self.0, &curve.0, &discount.0, &cal.0).map_err(err)?)
    }

    /// Rolling-intrinsic Monte Carlo under a Clewlow-Strickland model.
    #[allow(clippy::too_many_arguments)]
    #[pyo3(signature = (curve, model, discount, cal, day_count = "ACT/365F", paths = 2000, seed = 12_345, antithetic = true))]
    fn rolling_intrinsic<'py>(
        &self,
        py: Python<'py>,
        curve: &PyFuturesCurve,
        model: &PyClewlowStrickland,
        discount: &PyDiscountCurve,
        cal: &PyNymexCalendar,
        day_count: &str,
        paths: usize,
        seed: u64,
        antithetic: bool,
    ) -> PyResult<Bound<'py, PyDict>> {
        let v = rolling_intrinsic_storage(&self.0, &curve.0, &model.0, &discount.0, &cal.0, parse_dc(day_count)?, McConfig { paths, seed, antithetic })
            .map_err(err)?;
        storage_dict(py, &v)
    }

    fn __repr__(&self) -> String {
        format!("StorageFacility({:?})", self.0)
    }
}

/// Swing (take-or-pay) contract.
#[pyclass(name = "SwingOption", module = "ferrumquant", frozen)]
#[derive(Debug, Clone)]
pub struct PySwingOption(pub SwingOption);

fn swing_dict<'py>(py: Python<'py>, v: &fq_gas::SwingValuation) -> PyResult<Bound<'py, PyDict>> {
    let months: Vec<PyContractMonth> = v.months.iter().copied().map(PyContractMonth).collect();
    Ok(dict!(py,
        "months" => months, "intrinsic_takes" => v.intrinsic_takes.clone(), "intrinsic_pv" => v.intrinsic_pv,
        "upper_bound_pv" => v.upper_bound_pv, "rolling_pv" => v.rolling_pv, "rolling_std_err" => v.rolling_std_err,
    ))
}

#[pymethods]
impl PySwingOption {
    /// `SwingOption(hub, start, end, strike, min_daily, max_daily, min_total, max_total)`.
    #[new]
    #[allow(clippy::too_many_arguments)]
    fn new(hub: &str, start: MonthLike, end: MonthLike, strike: f64, min_daily: f64, max_daily: f64, min_total: f64, max_total: f64) -> PyResult<Self> {
        SwingOption::new(parse_gas_hub(hub)?, start.0, end.0, strike, min_daily, max_daily, min_total, max_total).map(Self).map_err(err)
    }

    fn months(&self) -> Vec<PyContractMonth> {
        self.0.months().into_iter().map(PyContractMonth).collect()
    }

    /// Intrinsic plus Black-76 upper bound.
    #[pyo3(signature = (curve, vol, discount, cal, day_count = "ACT/365F"))]
    fn value<'py>(
        &self,
        py: Python<'py>,
        curve: &PyFuturesCurve,
        vol: VolLike,
        discount: &PyDiscountCurve,
        cal: &PyNymexCalendar,
        day_count: &str,
    ) -> PyResult<Bound<'py, PyDict>> {
        let v = self.0.value(&curve.0, &vol.seasonal(curve.0.first_month())?, &discount.0, &cal.0, parse_dc(day_count)?).map_err(err)?;
        swing_dict(py, &v)
    }

    /// Adds a rolling-intrinsic Monte Carlo estimate.
    #[allow(clippy::too_many_arguments)]
    #[pyo3(signature = (curve, vol, model, discount, cal, day_count = "ACT/365F", paths = 2000, seed = 12_345, antithetic = true))]
    fn value_with_model<'py>(
        &self,
        py: Python<'py>,
        curve: &PyFuturesCurve,
        vol: VolLike,
        model: &PyClewlowStrickland,
        discount: &PyDiscountCurve,
        cal: &PyNymexCalendar,
        day_count: &str,
        paths: usize,
        seed: u64,
        antithetic: bool,
    ) -> PyResult<Bound<'py, PyDict>> {
        let v = self
            .0
            .value_with_model(
                &curve.0,
                &vol.seasonal(curve.0.first_month())?,
                &model.0,
                &discount.0,
                &cal.0,
                parse_dc(day_count)?,
                McConfig { paths, seed, antithetic },
            )
            .map_err(err)?;
        swing_dict(py, &v)
    }

    fn __repr__(&self) -> String {
        format!("SwingOption({:?})", self.0)
    }
}

pub(crate) fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyNymexCalendar>()?;
    m.add_class::<PyGasSwap>()?;
    m.add_class::<PyGasBasisSwap>()?;
    m.add_class::<PyTransportCapacity>()?;
    m.add_class::<PyStorageFacility>()?;
    m.add_class::<PySwingOption>()?;
    m.add_function(wrap_pyfunction!(py_gas_hubs, m)?)?;
    m.add_function(wrap_pyfunction!(py_gas_hub, m)?)?;
    Ok(())
}
