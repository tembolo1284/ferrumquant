//! Commodity core: contract months, hubs, futures and basis curves,
//! seasonal vol, options on futures, Asian and spread options, and the
//! Clewlow-Strickland Monte Carlo model.
//!
//! Months are accepted as `ContractMonth`, `"2027-01"`, `"2027-01-15"`
//! (its month), `(2027, 1)` or a futures code like `"F27"` / `"Z6"`.
//! Option kinds are `"call"` / `"put"`, spread methods `"margrabe"`,
//! `"kirk"` or `"bs"` (Bjerksund-Stensland, the default), day counts any
//! string `fq_core` parses (`"ACT/365F"` default).

use pyo3::prelude::*;
use pyo3::types::{PyDict, PyType};

use fq_commod_core::spread::{implied_correlation, spread_price};
use fq_commod_core::{
    AsianOption, BasisCurve, ClewlowStrickland, Commodity, ContractMonth, CurveInterpolation, CurvePaths, DeliveryPeriod, Fixing, FuturesCurve, FuturesOption,
    FuturesQuote, Hub, McConfig, SamuelsonVol, SeasonalVol, SpreadMethod, SpreadOption, Unit, VolQuote,
};
use fq_core::math::OptionType;
use fq_core::time::DayCount;

use crate::curve::PyDiscountCurve;
use crate::time::{DateLike, PyDate};
use crate::{FerrumError, dict, err};

// ---------------------------------------------------------------------------
// Shared parsers
// ---------------------------------------------------------------------------

pub(crate) fn parse_kind(s: &str) -> PyResult<OptionType> {
    match s.to_ascii_lowercase().as_str() {
        "call" | "c" => Ok(OptionType::Call),
        "put" | "p" => Ok(OptionType::Put),
        _ => Err(FerrumError::new_err(format!("unknown option kind '{s}' (call | put)"))),
    }
}

pub(crate) fn kind_name(k: OptionType) -> &'static str {
    match k {
        OptionType::Call => "call",
        OptionType::Put => "put",
    }
}

pub(crate) fn parse_dc(s: &str) -> PyResult<DayCount> {
    s.parse::<DayCount>().map_err(err)
}

pub(crate) fn parse_method(s: &str) -> PyResult<SpreadMethod> {
    match s.to_ascii_lowercase().replace(['-', '_', ' '], "").as_str() {
        "margrabe" => Ok(SpreadMethod::Margrabe),
        "kirk" => Ok(SpreadMethod::Kirk),
        "bs" | "bjerksundstensland" | "bjerksund" => Ok(SpreadMethod::BjerksundStensland),
        _ => Err(FerrumError::new_err(format!("unknown spread method '{s}' (margrabe | kirk | bs)"))),
    }
}

fn parse_commodity(s: &str) -> PyResult<Commodity> {
    Ok(match s.to_ascii_lowercase().replace([' ', '_', '-'], "").as_str() {
        "naturalgas" | "gas" | "ng" => Commodity::NaturalGas,
        "power" | "electricity" => Commodity::Power,
        "crude" | "oil" => Commodity::Crude,
        "refinedproduct" | "product" | "products" => Commodity::RefinedProduct,
        "coal" => Commodity::Coal,
        "emissions" | "carbon" => Commodity::Emissions,
        "preciousmetal" | "precious" => Commodity::PreciousMetal,
        "basemetal" | "base" => Commodity::BaseMetal,
        "grain" | "grains" => Commodity::Grain,
        "soft" | "softs" => Commodity::Soft,
        "livestock" => Commodity::Livestock,
        _ => return Err(FerrumError::new_err(format!("unknown commodity '{s}'"))),
    })
}

fn parse_unit(s: &str) -> PyResult<Unit> {
    Ok(match s.to_ascii_lowercase().replace([' ', '_', '-'], "").as_str() {
        "mmbtu" => Unit::MmBtu,
        "mwh" => Unit::MWh,
        "bbl" | "barrel" => Unit::Barrel,
        "gal" | "gallon" => Unit::Gallon,
        "ozt" | "oz" | "troyounce" => Unit::TroyOunce,
        "t" | "mt" | "tonne" | "metricton" => Unit::MetricTon,
        "bu" | "bushel" => Unit::Bushel,
        "lb" | "pound" => Unit::Pound,
        "tco2e" | "allowance" => Unit::Allowance,
        _ => return Err(FerrumError::new_err(format!("unknown unit '{s}'"))),
    })
}

