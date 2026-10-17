//! SABR stochastic-volatility smile: Hagan et al. (2002) asymptotic
//! implied-vol formulas and a least-squares calibration.
//!
//! The model is `dF = α F^β dW₁`, `dα = ν α dW₂`, `⟨dW₁ dW₂⟩ = ρ dt`. Two
//! output conventions are supported:
//!
//! - **Normal (Bachelier) vol**: Hagan's normal-vol expansion. With `β = 0`
//!   this is the exact "normal SABR" the USD swaption market quotes in, and
//!   it handles negative forwards and strikes directly.
//! - **Lognormal (Black) vol** on a shifted forward `F + s`, `K + s`.
//!
//! Calibration fixes `β` and fits `(α, ρ, ν)` to a strike strip by
//! Nelder–Mead on a bounded parameterisation, which is robust for the 3–7
//! point strips swaption desks quote.

use thiserror::Error;

#[derive(Debug, Error, Clone, PartialEq)]
pub enum SabrError {
    #[error("invalid SABR parameters: {0}")]
    Invalid(String),
    #[error("calibration needs at least {need} strikes, got {got}")]
    TooFewStrikes { need: usize, got: usize },
    #[error("calibration did not converge (rmse {rmse})")]
    NoConvergence { rmse: f64 },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SabrParams {
    pub alpha: f64,
    pub beta: f64,
    pub rho: f64,
    pub nu: f64,
    /// Shift applied to forward and strikes (0 for unshifted). Only matters
    /// for `beta > 0` or for lognormal-vol output.
    pub shift: f64,
}

impl SabrParams {
    pub fn new(alpha: f64, beta: f64, rho: f64, nu: f64, shift: f64) -> Result<Self, SabrError> {
        let p = Self { alpha, beta, rho, nu, shift };
        p.validate()?;
        Ok(p)
    }

    pub fn validate(&self) -> Result<(), SabrError> {
        if self.alpha <= 0.0 || !self.alpha.is_finite() {
            return Err(SabrError::Invalid(format!("alpha {} must be positive", self.alpha)));
        }
        if !(0.0..=1.0).contains(&self.beta) {
            return Err(SabrError::Invalid(format!("beta {} must be in [0, 1]", self.beta)));
        }
        if self.rho <= -1.0 || self.rho >= 1.0 {
            return Err(SabrError::Invalid(format!("rho {} must be in (-1, 1)", self.rho)));
        }
        if self.nu < 0.0 || !self.nu.is_finite() {
            return Err(SabrError::Invalid(format!("nu {} must be non-negative", self.nu)));
        }
        Ok(())
    }

    /// Hagan normal (Bachelier) implied vol at strike `k` for forward `f`
    /// and expiry `t`. Exact for `β = 0` up to the O(t) correction.
    pub fn normal_vol(&self, f: f64, k: f64, t: f64) -> f64 {
        let (a, b, r, n) = (self.alpha, self.beta, self.rho, self.nu);
        let (f, k) = (f + self.shift, k + self.shift);
        if b == 0.0 {
            // Normal SABR: closed form in (f - k).
            let z = n / a * (f - k);
            let x = if z.abs() < 1e-7 { 1.0 } else { z / ((((1.0 - 2.0 * r * z + z * z).sqrt() + z - r) / (1.0 - r)).ln()) };
            let corr = 1.0 + (2.0 - 3.0 * r * r) / 24.0 * n * n * t;
            return a * x * corr;
        }
        // General beta: Hagan normal-vol expansion with the geometric mean.
        let fk = f * k;
        let fk_mid = fk.powf((1.0 - b) / 2.0);
        let log_fk = (f / k).ln();
        let one_b = 1.0 - b;
        let z = n / a * fk_mid * log_fk;
        let x = if z.abs() < 1e-7 { 1.0 } else { z / ((((1.0 - 2.0 * r * z + z * z).sqrt() + z - r) / (1.0 - r)).ln()) };
        let numer_series = 1.0 + log_fk * log_fk / 24.0 + log_fk.powi(4) / 1920.0;
        let denom_series = 1.0 + one_b * one_b * log_fk * log_fk / 24.0 + one_b.powi(4) * log_fk.powi(4) / 1920.0;
        let corr = 1.0
            + (-b * (2.0 - b) * a * a / (24.0 * fk.powf(one_b)) + r * b * n * a / (4.0 * fk.powf(one_b / 2.0)) + (2.0 - 3.0 * r * r) * n * n / 24.0) * t;
        a * fk.powf(b / 2.0) * numer_series / denom_series * x * corr
    }

