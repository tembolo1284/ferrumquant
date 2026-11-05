//! Swaption vol cube (SABR smiles on an expiry x tenor grid) and caps/floors.

use pyo3::prelude::*;
use pyo3::types::PyDict;

use fq_core::time::Period;
use fq_rates::capfloor::{CapFloor, CapFloorPricing, CapFloorType, CapletPricing, VolSource};
use fq_rates::swaption::{DefaultSmile, NodeQuotes, SwaptionVolCube, VolQuote};

use crate::curve::{PyDiscountCurve, PyOvernightIndex};
use crate::swap::{PySwaption, PySwaptionPricing};
use crate::time::{DateLike, PeriodLike, PyDate, PyPeriod};
use crate::{FerrumError, err};

// ---------------------------------------------------------------------------
// SwaptionVolCube
// ---------------------------------------------------------------------------

/// Tenor of a swaption's underlying from its unadjusted schedule: whole
/// months between effective and maturity, as years when it divides by 12.
fn swap_tenor(sw: &fq_rates::swaption::Swaption) -> Period {
    let dates = sw.underlying().schedule().dates();
    let (start, end) = (dates[0], dates[dates.len() - 1]);
    let months = (end.year() - start.year()) * 12 + end.month() as i32 - start.month() as i32;
    if months % 12 == 0 { Period::years(months / 12) } else { Period::months(months) }
}

/// Normal-vol cube: ATM bilinear in (expiry, tenor), SABR (beta 0) smile per node.
#[pyclass(name = "SwaptionVolCube", module = "ferrumquant", frozen)]
#[derive(Debug, Clone)]
pub struct PySwaptionVolCube(pub SwaptionVolCube);

#[pymethods]
impl PySwaptionVolCube {
    /// `quotes` is a dict keyed `(expiry, tenor)` (strings) with values
    /// `{"forward": f, "atm_vol": v, "smile": [(strike_offset, vol), ...]}`;
    /// `smile` may be omitted, in which case `default_smile=(rho, nu)` is used.
    #[new]
    #[pyo3(signature = (reference, expiries, tenors, quotes, default_smile = None))]
    fn new(
        reference: DateLike,
        expiries: Vec<PeriodLike>,
        tenors: Vec<PeriodLike>,
        quotes: &Bound<'_, PyDict>,
        default_smile: Option<(f64, f64)>,
    ) -> PyResult<Self> {
        let exp: Vec<_> = expiries.iter().map(|p| p.0).collect();
        let ten: Vec<_> = tenors.iter().map(|p| p.0).collect();
        let mut nodes = Vec::with_capacity(exp.len() * ten.len());
        for e in &exp {
            for t in &ten {
                let key = (e.to_string(), t.to_string());
                let q = quotes.get_item(key.clone())?.ok_or_else(|| FerrumError::new_err(format!("missing vol quote for ({}, {})", key.0, key.1)))?;
                let q = q.downcast::<PyDict>()?;
                let get = |name: &str| -> PyResult<Option<Bound<'_, PyAny>>> { q.get_item(name) };
                let forward: f64 = get("forward")?.ok_or_else(|| FerrumError::new_err(format!("({}, {}): missing 'forward'", key.0, key.1)))?.extract()?;
                let atm_vol: f64 = get("atm_vol")?.ok_or_else(|| FerrumError::new_err(format!("({}, {}): missing 'atm_vol'", key.0, key.1)))?.extract()?;
                let smile: Vec<(f64, f64)> = match get("smile")? {
                    Some(s) => s.extract()?,
                    None => Vec::new(),
                };
                nodes.push(NodeQuotes { forward, atm_vol, smile });
            }
        }
        let default = default_smile.map(|(rho, nu)| DefaultSmile { rho, nu });
        SwaptionVolCube::new(reference.0, &exp, &ten, &nodes, default).map(Self).map_err(err)
    }

    #[getter]
    fn reference_date(&self) -> PyDate {
        PyDate(self.0.reference_date())
    }

    #[getter]
    fn expiries(&self) -> Vec<PyPeriod> {
        self.0.expiries().iter().map(|p| PyPeriod(*p)).collect()
    }

    #[getter]
    fn tenors(&self) -> Vec<PyPeriod> {
        self.0.tenors().iter().map(|p| PyPeriod(*p)).collect()
    }