/// Anything accepted where a contract month is expected.
#[derive(Debug, Clone, Copy)]
pub struct MonthLike(pub ContractMonth);

impl<'py> FromPyObject<'py> for MonthLike {
    fn extract_bound(obj: &Bound<'py, PyAny>) -> PyResult<Self> {
        if let Ok(m) = obj.downcast::<PyContractMonth>() {
            return Ok(Self(m.get().0));
        }
        if let Ok((y, m)) = obj.extract::<(i32, u32)>() {
            return ContractMonth::new(y, m).map(Self).map_err(err);
        }
        if let Ok(s) = obj.extract::<String>() {
            return parse_month_str(&s).map(Self);
        }
        if let Ok(d) = obj.extract::<DateLike>() {
            return Ok(Self(ContractMonth::of(d.0)));
        }
        Err(FerrumError::new_err(format!("cannot interpret {} as a contract month", obj.get_type().name()?)))
    }
}

fn parse_month_str(s: &str) -> PyResult<ContractMonth> {
    let t = s.trim();
    // YYYY-MM or YYYY-MM-DD
    if t.len() >= 7 && t.as_bytes()[4] == b'-' {
        let y: i32 = t[..4].parse().map_err(|_| FerrumError::new_err(format!("bad month '{s}'")))?;
        let m: u32 = t[5..7].parse().map_err(|_| FerrumError::new_err(format!("bad month '{s}'")))?;
        return ContractMonth::new(y, m).map_err(err);
    }
    // Futures code: F27, Z6, H2027
    let mut chars = t.chars();
    let code = chars.next().ok_or_else(|| FerrumError::new_err("empty month"))?.to_ascii_uppercase();
    let month = match code {
        'F' => 1,
        'G' => 2,
        'H' => 3,
        'J' => 4,
        'K' => 5,
        'M' => 6,
        'N' => 7,
        'Q' => 8,
        'U' => 9,
        'V' => 10,
        'X' => 11,
        'Z' => 12,
        _ => return Err(FerrumError::new_err(format!("bad month '{s}' (YYYY-MM, (y, m) or a code like F27)"))),
    };
    let rest: String = chars.collect();
    let year: i32 = match rest.len() {
        1 => 2020 + rest.parse::<i32>().map_err(|_| FerrumError::new_err(format!("bad month '{s}'")))?,
        2 => 2000 + rest.parse::<i32>().map_err(|_| FerrumError::new_err(format!("bad month '{s}'")))?,
        4 => rest.parse::<i32>().map_err(|_| FerrumError::new_err(format!("bad month '{s}'")))?,
        _ => return Err(FerrumError::new_err(format!("bad month '{s}'"))),
    };
    ContractMonth::new(year, month).map_err(err)
}

/// A `SeasonalVol` or a flat float.
#[derive(Debug, Clone)]
pub enum VolLike {
    Flat(f64),
    Seasonal(SeasonalVol),
}

impl VolLike {
    pub fn vol(&self, m: ContractMonth) -> f64 {
        match self {
            Self::Flat(v) => *v,
            Self::Seasonal(s) => s.vol(m),
        }
    }

    pub fn seasonal(&self, anchor: ContractMonth) -> PyResult<SeasonalVol> {
        match self {
            Self::Flat(v) => SeasonalVol::flat(anchor, *v).map_err(err),
            Self::Seasonal(s) => Ok(s.clone()),
        }
    }
}

impl<'py> FromPyObject<'py> for VolLike {
    fn extract_bound(obj: &Bound<'py, PyAny>) -> PyResult<Self> {
        if let Ok(v) = obj.downcast::<PySeasonalVol>() {
            return Ok(Self::Seasonal(v.get().0.clone()));
        }
        Ok(Self::Flat(obj.extract::<f64>()?))
    }
}

// ---------------------------------------------------------------------------
// ContractMonth
// ---------------------------------------------------------------------------

/// A delivery month, the key of every commodity curve.
#[pyclass(name = "ContractMonth", module = "ferrumquant", frozen, eq, ord, hash)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PyContractMonth(pub ContractMonth);

#[pymethods]
impl PyContractMonth {
    #[new]
    fn new(year: i32, month: u32) -> PyResult<Self> {
        ContractMonth::new(year, month).map(Self).map_err(err)
    }

