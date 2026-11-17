//! Term-structure models: Hull-White one-factor.
//!
//! `HullWhite` wraps a discount curve with `(a, sigma)`; it prices ZCB
//! options, compounded-SOFR caplets, European swaptions in closed form and
//! Bermudans on the trinomial tree, simulates short-rate paths, and
//! `calibrate_hull_white` fits it to swaption normal vols. Option kinds are
//! `"call"` / `"put"`, caplet kinds `"cap"` / `"floor"`, calibration modes
//! `"sigma"` / `"both"`.

use pyo3::prelude::*;
use pyo3::types::PyDict;

use fq_models::{
    BermudanSwaption, CalibrationInstrument, CalibrationMode, CalibrationResult, CapletKind, HullWhite, HwSwaptionPricing, McConfig, ShortRatePaths, calibrate,
};
use fq_rates::swaption::SwaptionType;

use crate::commod::parse_kind as parse_option_kind;
use crate::curve::PyDiscountCurve;
use crate::swap::{PyOisSwap, PySwaption};
use crate::time::{DateLike, PeriodLike, PyDate};
use crate::{FerrumError, dict, err};

fn parse_caplet_kind(s: &str) -> PyResult<CapletKind> {
    match s.to_ascii_lowercase().as_str() {
        "cap" | "caplet" | "call" => Ok(CapletKind::Caplet),
        "floor" | "floorlet" | "put" => Ok(CapletKind::Floorlet),
        _ => Err(FerrumError::new_err(format!("unknown caplet kind '{s}' (cap | floor)"))),
    }
}

fn parse_swaption_kind(s: &str) -> PyResult<SwaptionType> {
    match s.to_ascii_lowercase().as_str() {
        "payer" | "pay" | "call" => Ok(SwaptionType::Payer),
        "receiver" | "receive" | "rec" | "put" => Ok(SwaptionType::Receiver),
        _ => Err(FerrumError::new_err(format!("unknown swaption type '{s}' (payer | receiver)"))),
    }
}

fn swaption_kind_name(k: SwaptionType) -> &'static str {
    match k {
        SwaptionType::Payer => "payer",
        SwaptionType::Receiver => "receiver",
    }
}

fn parse_mode(s: &str) -> PyResult<CalibrationMode> {
    match s.to_ascii_lowercase().as_str() {
        "sigma" | "sigma_only" | "vol" => Ok(CalibrationMode::SigmaOnly),
        "both" | "a_sigma" | "mean_reversion_and_sigma" => Ok(CalibrationMode::MeanReversionAndSigma),
        _ => Err(FerrumError::new_err(format!("unknown calibration mode '{s}' (sigma | both)"))),
    }
}

// ---------------------------------------------------------------------------
// HwSwaptionPricing
// ---------------------------------------------------------------------------

/// Hull-White swaption price in currency units with the implied normal vol.
#[pyclass(name = "HwSwaptionPricing", module = "ferrumquant", frozen, get_all)]
#[derive(Debug, Clone, Copy)]
pub struct PyHwSwaptionPricing {
    pv: f64,
    forward_rate: f64,
    strike: f64,
    annuity: f64,
    expiry_time: f64,
    /// Bachelier vol reproducing `pv`, or None below intrinsic.
    implied_normal_vol: Option<f64>,
    /// Exercise boundary in factor space, or None when the option is the forward or nothing.
    exercise_boundary: Option<f64>,
}

impl From<HwSwaptionPricing> for PyHwSwaptionPricing {
    fn from(p: HwSwaptionPricing) -> Self {
        Self {
            pv: p.pv,
            forward_rate: p.forward_rate,
            strike: p.strike,
            annuity: p.annuity,
            expiry_time: p.expiry_time,
            implied_normal_vol: p.implied_normal_vol,
            exercise_boundary: p.exercise_boundary,
        }
    }
}

