//! Calibration of Hull-White to European swaption quotes.
//!
//! Each instrument is a swaption with a market normal (Bachelier) vol. The
//! objective is the sum of squared price errors scaled by the market vega
//! of each instrument, which is the vol error to first order but never
//! fails on a price below intrinsic. `SigmaOnly` keeps the mean reversion
//! fixed (the usual desk choice: `a` from a view or a term-structure fit,
//! `sigma` from the quotes); `MeanReversionAndSigma` fits both, which needs
//! quotes spread across expiries and tenors to be identified.

use fq_core::math::{OptionType, bachelier, nelder_mead};
use fq_rates::curve::DiscountCurve;
use fq_rates::swaption::{Swaption, SwaptionType};

use crate::error::ModelError;
use crate::hull_white::HullWhite;

#[derive(Debug, Clone, PartialEq)]
pub struct CalibrationInstrument {
    pub swaption: Swaption,
    /// Market normal vol in rate units (0.0080 for 80bp).
    pub normal_vol: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CalibrationMode {
    SigmaOnly,
    MeanReversionAndSigma,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InstrumentFit {
    pub market_vol: f64,
    /// Model-implied normal vol, if the model price is above intrinsic.
    pub model_vol: Option<f64>,
    pub market_pv: f64,
    pub model_pv: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CalibrationResult {
    pub model: HullWhite,
    /// Root-mean-square vol error in rate units (vega-scaled price errors).
    pub rmse: f64,
    pub iterations: usize,
    pub converged: bool,
    pub fits: Vec<InstrumentFit>,
}

struct Target {
    pv: f64,
    vega: f64,
}

fn targets(curve: &DiscountCurve, instruments: &[CalibrationInstrument]) -> Result<Vec<Target>, ModelError> {
    // Any positive parameters give the forward and annuity; they do not depend on them.
    let probe = HullWhite::new(curve.clone(), 0.01, 0.01)?;
    let today = curve.reference_date();
    instruments
        .iter()
        .enumerate()
        .map(|(index, inst)| {
            if !(inst.normal_vol.is_finite() && inst.normal_vol > 0.0) {
                return Err(ModelError::BadInstrument { index, reason: format!("normal vol {} must be positive", inst.normal_vol) });
            }
            if inst.swaption.expiry() <= today {
                return Err(ModelError::BadInstrument { index, reason: format!("expiry {} is not after {}", inst.swaption.expiry(), today) });
            }
            let t = curve.time(inst.swaption.expiry());
            let (f, annuity) = probe.forward_and_annuity(inst.swaption.underlying(), inst.swaption.expiry());
            let kind = match inst.swaption.kind() {
                SwaptionType::Payer => OptionType::Call,
                SwaptionType::Receiver => OptionType::Put,
            };
            let b = bachelier(kind, f, inst.swaption.strike(), inst.normal_vol, t)?;
            Ok(Target { pv: annuity * b.price, vega: annuity * b.vega })
        })
        .collect()
}

/// Fits the model to `instruments`. `initial` is `(a, sigma)`; in
/// `SigmaOnly` mode `a` stays at its initial value.
pub fn calibrate(
    curve: &DiscountCurve,
    instruments: &[CalibrationInstrument],
    mode: CalibrationMode,
    initial: (f64, f64),
) -> Result<CalibrationResult, ModelError> {
    if instruments.is_empty() {
        return Err(ModelError::NoInstruments);
    }
    let (a0, s0) = initial;
    HullWhite::new(curve.clone(), a0, s0)?; // validates the seed
    let targets = targets(curve, instruments)?;

    let objective = |a: f64, sigma: f64| -> f64 {
        let model = match HullWhite::new(curve.clone(), a, sigma) {
            Ok(m) => m,
            Err(_) => return f64::INFINITY,
        };
        let mut sse = 0.0;
        for (inst, target) in instruments.iter().zip(&targets) {
            match model.european_swaption(&inst.swaption) {
                Ok(p) => {
                    let err = (p.pv - target.pv) / target.vega;
                    if !err.is_finite() {
                        return f64::INFINITY;
                    }
                    sse += err * err;
                }
                Err(_) => return f64::INFINITY,
            }
        }
        sse
    };

    // Log coordinates keep both parameters positive.
    let (start, step): (Vec<f64>, Vec<f64>) = match mode {
        CalibrationMode::SigmaOnly => (vec![s0.ln()], vec![0.5]),
        CalibrationMode::MeanReversionAndSigma => (vec![a0.ln(), s0.ln()], vec![0.5, 0.5]),
    };
    let f = |u: &[f64]| match mode {
        CalibrationMode::SigmaOnly => objective(a0, u[0].exp()),
        CalibrationMode::MeanReversionAndSigma => objective(u[0].exp(), u[1].exp()),
    };
    let m = nelder_mead(f, &start, &step, 1000, 1e-14);
    let (a, sigma) = match mode {
        CalibrationMode::SigmaOnly => (a0, m.x[0].exp()),
        CalibrationMode::MeanReversionAndSigma => (m.x[0].exp(), m.x[1].exp()),
    };
    let rmse = (m.value / instruments.len() as f64).sqrt();
    if !rmse.is_finite() {
        return Err(ModelError::NoConvergence { rmse });
    }
    let model = HullWhite::new(curve.clone(), a, sigma)?;
    let fits = instruments
        .iter()
        .zip(&targets)
        .map(|(inst, target)| {
            let p = model.european_swaption(&inst.swaption)?;
            Ok(InstrumentFit { market_vol: inst.normal_vol, model_vol: p.implied_normal_vol, market_pv: target.pv, model_pv: p.pv })
        })
        .collect::<Result<Vec<_>, ModelError>>()?;
    Ok(CalibrationResult { model, rmse, iterations: m.iterations, converged: m.converged(), fits })
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_abs_diff_eq;
    use fq_core::time::Date;
    use fq_rates::swap::Tenor;

    type R = Result<(), ModelError>;

    fn d(y: i32, m: u32, dd: u32) -> Date {
        Date::from_ymd(y, m, dd).expect("valid test date")
    }

    fn curve() -> DiscountCurve {
        let today = d(2026, 10, 13);
        let nodes = [(d(2027, 10, 13), 0.035), (d(2029, 10, 13), 0.038), (d(2031, 10, 13), 0.040), (d(2036, 10, 13), 0.042), (d(2056, 10, 13), 0.043)];
        DiscountCurve::from_zero_rates(today, &nodes, Default::default()).expect("curve")
    }

    fn atm(expiry_years: i32, tenor_years: i32) -> Swaption {
        let expiry = d(2026 + expiry_years, 10, 13);
        let probe = HullWhite::new(curve(), 0.05, 0.01).expect("model");
        let tmp = Swaption::usd_sofr(expiry, Tenor::years(tenor_years), SwaptionType::Payer, 1e6, 0.04).expect("swaption");
        let (f, _) = probe.forward_and_annuity(tmp.underlying(), expiry);
        Swaption::usd_sofr(expiry, Tenor::years(tenor_years), SwaptionType::Payer, 1e6, f).expect("swaption")
    }

    fn quotes_from(a: f64, sigma: f64) -> Vec<CalibrationInstrument> {
        let truth = HullWhite::new(curve(), a, sigma).expect("model");
        [(1, 5), (2, 5), (5, 5), (1, 10), (5, 10), (10, 10)]
            .into_iter()
            .map(|(e, t)| {
                let sw = atm(e, t);
                let vol = truth.european_swaption(&sw).expect("price").implied_normal_vol.expect("vol");
                CalibrationInstrument { swaption: sw, normal_vol: vol }
            })
            .collect()
    }

    #[test]
    fn sigma_only_recovers_the_generating_vol() -> R {
        let quotes = quotes_from(0.05, 0.011);
        let r = calibrate(&curve(), &quotes, CalibrationMode::SigmaOnly, (0.05, 0.02))?;
        assert!(r.converged);
        assert_abs_diff_eq!(r.model.sigma(), 0.011, epsilon = 1e-7);
        assert_abs_diff_eq!(r.model.mean_reversion(), 0.05, epsilon = 0.0);
        assert!(r.rmse < 1e-7, "{}", r.rmse);
        for f in &r.fits {
            assert_abs_diff_eq!(f.model_vol.expect("vol"), f.market_vol, epsilon = 1e-7);
            assert_abs_diff_eq!(f.model_pv, f.market_pv, epsilon = 1e-5 * f.market_pv);
        }
        Ok(())
    }

    #[test]
    fn joint_fit_recovers_both_parameters() -> R {
        let quotes = quotes_from(0.08, 0.012);
        let r = calibrate(&curve(), &quotes, CalibrationMode::MeanReversionAndSigma, (0.03, 0.008))?;
        assert!(r.converged);
        assert_abs_diff_eq!(r.model.mean_reversion(), 0.08, epsilon = 1e-4);
        assert_abs_diff_eq!(r.model.sigma(), 0.012, epsilon = 1e-6);
        assert!(r.rmse < 1e-6, "{}", r.rmse);
        Ok(())
    }

    #[test]
    fn flat_vol_surface_is_a_best_fit_not_an_exact_one() -> R {
        // Market quotes that no single (a, sigma) reproduces: same vol everywhere.
        let quotes: Vec<_> =
            [(1, 5), (5, 5), (1, 10), (10, 10)].into_iter().map(|(e, t)| CalibrationInstrument { swaption: atm(e, t), normal_vol: 0.0085 }).collect();
        let r = calibrate(&curve(), &quotes, CalibrationMode::SigmaOnly, (0.03, 0.01))?;
        assert!(r.rmse > 1e-5 && r.rmse < 0.002, "{}", r.rmse);
        let avg: f64 = r.fits.iter().map(|f| f.model_vol.expect("vol")).sum::<f64>() / 4.0;
        assert!((avg - 0.0085).abs() < 0.001, "{avg}");
        Ok(())
    }

    #[test]
    fn invalid_inputs() {
        assert!(matches!(calibrate(&curve(), &[], CalibrationMode::SigmaOnly, (0.05, 0.01)), Err(ModelError::NoInstruments)));
        let mut q = quotes_from(0.05, 0.01);
        assert!(calibrate(&curve(), &q, CalibrationMode::SigmaOnly, (-0.05, 0.01)).is_err());
        q[0].normal_vol = -0.01;
        assert!(matches!(calibrate(&curve(), &q, CalibrationMode::SigmaOnly, (0.05, 0.01)), Err(ModelError::BadInstrument { index: 0, .. })));
        let expired = Swaption::usd_sofr(d(2026, 10, 13), Tenor::years(5), SwaptionType::Payer, 1e6, 0.04).expect("swaption");
        let q = vec![CalibrationInstrument { swaption: expired, normal_vol: 0.008 }];
        assert!(matches!(calibrate(&curve(), &q, CalibrationMode::SigmaOnly, (0.05, 0.01)), Err(ModelError::BadInstrument { index: 0, .. })));
    }
}