    #[classmethod]
    fn from_any(_cls: &Bound<'_, PyType>, value: MonthLike) -> Self {
        Self(value.0)
    }

    #[getter]
    fn year(&self) -> i32 {
        self.0.year()
    }

    #[getter]
    fn month(&self) -> u32 {
        self.0.month()
    }

    /// Futures month code, e.g. "F27".
    #[getter]
    fn code(&self) -> String {
        self.0.code()
    }

    #[getter]
    fn is_winter(&self) -> bool {
        self.0.is_winter()
    }

    fn next(&self) -> Self {
        Self(self.0.next())
    }

    fn prev(&self) -> Self {
        Self(self.0.prev())
    }

    fn add_months(&self, n: i32) -> Self {
        Self(self.0.add_months(n))
    }

    fn months_until(&self, other: MonthLike) -> i32 {
        self.0.months_until(other.0)
    }

    fn first_day(&self) -> PyResult<PyDate> {
        self.0.first_day().map(PyDate).map_err(err)
    }

    fn last_day(&self) -> PyResult<PyDate> {
        self.0.last_day().map(PyDate).map_err(err)
    }

    /// Calendar days in the month.
    fn days(&self) -> PyResult<i32> {
        Ok(self.0.delivery().map_err(err)?.days())
    }

    /// Inclusive list of months from `self` to `to`.
    fn range_to(&self, to: MonthLike) -> Vec<Self> {
        ContractMonth::range(self.0, to.0).into_iter().map(Self).collect()
    }

    fn __str__(&self) -> String {
        self.0.to_string()
    }

    fn __repr__(&self) -> String {
        format!("ContractMonth({})", self.0)
    }
}

// ---------------------------------------------------------------------------
// Hub
// ---------------------------------------------------------------------------

/// A pricing location with its commodity and unit.
#[pyclass(name = "Hub", module = "ferrumquant", frozen, eq, hash)]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PyHub(pub Hub);

#[pymethods]
impl PyHub {
    /// `Hub("HH", "natural_gas", "MMBtu")`.
    #[new]
    fn new(code: &str, commodity: &str, unit: &str) -> PyResult<Self> {
        Ok(Self(Hub::new(code, parse_commodity(commodity)?, parse_unit(unit)?)))
    }

    #[getter]
    fn code(&self) -> &str {
        self.0.code()
    }

    #[getter]
    fn commodity(&self) -> String {
        format!("{:?}", self.0.commodity())
    }

    #[getter]
    fn unit(&self) -> &'static str {
        self.0.unit().code()
    }

    fn __repr__(&self) -> String {
        format!("Hub({})", self.0)
    }
}

// ---------------------------------------------------------------------------
// FuturesCurve / BasisCurve
// ---------------------------------------------------------------------------

fn parse_interp(s: &str) -> PyResult<CurveInterpolation> {
    match s.to_ascii_lowercase().replace(['-', ' '], "_").as_str() {
        "linear" => Ok(CurveInterpolation::Linear),
        "previous_flat" | "flat" | "previous" => Ok(CurveInterpolation::PreviousFlat),
        _ => Err(FerrumError::new_err(format!("unknown interpolation '{s}' (linear | previous_flat)"))),
    }
}

/// Futures settlement curve keyed by delivery month.
#[pyclass(name = "FuturesCurve", module = "ferrumquant", frozen)]
#[derive(Debug, Clone)]
pub struct PyFuturesCurve(pub FuturesCurve);

#[pymethods]
impl PyFuturesCurve {
    /// `FuturesCurve(hub, reference, [(month, price), ...], interpolation="linear")`.
    #[new]
    #[pyo3(signature = (hub, reference, quotes, interpolation = "linear"))]
    fn new(hub: &PyHub, reference: DateLike, quotes: Vec<(MonthLike, f64)>, interpolation: &str) -> PyResult<Self> {
        let q = quotes.into_iter().map(|(m, p)| FuturesQuote::new(m.0, p)).collect();
        FuturesCurve::new(hub.0.clone(), reference.0, q, parse_interp(interpolation)?).map(Self).map_err(err)
    }

    #[getter]
    fn hub(&self) -> PyHub {
        PyHub(self.0.hub().clone())
    }

    #[getter]
    fn reference_date(&self) -> PyDate {
        PyDate(self.0.reference_date())
    }

    #[getter]
    fn first_month(&self) -> PyContractMonth {
        PyContractMonth(self.0.first_month())
    }