    /// Calibrated node: forward, atm_vol, alpha, rho, nu, rmse.
    fn node<'py>(&self, py: Python<'py>, expiry_idx: usize, tenor_idx: usize) -> PyResult<Bound<'py, PyDict>> {
        if expiry_idx >= self.0.expiries().len() || tenor_idx >= self.0.tenors().len() {
            return Err(FerrumError::new_err(format!("node ({expiry_idx}, {tenor_idx}) out of range")));
        }
        let n = self.0.node(expiry_idx, tenor_idx);
        let d = PyDict::new(py);
        d.set_item("expiry", PyPeriod(self.0.expiries()[expiry_idx]))?;
        d.set_item("tenor", PyPeriod(self.0.tenors()[tenor_idx]))?;
        d.set_item("forward", n.forward)?;
        d.set_item("atm_vol", n.atm_vol)?;
        d.set_item("alpha", n.sabr.alpha())?;
        d.set_item("rho", n.sabr.rho())?;
        d.set_item("nu", n.sabr.nu())?;
        d.set_item("rmse", n.rmse)?;
        Ok(d)
    }

    /// ATM normal vol at (expiry time in years, tenor in years), flat outside the grid.
    fn atm_vol(&self, expiry_time: f64, tenor_years: f64) -> f64 {
        self.0.atm_vol(expiry_time, tenor_years)
    }

    /// Interpolated SABR (alpha, rho, nu) at (expiry time, tenor years).
    fn sabr_at(&self, expiry_time: f64, tenor_years: f64) -> PyResult<(f64, f64, f64)> {
        self.0.sabr_at(expiry_time, tenor_years).map(|p| (p.alpha(), p.rho(), p.nu())).map_err(err)
    }

    /// Normal vol for a swaption expiring `expiry` on a `tenor` swap at `strike` given its `forward`.
    fn normal_vol(&self, expiry: DateLike, tenor: PeriodLike, forward: f64, strike: f64) -> PyResult<f64> {
        self.0.normal_vol(expiry.0, tenor.0, forward, strike).map_err(err)
    }

    /// Grid nodes and weights used for a query, as `[(expiry_idx, tenor_idx, weight), ...]`.
    fn node_weights(&self, expiry: DateLike, tenor: PeriodLike) -> PyResult<Vec<(usize, usize, f64)>> {
        self.0.node_weights(expiry.0, tenor.0).map_err(err)
    }

    /// Copy of the cube with one node's ATM vol shifted (for vega buckets).
    fn with_atm_bump(&self, expiry_idx: usize, tenor_idx: usize, shift: f64) -> PyResult<Self> {
        self.0.with_atm_bump(expiry_idx, tenor_idx, shift).map(Self).map_err(err)
    }

    /// Price a swaption off the cube: forward from the curves, vol from the smile.
    #[pyo3(signature = (swaption, index, projection, discount = None))]
    fn price(
        &self,
        swaption: &PySwaption,
        index: &PyOvernightIndex,
        projection: &PyDiscountCurve,
        discount: Option<&PyDiscountCurve>,
    ) -> PyResult<PySwaptionPricing> {
        let disc = discount.map_or(&projection.0, |d| &d.0);
        let (forward, _) = swaption.0.forward_and_annuity(&index.0, &projection.0, disc).map_err(err)?;
        let tenor = swap_tenor(&swaption.0);
        let quote: VolQuote = self.0.quote(swaption.0.expiry(), tenor, forward, swaption.0.strike()).map_err(err)?;
        swaption.0.price(&index.0, &projection.0, disc, quote).map(Into::into).map_err(err)
    }

    fn __repr__(&self) -> String {
        format!("SwaptionVolCube(reference={}, expiries={}, tenors={})", self.0.reference_date(), self.0.expiries().len(), self.0.tenors().len())
    }
}

// ---------------------------------------------------------------------------
// CapFloor
// ---------------------------------------------------------------------------

fn parse_cf_kind(s: &str) -> PyResult<CapFloorType> {
    match s.to_ascii_lowercase().as_str() {
        "cap" => Ok(CapFloorType::Cap),
        "floor" => Ok(CapFloorType::Floor),
        _ => Err(FerrumError::new_err(format!("unknown cap/floor type '{s}' (cap | floor)"))),
    }
}

fn vol_source(flat_vol: Option<f64>, vol_strip: Option<Vec<f64>>) -> PyResult<VolSource> {
    match (flat_vol, vol_strip) {
        (Some(v), None) => Ok(VolSource::Flat(v)),
        (None, Some(s)) => Ok(VolSource::Strip(s)),
        _ => Err(FerrumError::new_err("give exactly one of flat_vol or vol_strip")),
    }
}

fn caplet_dict<'py>(py: Python<'py>, c: &CapletPricing) -> PyResult<Bound<'py, PyDict>> {
    let d = PyDict::new(py);
    d.set_item("accrual_start", PyDate(c.accrual_start))?;
    d.set_item("accrual_end", PyDate(c.accrual_end))?;
    d.set_item("payment_date", PyDate(c.payment_date))?;
    d.set_item("fixing_date", PyDate(c.fixing_date))?;
    d.set_item("expiry_time", c.expiry_time)?;
    d.set_item("forward", c.forward)?;
    d.set_item("vol", c.vol)?;
    d.set_item("pv", c.pv)?;
    d.set_item("vega", c.vega)?;
    Ok(d)
}