#[pymethods]
impl PyHwSwaptionPricing {
    fn to_dict<'py>(self_: PyRef<'py, Self>, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        Ok(dict!(py,
            "pv" => self_.pv,
            "forward_rate" => self_.forward_rate,
            "strike" => self_.strike,
            "annuity" => self_.annuity,
            "expiry_time" => self_.expiry_time,
            "implied_normal_vol" => self_.implied_normal_vol,
            "exercise_boundary" => self_.exercise_boundary,
        ))
    }

    fn __repr__(&self) -> String {
        let vol = self.implied_normal_vol.map_or("n/a".to_string(), |v| format!("{:.2}bp", v * 1e4));
        format!("HwSwaptionPricing(pv={:.2}, forward={:.4}%, strike={:.4}%, implied_normal_vol={vol})", self.pv, self.forward_rate * 100.0, self.strike * 100.0)
    }
}

// ---------------------------------------------------------------------------
// BermudanSwaption
// ---------------------------------------------------------------------------

/// Right to enter the remaining periods of an OIS on any of several exercise dates.
#[pyclass(name = "BermudanSwaption", module = "ferrumquant", frozen)]
#[derive(Debug, Clone)]
pub struct PyBermudanSwaption(pub BermudanSwaption);

#[pymethods]
impl PyBermudanSwaption {
    /// `BermudanSwaption(exercise_dates, kind, underlying)`; on exercise at `d`
    /// the holder enters the periods whose accrual starts on or after `d`.
    #[new]
    fn new(exercise_dates: Vec<DateLike>, kind: &str, underlying: &PyOisSwap) -> PyResult<Self> {
        BermudanSwaption::new(exercise_dates.into_iter().map(|d| d.0).collect(), parse_swaption_kind(kind)?, underlying.0.clone()).map(Self).map_err(err)
    }

    /// Co-terminal Bermudan on a spot-starting USD SOFR swap: first exercise
    /// on `first_exercise`, then two business days before every later period.
    #[staticmethod]
    fn usd_sofr_coterminal(first_exercise: DateLike, tenor: PeriodLike, kind: &str, notional: f64, strike: f64) -> PyResult<Self> {
        BermudanSwaption::usd_sofr_coterminal(first_exercise.0, tenor.0, parse_swaption_kind(kind)?, notional, strike).map(Self).map_err(err)
    }

    #[getter]
    fn exercise_dates(&self) -> Vec<PyDate> {
        self.0.exercise_dates().iter().map(|&d| PyDate(d)).collect()
    }

    #[getter]
    fn kind(&self) -> &'static str {
        swaption_kind_name(self.0.kind())
    }

    #[getter]
    fn strike(&self) -> f64 {
        self.0.strike()
    }

    #[getter]
    fn underlying(&self) -> PyOisSwap {
        PyOisSwap(self.0.underlying().clone())
    }

    fn __repr__(&self) -> String {
        let ex = self.0.exercise_dates();
        format!(
            "BermudanSwaption({} {} exercises {}..{}, strike={:.4}%, maturity={})",
            swaption_kind_name(self.0.kind()),
            ex.len(),
            ex[0],
            ex[ex.len() - 1],
            self.0.strike() * 100.0,
            self.0.underlying().maturity_date()
        )
    }
}

// ---------------------------------------------------------------------------
// ShortRatePaths
// ---------------------------------------------------------------------------

/// Simulated Hull-White paths: the factor, its integral, discount factors and bonds.
#[pyclass(name = "ShortRatePaths", module = "ferrumquant", frozen)]
#[derive(Debug, Clone)]
pub struct PyShortRatePaths(pub ShortRatePaths);

impl PyShortRatePaths {
    fn check(&self, path: usize, step: usize) -> PyResult<()> {
        if path >= self.0.n_paths() || step >= self.0.n_steps() {
            return Err(FerrumError::new_err(format!("index out of range: path {path} of {}, step {step} of {}", self.0.n_paths(), self.0.n_steps())));
        }
        Ok(())
    }

    fn check_step(&self, step: usize) -> PyResult<()> {
        if step >= self.0.n_steps() {
            return Err(FerrumError::new_err(format!("step {step} out of range ({} steps)", self.0.n_steps())));
        }
        Ok(())
    }
}

#[pymethods]
impl PyShortRatePaths {
    #[getter]
    fn n_paths(&self) -> usize {
        self.0.n_paths()
    }

    #[getter]
    fn n_steps(&self) -> usize {
        self.0.n_steps()
    }