    #[getter]
    fn last_month(&self) -> PyContractMonth {
        PyContractMonth(self.0.last_month())
    }

    fn quotes(&self) -> Vec<(PyContractMonth, f64)> {
        self.0.quotes().iter().map(|q| (PyContractMonth(q.month), q.price)).collect()
    }

    fn price(&self, month: MonthLike) -> PyResult<f64> {
        self.0.price(month.0).map_err(err)
    }

    fn front_month(&self, d: DateLike) -> PyResult<PyContractMonth> {
        self.0.front_month(d.0).map(PyContractMonth).map_err(err)
    }

    /// Day-weighted average price over `[start, end]`.
    fn strip_price(&self, start: DateLike, end: DateLike) -> PyResult<f64> {
        let p = DeliveryPeriod::new(start.0, end.0).map_err(err)?;
        self.0.strip_price(p).map_err(err)
    }

    /// Equal-weighted average over an inclusive month range.
    fn average(&self, from: MonthLike, to: MonthLike) -> PyResult<f64> {
        self.0.average(from.0, to.0).map_err(err)
    }

    /// `price(far) - price(near)`.
    fn calendar_spread(&self, near: MonthLike, far: MonthLike) -> PyResult<f64> {
        self.0.calendar_spread(near.0, far.0).map(|s| s.spread).map_err(err)
    }

    fn shifted(&self, bump: f64) -> PyResult<Self> {
        self.0.shifted(bump).map(Self).map_err(err)
    }

    fn scaled(&self, factor: f64) -> PyResult<Self> {
        self.0.scaled(factor).map(Self).map_err(err)
    }

    fn bumped_month(&self, month: MonthLike, bump: f64) -> PyResult<Self> {
        self.0.bumped_month(month.0, bump).map(Self).map_err(err)
    }

    fn __repr__(&self) -> String {
        format!("FuturesCurve({}, {} quotes {}..{})", self.0.hub(), self.0.quotes().len(), self.0.first_month(), self.0.last_month())
    }
}

/// Locational basis `hub - benchmark` by month.
#[pyclass(name = "BasisCurve", module = "ferrumquant", frozen)]
#[derive(Debug, Clone)]
pub struct PyBasisCurve(pub BasisCurve);

#[pymethods]
impl PyBasisCurve {
    #[new]
    fn new(hub: &PyHub, benchmark: &PyHub, reference: DateLike, points: Vec<(MonthLike, f64)>) -> PyResult<Self> {
        let pts = points.into_iter().map(|(m, b)| (m.0, b)).collect();
        BasisCurve::new(hub.0.clone(), benchmark.0.clone(), reference.0, pts).map(Self).map_err(err)
    }

    #[staticmethod]
    fn from_outrights(hub: &PyFuturesCurve, benchmark: &PyFuturesCurve) -> PyResult<Self> {
        BasisCurve::from_outrights(&hub.0, &benchmark.0).map(Self).map_err(err)
    }

    #[getter]
    fn hub(&self) -> PyHub {
        PyHub(self.0.hub().clone())
    }

    #[getter]
    fn benchmark(&self) -> PyHub {
        PyHub(self.0.benchmark().clone())
    }

    fn points(&self) -> Vec<(PyContractMonth, f64)> {
        self.0.points().iter().map(|(m, b)| (PyContractMonth(*m), *b)).collect()
    }

    fn basis(&self, month: MonthLike) -> PyResult<f64> {
        self.0.basis(month.0).map_err(err)
    }

    /// Outright hub curve = benchmark + basis.
    fn apply_to(&self, benchmark: &PyFuturesCurve) -> PyResult<PyFuturesCurve> {
        self.0.apply_to(&benchmark.0).map(PyFuturesCurve).map_err(err)
    }

    fn __repr__(&self) -> String {
        format!("BasisCurve({} vs {}, {} points)", self.0.hub().code(), self.0.benchmark().code(), self.0.points().len())
    }
}

// ---------------------------------------------------------------------------
// Vol
// ---------------------------------------------------------------------------

/// ATM Black vol per contract month.
#[pyclass(name = "SeasonalVol", module = "ferrumquant", frozen)]
#[derive(Debug, Clone)]
pub struct PySeasonalVol(pub SeasonalVol);

#[pymethods]
impl PySeasonalVol {
    #[new]
    fn new(quotes: Vec<(MonthLike, f64)>) -> PyResult<Self> {
        SeasonalVol::new(quotes.into_iter().map(|(m, v)| VolQuote::new(m.0, v)).collect()).map(Self).map_err(err)
    }