    /// Hagan lognormal (Black) implied vol on the shifted forward.
    pub fn lognormal_vol(&self, f: f64, k: f64, t: f64) -> Result<f64, SabrError> {
        let (a, b, r, n) = (self.alpha, self.beta, self.rho, self.nu);
        let (f, k) = (f + self.shift, k + self.shift);
        if f <= 0.0 || k <= 0.0 {
            return Err(SabrError::Invalid(format!("shifted forward {f} and strike {k} must be positive")));
        }
        let one_b = 1.0 - b;
        let fk = f * k;
        let log_fk = (f / k).ln();
        let fk_pow = fk.powf(one_b / 2.0);
        let z = n / a * fk_pow * log_fk;
        let x = if z.abs() < 1e-7 { 1.0 } else { z / ((((1.0 - 2.0 * r * z + z * z).sqrt() + z - r) / (1.0 - r)).ln()) };
        let denom = fk_pow * (1.0 + one_b * one_b * log_fk * log_fk / 24.0 + one_b.powi(4) * log_fk.powi(4) / 1920.0);
        let corr = 1.0 + (one_b * one_b * a * a / (24.0 * fk.powf(one_b)) + r * b * n * a / (4.0 * fk_pow) + (2.0 - 3.0 * r * r) * n * n / 24.0) * t;
        Ok(a / denom * x * corr)
    }
}

/// Which vol the market quotes are in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SabrVolType {
    Normal,
    Lognormal,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SabrCalibration {
    pub params: SabrParams,
    /// Root-mean-square vol error over the strip.
    pub rmse: f64,
    pub iterations: usize,
}

/// Fit `(alpha, rho, nu)` with `beta` and `shift` fixed to `(strike, vol)`
/// quotes at forward `f`, expiry `t`. `initial` seeds the search; a
/// reasonable default is `alpha = atm_vol`, `rho = 0`, `nu = 0.3`.
pub fn calibrate(
    f: f64,
    t: f64,
    beta: f64,
    shift: f64,
    quotes: &[(f64, f64)],
    vol_type: SabrVolType,
    initial: Option<(f64, f64, f64)>,
) -> Result<SabrCalibration, SabrError> {
    if quotes.len() < 3 {
        return Err(SabrError::TooFewStrikes { need: 3, got: quotes.len() });
    }
    let atm_guess = quotes.iter().min_by(|a, b| (a.0 - f).abs().partial_cmp(&(b.0 - f).abs()).unwrap_or(std::cmp::Ordering::Equal)).map(|q| q.1).unwrap_or(0.01);
    let (a0, r0, n0) = initial.unwrap_or((if beta == 0.0 { atm_guess } else { atm_guess * (f + shift).powf(1.0 - beta) }, 0.0, 0.3));

    // Unconstrained coordinates: alpha = exp(u0), rho = tanh(u1), nu = exp(u2).
    let to_params = |u: &[f64; 3]| SabrParams { alpha: u[0].exp(), beta, rho: u[1].tanh(), nu: u[2].exp(), shift };
    let objective = |u: &[f64; 3]| -> f64 {
        let p = to_params(u);
        let mut sse = 0.0;
        for &(k, v) in quotes {
            let model = match vol_type {
                SabrVolType::Normal => p.normal_vol(f, k, t),
                SabrVolType::Lognormal => match p.lognormal_vol(f, k, t) {
                    Ok(x) => x,
                    Err(_) => return f64::INFINITY,
                },
            };
            if !model.is_finite() {
                return f64::INFINITY;
            }
            sse += (model - v) * (model - v);
        }
        sse
    };

    let start = [a0.max(1e-8).ln(), r0.clamp(-0.999, 0.999).atanh(), n0.max(1e-8).ln()];
    let (u, sse, iterations) = nelder_mead(objective, start, [0.3, 0.3, 0.5], 2000, 1e-16);
    let params = to_params(&u);
    params.validate()?;
    let rmse = (sse / quotes.len() as f64).sqrt();
    if !rmse.is_finite() {
        return Err(SabrError::NoConvergence { rmse });
    }
    Ok(SabrCalibration { params, rmse, iterations })
}

/// Nelder–Mead simplex minimiser in three dimensions. Returns the best
/// point, its objective value, and the iteration count.
fn nelder_mead(f: impl Fn(&[f64; 3]) -> f64, start: [f64; 3], step: [f64; 3], max_iter: usize, tol: f64) -> ([f64; 3], f64, usize) {
    let mut simplex: Vec<([f64; 3], f64)> = Vec::with_capacity(4);
    simplex.push((start, f(&start)));
    for i in 0..3 {
        let mut p = start;
        p[i] += step[i];
        simplex.push((p, f(&p)));
    }
    let (alpha, gamma, rho, sigma) = (1.0, 2.0, 0.5, 0.5);
    let mut iter = 0;
    while iter < max_iter {
        iter += 1;
        simplex.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        let spread = simplex[3].1 - simplex[0].1;
        if spread.abs() < tol * (1.0 + simplex[0].1.abs()) {
            break;
        }
        let mut centroid = [0.0; 3];
        for p in &simplex[..3] {
            for k in 0..3 {
                centroid[k] += p.0[k] / 3.0;
            }
        }
        let worst = simplex[3];
        let reflect = |c: f64| {
            let mut p = [0.0; 3];
            for k in 0..3 {
                p[k] = centroid[k] + c * (centroid[k] - worst.0[k]);
            }
            p
        };
        let xr = reflect(alpha);
        let fr = f(&xr);
        if fr < simplex[0].1 {
            let xe = reflect(gamma);
            let fe = f(&xe);
            simplex[3] = if fe < fr { (xe, fe) } else { (xr, fr) };
        } else if fr < simplex[2].1 {
            simplex[3] = (xr, fr);
        } else {
            let xc = if fr < worst.1 { reflect(rho) } else { reflect(-rho) };
            let fc = f(&xc);
            if fc < fr.min(worst.1) {
                simplex[3] = (xc, fc);
            } else {
                let best = simplex[0].0;
                for p in simplex.iter_mut().skip(1) {
                    for k in 0..3 {
                        p.0[k] = best[k] + sigma * (p.0[k] - best[k]);
                    }
                    p.1 = f(&p.0);
                }
            }
        }
    }
    simplex.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
    (simplex[0].0, simplex[0].1, iter)
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_abs_diff_eq;

    type R = Result<(), SabrError>;

    #[test]
    fn normal_sabr_atm_is_alpha_times_correction() -> R {
        let p = SabrParams::new(0.0070, 0.0, -0.2, 0.35, 0.0)?;
        let (f, t) = (0.04, 2.0);
        let expected = 0.0070 * (1.0 + (2.0 - 3.0 * 0.04) / 24.0 * 0.35 * 0.35 * t);
        assert_abs_diff_eq!(p.normal_vol(f, f, t), expected, epsilon = 1e-15);
        // Zero vol-of-vol: flat smile at alpha.
        let flat = SabrParams::new(0.0070, 0.0, 0.0, 0.0, 0.0)?;
        for k in [0.02, 0.04, 0.06] {
            assert_abs_diff_eq!(flat.normal_vol(f, k, t), 0.0070, epsilon = 1e-15);
        }
        Ok(())
    }

    #[test]
    fn negative_rho_skews_the_normal_smile_down_to_the_right() -> R {
        let p = SabrParams::new(0.0070, 0.0, -0.3, 0.4, 0.0)?;
        let (f, t) = (0.04, 1.0);
        let lo = p.normal_vol(f, 0.03, t);
        let atm = p.normal_vol(f, f, t);
        let hi = p.normal_vol(f, 0.05, t);
        assert!(lo > atm, "{lo} vs {atm}");
        assert!(hi < lo);
        // Smile is convex: wings above a chord through ATM for rho = 0.
        let sym = SabrParams::new(0.0070, 0.0, 0.0, 0.4, 0.0)?;
        assert_abs_diff_eq!(sym.normal_vol(f, 0.03, t), sym.normal_vol(f, 0.05, t), epsilon = 1e-15);
        assert!(sym.normal_vol(f, 0.03, t) > sym.normal_vol(f, f, t));
        // Negative strikes are fine in normal SABR.
        assert!(p.normal_vol(0.001, -0.005, t).is_finite());
        Ok(())
    }

    #[test]
    fn lognormal_atm_matches_hagan_closed_form() -> R {
        let p = SabrParams::new(0.04, 0.5, -0.25, 0.4, 0.0)?;
        let (f, t): (f64, f64) = (0.03, 3.0);
        let fb = f.powf(1.0 - 0.5);
        let expected = 0.04 / fb * (1.0 + (0.25 * 0.04 * 0.04 / (24.0 * f) + (-0.25) * 0.5 * 0.4 * 0.04 / (4.0 * fb) + (2.0 - 3.0 * 0.0625) * 0.16 / 24.0) * t);
        assert_abs_diff_eq!(p.lognormal_vol(f, f, t)?, expected, epsilon = 1e-12);
        assert!(p.lognormal_vol(-0.01, 0.01, t).is_err());
        Ok(())
    }

    #[test]
    fn beta_zero_general_branch_agrees_with_closed_form_near_atm() -> R {
        // The beta > 0 expansion with beta -> 0 should approach the normal closed form.
        let closed = SabrParams::new(0.0065, 0.0, -0.1, 0.3, 0.0)?;
        let small_beta = SabrParams::new(0.0065 * 0.04_f64.powf(-1e-6), 1e-6, -0.1, 0.3, 0.0)?;
        for k in [0.035, 0.04, 0.045] {
            assert_abs_diff_eq!(closed.normal_vol(0.04, k, 1.0), small_beta.normal_vol(0.04, k, 1.0), epsilon = 2e-6);
        }
        Ok(())
    }

    #[test]
    fn calibration_recovers_known_normal_parameters() -> R {
        let truth = SabrParams::new(0.0072, 0.0, -0.28, 0.42, 0.0)?;
        let (f, t) = (0.038, 2.0);
        let strikes = [-0.010, -0.005, -0.0025, 0.0, 0.0025, 0.005, 0.010];
        let quotes: Vec<(f64, f64)> = strikes.iter().map(|&dk| (f + dk, truth.normal_vol(f, f + dk, t))).collect();
        let cal = calibrate(f, t, 0.0, 0.0, &quotes, SabrVolType::Normal, None)?;
        assert!(cal.rmse < 1e-8, "rmse {}", cal.rmse);
        assert_abs_diff_eq!(cal.params.alpha, 0.0072, epsilon = 1e-6);
        assert_abs_diff_eq!(cal.params.rho, -0.28, epsilon = 1e-3);
        assert_abs_diff_eq!(cal.params.nu, 0.42, epsilon = 1e-3);
        Ok(())
    }

    #[test]
    fn calibration_recovers_known_lognormal_parameters() -> R {
        let truth = SabrParams::new(0.035, 0.5, -0.2, 0.5, 0.02)?;
        let (f, t) = (0.025, 5.0);
        let quotes: Vec<(f64, f64)> = [0.005, 0.015, 0.025, 0.035, 0.05].iter().map(|&k| (k, truth.lognormal_vol(f, k, t).expect("vol"))).collect();
        let cal = calibrate(f, t, 0.5, 0.02, &quotes, SabrVolType::Lognormal, None)?;
        assert!(cal.rmse < 1e-8, "rmse {}", cal.rmse);
        assert_abs_diff_eq!(cal.params.alpha, 0.035, epsilon = 1e-5);
        assert_abs_diff_eq!(cal.params.rho, -0.2, epsilon = 1e-3);
        assert_abs_diff_eq!(cal.params.nu, 0.5, epsilon = 1e-3);
        Ok(())
    }

    #[test]
    fn validation_and_strip_size() {
        assert!(SabrParams::new(-0.01, 0.0, 0.0, 0.3, 0.0).is_err());
        assert!(SabrParams::new(0.01, 1.5, 0.0, 0.3, 0.0).is_err());
        assert!(SabrParams::new(0.01, 0.0, 1.0, 0.3, 0.0).is_err());
        assert!(SabrParams::new(0.01, 0.0, 0.0, -0.1, 0.0).is_err());
        assert!(matches!(calibrate(0.04, 1.0, 0.0, 0.0, &[(0.04, 0.007), (0.05, 0.0075)], SabrVolType::Normal, None), Err(SabrError::TooFewStrikes { .. })));
    }
}