    fn times(&self) -> Vec<f64> {
        self.0.times().to_vec()
    }

    /// The zero-mean factor `x` at (path, step).
    fn factor(&self, path: usize, step: usize) -> PyResult<f64> {
        self.check(path, step)?;
        Ok(self.0.factor(path, step))
    }

    /// `int_0^t x` at (path, step).
    fn integral(&self, path: usize, step: usize) -> PyResult<f64> {
        self.check(path, step)?;
        Ok(self.0.integral(path, step))
    }

    /// Short rate `x + alpha(t)` at (path, step).
    fn short_rate(&self, path: usize, step: usize) -> PyResult<f64> {
        self.check(path, step)?;
        Ok(self.0.short_rate(path, step))
    }

    /// Stochastic discount factor from today to the step's time.
    fn discount_to(&self, path: usize, step: usize) -> PyResult<f64> {
        self.check(path, step)?;
        Ok(self.0.discount_to(path, step))
    }

    /// Zero-coupon bond `P(t_step, T)` on the path (`T` in years).
    fn bond(&self, path: usize, step: usize, maturity: f64) -> PyResult<f64> {
        self.check(path, step)?;
        self.0.bond(path, step, maturity).map_err(err)
    }

    /// Factor on every path at one step (column for pandas/numpy).
    fn factors(&self, step: usize) -> PyResult<Vec<f64>> {
        self.check_step(step)?;
        Ok((0..self.0.n_paths()).map(|p| self.0.factor(p, step)).collect())
    }

    /// Short rate on every path at one step.
    fn short_rates(&self, step: usize) -> PyResult<Vec<f64>> {
        self.check_step(step)?;
        Ok((0..self.0.n_paths()).map(|p| self.0.short_rate(p, step)).collect())
    }

    /// Discount factor on every path at one step.
    fn discounts(&self, step: usize) -> PyResult<Vec<f64>> {
        self.check_step(step)?;
        Ok((0..self.0.n_paths()).map(|p| self.0.discount_to(p, step)).collect())
    }

    /// Mean and standard error of `payoff(path)` over the paths; the Python
    /// callable receives the path index. Exceptions propagate.
    fn estimate(&self, payoff: &Bound<'_, PyAny>) -> PyResult<(f64, f64)> {
        let mut failure: Option<PyErr> = None;
        let (mean, se) = self.0.estimate(|p| {
            if failure.is_some() {
                return 0.0;
            }
            match payoff.call1((p,)).and_then(|v| v.extract::<f64>()) {
                Ok(v) => v,
                Err(e) => {
                    failure = Some(e);
                    0.0
                }
            }
        });
        match failure {
            Some(e) => Err(e),
            None => Ok((mean, se)),
        }
    }

    /// Mean and standard error of the discounted bond `D(t) P(t, T)` at a
    /// step; equals `P(0, T)` in expectation.
    fn discounted_bond(&self, step: usize, maturity: f64) -> PyResult<(f64, f64)> {
        self.check_step(step)?;
        let t = self.0.times()[step];
        if maturity < t {
            return Err(FerrumError::new_err(format!("maturity {maturity} is before the step time {t}")));
        }
        Ok(self.0.estimate(|p| self.0.discount_to(p, step) * self.0.model().bond(t, maturity, self.0.factor(p, step)).unwrap_or(f64::NAN)))
    }

    fn __repr__(&self) -> String {
        format!("ShortRatePaths(paths={}, steps={}, horizon={:.3})", self.0.n_paths(), self.0.n_steps(), self.0.times().last().copied().unwrap_or(0.0))
    }
}

// ---------------------------------------------------------------------------
// HullWhite
// ---------------------------------------------------------------------------

/// Hull-White one-factor model `dr = (theta(t) - a r) dt + sigma dW` fitted to a curve.
#[pyclass(name = "HullWhite", module = "ferrumquant", frozen)]
#[derive(Debug, Clone)]
pub struct PyHullWhite(pub HullWhite);