    #[staticmethod]
    fn flat(month: MonthLike, vol: f64) -> PyResult<Self> {
        SeasonalVol::flat(month.0, vol).map(Self).map_err(err)
    }

    fn quotes(&self) -> Vec<(PyContractMonth, f64)> {
        self.0.quotes().iter().map(|q| (PyContractMonth(q.month), q.vol)).collect()
    }

    fn vol(&self, month: MonthLike) -> f64 {
        self.0.vol(month.0)
    }

    fn shifted(&self, bump: f64) -> PyResult<Self> {
        self.0.shifted(bump).map(Self).map_err(err)
    }

    /// Multiply by 12 calendar-month factors (index 0 = January).
    fn with_seasonal_factors(&self, factors: Vec<f64>) -> PyResult<Self> {
        let arr: [f64; 12] = factors.try_into().map_err(|_| FerrumError::new_err("need exactly 12 factors"))?;
        self.0.with_seasonal_factors(arr).map(Self).map_err(err)
    }

    fn __repr__(&self) -> String {
        format!("SeasonalVol({} quotes)", self.0.quotes().len())
    }
}

/// Instantaneous vol `long_run + (short_end - long_run) exp(-alpha tau)`.
#[pyclass(name = "SamuelsonVol", module = "ferrumquant", frozen)]
#[derive(Debug, Clone, Copy)]
pub struct PySamuelsonVol(pub SamuelsonVol);

#[pymethods]
impl PySamuelsonVol {
    #[new]
    fn new(long_run: f64, short_end: f64, alpha: f64) -> PyResult<Self> {
        SamuelsonVol::new(long_run, short_end, alpha).map(Self).map_err(err)
    }

    fn instantaneous(&self, tau: f64) -> f64 {
        self.0.instantaneous(tau)
    }

    fn integrated_variance(&self, t_option: f64, t_contract: f64) -> PyResult<f64> {
        self.0.integrated_variance(t_option, t_contract).map_err(err)
    }

    fn black_vol(&self, t_option: f64, t_contract: f64) -> PyResult<f64> {
        self.0.black_vol(t_option, t_contract).map_err(err)
    }

    fn __repr__(&self) -> String {
        format!("SamuelsonVol(long_run={}, short_end={}, alpha={})", self.0.long_run, self.0.short_end, self.0.alpha)
    }
}

// ---------------------------------------------------------------------------
// FuturesOption
// ---------------------------------------------------------------------------

/// Black-76 European on one contract month.
#[pyclass(name = "FuturesOption", module = "ferrumquant", frozen)]
#[derive(Debug, Clone, Copy)]
pub struct PyFuturesOption(pub FuturesOption);

#[pymethods]
impl PyFuturesOption {
    /// `FuturesOption("call", month, strike, expiry, quantity)`.
    #[new]
    fn new(kind: &str, month: MonthLike, strike: f64, expiry: DateLike, quantity: f64) -> PyResult<Self> {
        FuturesOption::new(parse_kind(kind)?, month.0, strike, expiry.0, quantity).map(Self).map_err(err)
    }

    #[getter]
    fn kind(&self) -> &'static str {
        kind_name(self.0.kind)
    }

    #[getter]
    fn contract(&self) -> PyContractMonth {
        PyContractMonth(self.0.contract)
    }

    #[getter]
    fn strike(&self) -> f64 {
        self.0.strike
    }

    #[getter]
    fn expiry(&self) -> PyDate {
        PyDate(self.0.expiry)
    }

    /// `vol` is a `SeasonalVol` or a flat number. Returns a dict.
    #[pyo3(signature = (curve, vol, discount, day_count = "ACT/365F"))]
    fn value<'py>(&self, py: Python<'py>, curve: &PyFuturesCurve, vol: VolLike, discount: &PyDiscountCurve, day_count: &str) -> PyResult<Bound<'py, PyDict>> {
        let v = self.0.value_with_vol(&curve.0, vol.vol(self.0.contract), &discount.0, parse_dc(day_count)?).map_err(err)?;
        Ok(dict!(py,
            "forward" => v.forward, "vol" => v.vol, "time_to_expiry" => v.time_to_expiry, "discount" => v.discount,
            "forward_price" => v.forward_price, "pv" => v.pv, "delta" => v.delta, "gamma" => v.gamma, "vega" => v.vega,
        ))
    }

    #[pyo3(signature = (curve, forward_premium, day_count = "ACT/365F"))]
    fn implied_vol(&self, curve: &PyFuturesCurve, forward_premium: f64, day_count: &str) -> PyResult<f64> {
        self.0.implied_vol(&curve.0, forward_premium, parse_dc(day_count)?).map_err(err)
    }

    fn __repr__(&self) -> String {
        format!("FuturesOption({} {} K={} exp {})", kind_name(self.0.kind), self.0.contract, self.0.strike, self.0.expiry)
    }
}