/// Cap/floor pricing: total pv and vega plus one row per caplet.
#[pyclass(name = "CapFloorPricing", module = "ferrumquant", frozen)]
#[derive(Debug, Clone)]
pub struct PyCapFloorPricing(CapFloorPricing);

#[pymethods]
impl PyCapFloorPricing {
    #[getter]
    fn pv(&self) -> f64 {
        self.0.pv
    }

    /// dPV/dvol in currency per unit normal vol; times 1e-4 for per bp.
    #[getter]
    fn vega(&self) -> f64 {
        self.0.vega
    }

    fn caplets<'py>(&self, py: Python<'py>) -> PyResult<Vec<Bound<'py, PyDict>>> {
        self.0.caplets.iter().map(|c| caplet_dict(py, c)).collect()
    }

    fn __len__(&self) -> usize {
        self.0.caplets.len()
    }

    fn __repr__(&self) -> String {
        format!("CapFloorPricing(pv={:.2}, vega={:.2}, caplets={})", self.0.pv, self.0.vega, self.0.caplets.len())
    }
}

/// Cap or floor on compounded SOFR, one caplet per annual OIS period.
#[pyclass(name = "CapFloor", module = "ferrumquant", frozen)]
#[derive(Debug, Clone)]
pub struct PyCapFloor(pub CapFloor);

#[pymethods]
impl PyCapFloor {
    /// Spot-starting (T+2) USD SOFR cap/floor of a given tenor.
    #[staticmethod]
    fn usd_sofr(trade_date: DateLike, tenor: PeriodLike, kind: &str, notional: f64, strike: f64) -> PyResult<Self> {
        CapFloor::usd_sofr(trade_date.0, tenor.0, parse_cf_kind(kind)?, notional, strike).map(Self).map_err(err)
    }

    #[getter]
    fn kind(&self) -> &'static str {
        match self.0.kind() {
            CapFloorType::Cap => "cap",
            CapFloorType::Floor => "floor",
        }
    }

    #[getter]
    fn notional(&self) -> f64 {
        self.0.notional()
    }

    #[getter]
    fn strike(&self) -> f64 {
        self.0.strike()
    }

    /// Caplet periods: accrual_start, accrual_end, payment_date, fixing_date, year_fraction.
    fn periods<'py>(&self, py: Python<'py>) -> PyResult<Vec<Bound<'py, PyDict>>> {
        self.0
            .periods()
            .iter()
            .map(|p| {
                let d = PyDict::new(py);
                d.set_item("accrual_start", PyDate(p.accrual_start))?;
                d.set_item("accrual_end", PyDate(p.accrual_end))?;
                d.set_item("payment_date", PyDate(p.payment_date))?;
                d.set_item("fixing_date", PyDate(self.0.fixing_date(p).map_err(err)?))?;
                d.set_item("year_fraction", p.fixed_year_fraction)?;
                Ok(d)
            })
            .collect()
    }

    /// Price with a flat normal vol or one vol per period.
    #[pyo3(signature = (index, projection, discount = None, *, flat_vol = None, vol_strip = None))]
    fn price(
        &self,
        index: &PyOvernightIndex,
        projection: &PyDiscountCurve,
        discount: Option<&PyDiscountCurve>,
        flat_vol: Option<f64>,
        vol_strip: Option<Vec<f64>>,
    ) -> PyResult<PyCapFloorPricing> {
        let disc = discount.map_or(&projection.0, |d| &d.0);
        self.0.price(&index.0, &projection.0, disc, &vol_source(flat_vol, vol_strip)?).map(PyCapFloorPricing).map_err(err)
    }

    #[pyo3(signature = (index, curve, *, flat_vol = None, vol_strip = None))]
    fn npv(&self, index: &PyOvernightIndex, curve: &PyDiscountCurve, flat_vol: Option<f64>, vol_strip: Option<Vec<f64>>) -> PyResult<f64> {
        self.0.npv(&index.0, &curve.0, &vol_source(flat_vol, vol_strip)?).map_err(err)
    }

    /// Flat normal vol that reproduces `pv`.
    #[pyo3(signature = (index, projection, pv, discount = None))]
    fn implied_flat_vol(&self, index: &PyOvernightIndex, projection: &PyDiscountCurve, pv: f64, discount: Option<&PyDiscountCurve>) -> PyResult<f64> {
        let disc = discount.map_or(&projection.0, |d| &d.0);
        self.0.implied_flat_vol(&index.0, &projection.0, disc, pv).map_err(err)
    }

    fn __repr__(&self) -> String {
        format!("CapFloor({} strike {:.4}%, notional={}, periods={})", self.kind(), self.0.strike() * 100.0, self.0.notional(), self.0.periods().len())
    }
}

pub(crate) fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PySwaptionVolCube>()?;
    m.add_class::<PyCapFloorPricing>()?;
    m.add_class::<PyCapFloor>()?;
    Ok(())
}