#[pymethods]
impl PyHullWhite {
    /// `HullWhite(curve, a, sigma)` with `a` the mean reversion and `sigma` the
    /// absolute short-rate vol (0.01 for 100bp).
    #[new]
    fn new(curve: &PyDiscountCurve, a: f64, sigma: f64) -> PyResult<Self> {
        HullWhite::new(curve.0.clone(), a, sigma).map(Self).map_err(err)
    }

    #[getter]
    fn curve(&self) -> PyDiscountCurve {
        PyDiscountCurve(self.0.curve().clone())
    }

    #[getter]
    fn mean_reversion(&self) -> f64 {
        self.0.mean_reversion()
    }

    #[getter]
    fn sigma(&self) -> f64 {
        self.0.sigma()
    }

    /// Same curve, new parameters.
    fn with_params(&self, a: f64, sigma: f64) -> PyResult<Self> {
        self.0.with_params(a, sigma).map(Self).map_err(err)
    }

    /// ACT/365F years from the curve's reference date.
    fn time(&self, d: DateLike) -> f64 {
        self.0.time(d.0)
    }

    /// `B(t, T) = (1 - exp(-a (T - t))) / a`.
    fn b(&self, t: f64, maturity: f64) -> f64 {
        self.0.b(t, maturity)
    }

    /// Variance of the short rate at `t`.
    fn variance(&self, t: f64) -> f64 {
        self.0.variance(t)
    }

    /// Instantaneous forward `f(0, t)` from the curve.
    fn instantaneous_forward(&self, t: f64) -> f64 {
        self.0.instantaneous_forward(t)
    }

    /// Risk-neutral mean of the short rate at `t`.
    fn short_rate_mean(&self, t: f64) -> f64 {
        self.0.short_rate_mean(t)
    }

    /// Zero-coupon bond `P(t, T)` given the factor `x(t) = x` (x = 0 is the curve at t = 0).
    #[pyo3(signature = (t, maturity, x = 0.0))]
    fn bond(&self, t: f64, maturity: f64, x: f64) -> PyResult<f64> {
        self.0.bond(t, maturity, x).map_err(err)
    }

    /// Option (`"call"` / `"put"`) expiring at `t` on the ZCB maturing at `maturity`, strike per unit face.
    fn zcb_option(&self, kind: &str, t: f64, maturity: f64, strike: f64) -> PyResult<f64> {
        self.0.zcb_option(parse_option_kind(kind)?, t, maturity, strike).map_err(err)
    }

    /// Caplet or floorlet (`"cap"` / `"floor"`) on the daily-compounded rate
    /// over `[start, end]` (years), year fraction `tau`, paid at `pay`, per unit notional.
    #[pyo3(signature = (kind, start, end, strike, tau, pay = None))]
    fn compounded_caplet(&self, kind: &str, start: f64, end: f64, strike: f64, tau: f64, pay: Option<f64>) -> PyResult<f64> {
        self.0.compounded_caplet(parse_caplet_kind(kind)?, start, end, pay.unwrap_or(end), strike, tau).map_err(err)
    }

    /// Same payoff on the forward-looking term rate set at `start`.
    #[pyo3(signature = (kind, start, end, strike, tau, pay = None))]
    fn term_caplet(&self, kind: &str, start: f64, end: f64, strike: f64, tau: f64, pay: Option<f64>) -> PyResult<f64> {
        self.0.term_caplet(parse_caplet_kind(kind)?, start, end, pay.unwrap_or(end), strike, tau).map_err(err)
    }

    /// Dated `compounded_caplet`.
    #[pyo3(signature = (kind, start, end, strike, tau, pay = None))]
    fn compounded_caplet_on(&self, kind: &str, start: DateLike, end: DateLike, strike: f64, tau: f64, pay: Option<DateLike>) -> PyResult<f64> {
        self.0.compounded_caplet_on(parse_caplet_kind(kind)?, start.0, end.0, pay.map_or(end.0, |p| p.0), strike, tau).map_err(err)
    }

    /// (forward swap rate, annuity) off the model's curve for the periods of
    /// `swap` starting on or after `from_date`.
    #[pyo3(signature = (swap, from_date = None))]
    fn forward_and_annuity(&self, swap: &PyOisSwap, from_date: Option<DateLike>) -> (f64, f64) {
        self.0.forward_and_annuity(&swap.0, from_date.map_or(self.0.curve().reference_date(), |d| d.0))
    }