// ---------------------------------------------------------------------------
// AsianOption
// ---------------------------------------------------------------------------

/// Average-price option by moment matching.
#[pyclass(name = "AsianOption", module = "ferrumquant", frozen)]
#[derive(Debug, Clone)]
pub struct PyAsianOption(pub AsianOption);

#[pymethods]
impl PyAsianOption {
    /// `AsianOption(kind, strike, [(date, month, observed_or_None), ...], settlement, quantity, cross_contract_corr=1.0)`.
    #[new]
    #[pyo3(signature = (kind, strike, fixings, settlement, quantity, cross_contract_corr = 1.0))]
    fn new(
        kind: &str,
        strike: f64,
        fixings: Vec<(DateLike, MonthLike, Option<f64>)>,
        settlement: DateLike,
        quantity: f64,
        cross_contract_corr: f64,
    ) -> PyResult<Self> {
        let fx = fixings
            .into_iter()
            .map(|(d, m, o)| match o {
                Some(p) => Fixing::observed(d.0, m.0, p),
                None => Fixing::pending(d.0, m.0),
            })
            .collect();
        AsianOption::new(parse_kind(kind)?, strike, fx, settlement.0, quantity)
            .and_then(|a| a.with_cross_contract_corr(cross_contract_corr))
            .map(Self)
            .map_err(err)
    }

    /// Pending daily fixings on one contract over an inclusive date range.
    #[staticmethod]
    fn daily_fixings(start: DateLike, end: DateLike, month: MonthLike) -> Vec<(PyDate, PyContractMonth, Option<f64>)> {
        AsianOption::daily_fixings(start.0, end.0, month.0).into_iter().map(|f| (PyDate(f.date), PyContractMonth(f.contract), f.observed)).collect()
    }

    #[getter]
    fn fixing_count(&self) -> usize {
        self.0.fixings.len()
    }

    #[pyo3(signature = (curve, vol, discount, day_count = "ACT/365F"))]
    fn value<'py>(&self, py: Python<'py>, curve: &PyFuturesCurve, vol: VolLike, discount: &PyDiscountCurve, day_count: &str) -> PyResult<Bound<'py, PyDict>> {
        let v = self.0.value_with(&curve.0, |m| vol.vol(m), &discount.0, parse_dc(day_count)?).map_err(err)?;
        Ok(dict!(py,
            "forward_average" => v.forward_average, "fixed_contribution" => v.fixed_contribution, "effective_strike" => v.effective_strike,
            "effective_vol" => v.effective_vol, "time_to_last_fixing" => v.time_to_last_fixing, "discount" => v.discount,
            "forward_price" => v.forward_price, "pv" => v.pv,
        ))
    }

    fn __repr__(&self) -> String {
        format!("AsianOption({} K={} {} fixings)", kind_name(self.0.kind), self.0.strike, self.0.fixings.len())
    }
}

// ---------------------------------------------------------------------------
// Spread options
// ---------------------------------------------------------------------------

/// Undiscounted two-asset spread option price `max(sign (F1 - F2 - K), 0)`.
#[pyfunction]
#[pyo3(name = "spread_price", signature = (kind, f1, f2, vol1, vol2, rho, t, strike = 0.0, method = "bs"))]
#[allow(clippy::too_many_arguments)]
fn py_spread_price(kind: &str, f1: f64, f2: f64, vol1: f64, vol2: f64, rho: f64, t: f64, strike: f64, method: &str) -> PyResult<f64> {
    spread_price(parse_method(method)?, parse_kind(kind)?, f1, f2, vol1, vol2, rho, t, strike).map_err(err)
}

/// Correlation reproducing an undiscounted spread premium.
#[pyfunction]
#[pyo3(name = "implied_correlation", signature = (kind, f1, f2, vol1, vol2, t, strike, price, method = "bs"))]
#[allow(clippy::too_many_arguments)]
fn py_implied_correlation(kind: &str, f1: f64, f2: f64, vol1: f64, vol2: f64, t: f64, strike: f64, price: f64, method: &str) -> PyResult<f64> {
    implied_correlation(parse_method(method)?, parse_kind(kind)?, f1, f2, vol1, vol2, t, strike, price).map_err(err)
}

/// Spread option on two contract months at possibly different hubs.
#[pyclass(name = "SpreadOption", module = "ferrumquant", frozen)]
#[derive(Debug, Clone, Copy)]
pub struct PySpreadOption(pub SpreadOption);

#[pymethods]
impl PySpreadOption {
    /// `SpreadOption(kind, long_month, short_month, strike, expiry, quantity, long_weight=1, short_weight=1, method="bs")`.
    #[new]
    #[pyo3(signature = (kind, long_month, short_month, strike, expiry, quantity, long_weight = 1.0, short_weight = 1.0, method = "bs"))]
    #[allow(clippy::too_many_arguments)]
    fn new(
        kind: &str,
        long_month: MonthLike,
        short_month: MonthLike,
        strike: f64,
        expiry: DateLike,
        quantity: f64,
        long_weight: f64,
        short_weight: f64,
        method: &str,
    ) -> PyResult<Self> {
        let method = parse_method(method)?;
        SpreadOption::new(parse_kind(kind)?, long_month.0, long_weight, short_month.0, short_weight, strike, expiry.0, quantity)
            .map(|s| Self(s.with_method(method)))
            .map_err(err)
    }

    #[pyo3(signature = (long_curve, short_curve, long_vol, short_vol, rho, discount, day_count = "ACT/365F"))]
    #[allow(clippy::too_many_arguments)]
    fn value<'py>(
        &self,
        py: Python<'py>,
        long_curve: &PyFuturesCurve,
        short_curve: &PyFuturesCurve,
        long_vol: VolLike,
        short_vol: VolLike,
        rho: f64,
        discount: &PyDiscountCurve,
        day_count: &str,
    ) -> PyResult<Bound<'py, PyDict>> {
        let v = self
            .0
            .value(
                &long_curve.0,
                &short_curve.0,
                long_vol.vol(self.0.long_contract),
                short_vol.vol(self.0.short_contract),
                rho,
                &discount.0,
                parse_dc(day_count)?,
            )
            .map_err(err)?;
        Ok(dict!(py,
            "long_forward" => v.long_forward, "short_forward" => v.short_forward, "forward_spread" => v.forward_spread,
            "time_to_expiry" => v.time_to_expiry, "discount" => v.discount, "forward_price" => v.forward_price, "pv" => v.pv,
            "delta_long" => v.delta_long, "delta_short" => v.delta_short, "corr_sensitivity" => v.corr_sensitivity,
        ))
    }

    fn __repr__(&self) -> String {
        format!(
            "SpreadOption({} {}x{} - {}x{} K={})",
            kind_name(self.0.kind),
            self.0.long_weight,
            self.0.long_contract,
            self.0.short_weight,
            self.0.short_contract,
            self.0.strike
        )
    }
}

// ---------------------------------------------------------------------------
// Clewlow-Strickland
// ---------------------------------------------------------------------------

/// One/two-factor forward-curve model.
#[pyclass(name = "ClewlowStrickland", module = "ferrumquant", frozen)]
#[derive(Debug, Clone, Copy)]
pub struct PyClewlowStrickland(pub ClewlowStrickland);

#[pymethods]
impl PyClewlowStrickland {
    /// `ClewlowStrickland(sigma_short, alpha, sigma_long=0.0, rho=0.0)`.
    #[new]
    #[pyo3(signature = (sigma_short, alpha, sigma_long = 0.0, rho = 0.0))]
    fn new(sigma_short: f64, alpha: f64, sigma_long: f64, rho: f64) -> PyResult<Self> {
        ClewlowStrickland::two_factor(sigma_short, alpha, sigma_long, rho).map(Self).map_err(err)
    }

    #[getter]
    fn sigma_short(&self) -> f64 {
        self.0.sigma_short
    }

    #[getter]
    fn alpha(&self) -> f64 {
        self.0.alpha
    }

    #[getter]
    fn sigma_long(&self) -> f64 {
        self.0.sigma_long
    }

    #[getter]
    fn rho(&self) -> f64 {
        self.0.rho
    }