    /// European swaption in closed form.
    fn european_swaption(&self, swaption: &PySwaption) -> PyResult<PyHwSwaptionPricing> {
        self.0.european_swaption(&swaption.0).map(Into::into).map_err(err)
    }

    /// Bermudan swaption PV on the trinomial tree.
    #[pyo3(signature = (swaption, steps_per_year = 100))]
    fn bermudan_swaption(&self, swaption: &PyBermudanSwaption, steps_per_year: usize) -> PyResult<f64> {
        self.0.bermudan_swaption(&swaption.0, steps_per_year).map_err(err)
    }

    /// Exact simulation of the factor and bank account on `times` (years).
    #[pyo3(signature = (times, paths = 20_000, seed = 12345, antithetic = true))]
    fn simulate(&self, times: Vec<f64>, paths: usize, seed: u64, antithetic: bool) -> PyResult<PyShortRatePaths> {
        self.0.simulate(&times, McConfig { paths, seed, antithetic }).map(PyShortRatePaths).map_err(err)
    }

    fn __repr__(&self) -> String {
        format!("HullWhite(ref={}, a={}, sigma={})", self.0.curve().reference_date(), self.0.mean_reversion(), self.0.sigma())
    }
}

// ---------------------------------------------------------------------------
// Calibration
// ---------------------------------------------------------------------------

/// Result of `calibrate_hull_white`.
#[pyclass(name = "HwCalibration", module = "ferrumquant", frozen)]
#[derive(Debug, Clone)]
pub struct PyHwCalibration(CalibrationResult);

#[pymethods]
impl PyHwCalibration {
    #[getter]
    fn model(&self) -> PyHullWhite {
        PyHullWhite(self.0.model.clone())
    }

    /// Root-mean-square vol error in rate units.
    #[getter]
    fn rmse(&self) -> f64 {
        self.0.rmse
    }

    #[getter]
    fn iterations(&self) -> usize {
        self.0.iterations
    }

    #[getter]
    fn converged(&self) -> bool {
        self.0.converged
    }

    /// One dict per instrument: market_vol, model_vol, market_pv, model_pv.
    fn fits<'py>(&self, py: Python<'py>) -> PyResult<Vec<Bound<'py, PyDict>>> {
        self.0
            .fits
            .iter()
            .map(|f| Ok(dict!(py, "market_vol" => f.market_vol, "model_vol" => f.model_vol, "market_pv" => f.market_pv, "model_pv" => f.model_pv)))
            .collect()
    }

    fn __repr__(&self) -> String {
        format!(
            "HwCalibration(a={:.5}, sigma={:.6}, rmse={:.3}bp, instruments={}, converged={})",
            self.0.model.mean_reversion(),
            self.0.model.sigma(),
            self.0.rmse * 1e4,
            self.0.fits.len(),
            self.0.converged
        )
    }
}

/// Fit Hull-White to `[(Swaption, normal_vol), ...]`. `mode` is `"sigma"`
/// (mean reversion fixed at `a`) or `"both"`; `(a, sigma)` seed the search.
#[pyfunction]
#[pyo3(signature = (curve, instruments, mode = "sigma", a = 0.05, sigma = 0.01))]
fn calibrate_hull_white(curve: &PyDiscountCurve, instruments: Vec<(PyRef<'_, PySwaption>, f64)>, mode: &str, a: f64, sigma: f64) -> PyResult<PyHwCalibration> {
    let insts: Vec<CalibrationInstrument> = instruments.iter().map(|(s, v)| CalibrationInstrument { swaption: s.0.clone(), normal_vol: *v }).collect();
    calibrate(&curve.0, &insts, parse_mode(mode)?, (a, sigma)).map(PyHwCalibration).map_err(err)
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyHullWhite>()?;
    m.add_class::<PyHwSwaptionPricing>()?;
    m.add_class::<PyBermudanSwaption>()?;
    m.add_class::<PyShortRatePaths>()?;
    m.add_class::<PyHwCalibration>()?;
    m.add_function(wrap_pyfunction!(calibrate_hull_white, m)?)?;
    Ok(())
}