    fn instantaneous_vol(&self, tau: f64) -> f64 {
        self.0.instantaneous_vol(tau)
    }

    fn black_vol(&self, t_option: f64, t_contract: f64) -> PyResult<f64> {
        self.0.black_vol(t_option, t_contract).map_err(err)
    }

    fn correlation(&self, t: f64, t1: f64, t2: f64) -> PyResult<f64> {
        self.0.correlation(t, t1, t2).map_err(err)
    }

    /// `simulate([(t_contract, F0), ...], times, paths=20000, seed=12345, antithetic=True)`.
    #[pyo3(signature = (initial, times, paths = 20_000, seed = 12_345, antithetic = true))]
    fn simulate(&self, initial: Vec<(f64, f64)>, times: Vec<f64>, paths: usize, seed: u64, antithetic: bool) -> PyResult<PyCurvePaths> {
        self.0.simulate(&initial, &times, McConfig { paths, seed, antithetic }).map(PyCurvePaths).map_err(err)
    }

    fn __repr__(&self) -> String {
        format!("ClewlowStrickland(sigma_short={}, alpha={}, sigma_long={}, rho={})", self.0.sigma_short, self.0.alpha, self.0.sigma_long, self.0.rho)
    }
}

/// Simulated forward curves.
#[pyclass(name = "CurvePaths", module = "ferrumquant", frozen)]
#[derive(Debug, Clone)]
pub struct PyCurvePaths(pub CurvePaths);

#[pymethods]
impl PyCurvePaths {
    #[getter]
    fn n_paths(&self) -> usize {
        self.0.n_paths()
    }

    #[getter]
    fn n_contracts(&self) -> usize {
        self.0.n_contracts()
    }

    fn times(&self) -> Vec<f64> {
        self.0.times().to_vec()
    }

    fn maturities(&self) -> Vec<f64> {
        self.0.maturities().to_vec()
    }

    fn forward(&self, path: usize, step: usize, contract: usize) -> PyResult<f64> {
        if path >= self.0.n_paths() || step >= self.0.times().len() || contract >= self.0.n_contracts() {
            return Err(FerrumError::new_err("index out of range"));
        }
        Ok(self.0.forward(path, step, contract))
    }

    /// All contracts on one path at one step.
    fn curve_at(&self, path: usize, step: usize) -> PyResult<Vec<f64>> {
        if path >= self.0.n_paths() || step >= self.0.times().len() {
            return Err(FerrumError::new_err("index out of range"));
        }
        Ok(self.0.curve_at(path, step).to_vec())
    }

    /// Mean and standard error of `F(step, contract)` across paths.
    fn mean_forward(&self, step: usize, contract: usize) -> PyResult<(f64, f64)> {
        if step >= self.0.times().len() || contract >= self.0.n_contracts() {
            return Err(FerrumError::new_err("index out of range"));
        }
        Ok(self.0.estimate(|p| self.0.forward(p, step, contract)))
    }

    /// Mean and standard error of a European payoff on `F(step, contract)`.
    fn european(&self, kind: &str, strike: f64, step: usize, contract: usize) -> PyResult<(f64, f64)> {
        let k = parse_kind(kind)?;
        if step >= self.0.times().len() || contract >= self.0.n_contracts() {
            return Err(FerrumError::new_err("index out of range"));
        }
        Ok(self.0.estimate(|p| (k.sign() * (self.0.forward(p, step, contract) - strike)).max(0.0)))
    }

    fn __repr__(&self) -> String {
        format!("CurvePaths({} paths x {} steps x {} contracts)", self.0.n_paths(), self.0.times().len(), self.0.n_contracts())
    }
}

pub(crate) fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyContractMonth>()?;
    m.add_class::<PyHub>()?;
    m.add_class::<PyFuturesCurve>()?;
    m.add_class::<PyBasisCurve>()?;
    m.add_class::<PySeasonalVol>()?;
    m.add_class::<PySamuelsonVol>()?;
    m.add_class::<PyFuturesOption>()?;
    m.add_class::<PyAsianOption>()?;
    m.add_class::<PySpreadOption>()?;
    m.add_class::<PyClewlowStrickland>()?;
    m.add_class::<PyCurvePaths>()?;
    m.add_function(wrap_pyfunction!(py_spread_price, m)?)?;
    m.add_function(wrap_pyfunction!(py_implied_correlation, m)?)?;
    Ok(())
}
