//! Exact Monte Carlo of the Hull-White factor and bank account.
//!
//! Per step the pair `(x, I)` with `I(t) = int_0^t x(u) du` is jointly
//! Gaussian with known moments, so paths are sampled exactly on any time
//! grid (no Euler error):
//!
//! ```text
//! x(t + h) = x(t) e^{-a h} + e1
//! I(t + h) = I(t) + x(t) B(h) + e2
//! Var e1 = v(h),  Var e2 = sigma^2/a^2 (h - 2 B(h) + (1 - e^{-2 a h}) / (2 a)),
//! Cov    = sigma^2 (1 - e^{-a h})^2 / (2 a^2)
//! ```
//!
//! The stochastic discount factor to `t` is `P(0, t) exp(-I(t) - int_0^t y)`,
//! so `E[D(t)] = P(0, t)` and `E[D(t) P(t, T | x(t))] = P(0, T)` hold in
//! expectation and are what the tests check. Bond prices along a path come
//! from `HullWhite::bond`, which makes these paths the input for anything
//! path-dependent on rates (MBS cash flows, callable structures by
//! regression).

use fq_core::math::Xoshiro256StarStar;

use crate::error::ModelError;
use crate::hull_white::HullWhite;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct McConfig {
    pub paths: usize,
    pub seed: u64,
    /// Antithetic pairs (paths must be even).
    pub antithetic: bool,
}

impl Default for McConfig {
    fn default() -> Self {
        Self { paths: 20_000, seed: 12345, antithetic: true }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ShortRatePaths {
    model: HullWhite,
    times: Vec<f64>,
    n_paths: usize,
    /// Row-major `[path][step]`.
    factor: Vec<f64>,
    integral: Vec<f64>,
}

impl HullWhite {
    /// Simulates `(x, int x)` on `times` (strictly increasing, positive).
    pub fn simulate(&self, times: &[f64], cfg: McConfig) -> Result<ShortRatePaths, ModelError> {
        if cfg.paths == 0 || times.is_empty() || (cfg.antithetic && cfg.paths % 2 != 0) {
            return Err(ModelError::BadSimulation);
        }
        let mut prev = 0.0;
        for &t in times {
            if !(t.is_finite() && t > prev) {
                return Err(ModelError::BadSimulationTime(t));
            }
            prev = t;
        }
        let a = self.mean_reversion();
        let sigma = self.sigma();
        // Per-step moments and the Cholesky factor of the (x, I) innovation.
        let mut steps = Vec::with_capacity(times.len());
        let mut t0 = 0.0;
        for &t in times {
            let h = t - t0;
            let decay = (-a * h).exp();
            let b = (1.0 - decay) / a;
            let var_x = sigma * sigma * (1.0 - (-2.0 * a * h).exp()) / (2.0 * a);
            let var_i = sigma * sigma / (a * a) * (h - 2.0 * b + (1.0 - (-2.0 * a * h).exp()) / (2.0 * a));
            let cov = sigma * sigma * (1.0 - decay) * (1.0 - decay) / (2.0 * a * a);
            let l11 = var_x.sqrt();
            let l21 = if l11 > 0.0 { cov / l11 } else { 0.0 };
            let l22 = (var_i - l21 * l21).max(0.0).sqrt();
            steps.push((decay, b, l11, l21, l22));
            t0 = t;
        }
        let n_steps = times.len();
        let mut factor = vec![0.0; cfg.paths * n_steps];
        let mut integral = vec![0.0; cfg.paths * n_steps];
        let mut rng = Xoshiro256StarStar::seed_from_u64(cfg.seed);
        let mut draws = vec![0.0; 2 * n_steps];
        let mut p = 0;
        while p < cfg.paths {
            for z in draws.iter_mut() {
                *z = rng.next_gaussian();
            }
            let reps = if cfg.antithetic { 2 } else { 1 };
            for r in 0..reps {
                let flip = if r == 0 { 1.0 } else { -1.0 };
                let (mut x, mut i) = (0.0, 0.0);
                for (k, &(decay, b, l11, l21, l22)) in steps.iter().enumerate() {
                    let z1 = flip * draws[2 * k];
                    let z2 = flip * draws[2 * k + 1];
                    let e1 = l11 * z1;
                    let e2 = l21 * z1 + l22 * z2;
                    i += x * b + e2;
                    x = x * decay + e1;
                    factor[p * n_steps + k] = x;
                    integral[p * n_steps + k] = i;
                }
                p += 1;
            }
        }
        Ok(ShortRatePaths { model: self.clone(), times: times.to_vec(), n_paths: cfg.paths, factor, integral })
    }
}

impl ShortRatePaths {
    pub fn times(&self) -> &[f64] {
        &self.times
    }

    pub fn n_paths(&self) -> usize {
        self.n_paths
    }

    pub fn n_steps(&self) -> usize {
        self.times.len()
    }

    pub fn model(&self) -> &HullWhite {
        &self.model
    }

    /// The factor `x` at `(path, step)`.
    pub fn factor(&self, path: usize, step: usize) -> f64 {
        self.factor[path * self.times.len() + step]
    }

    /// `int_0^t x(u) du` at `(path, step)`.
    pub fn integral(&self, path: usize, step: usize) -> f64 {
        self.integral[path * self.times.len() + step]
    }

    /// Short rate `x + alpha(t)` (uses the curve's instantaneous forward).
    pub fn short_rate(&self, path: usize, step: usize) -> f64 {
        self.factor(path, step) + self.model.alpha(self.times[step])
    }

    /// Stochastic discount factor from 0 to the step's time.
    pub fn discount_to(&self, path: usize, step: usize) -> f64 {
        let t = self.times[step];
        self.model.curve().discount_t(t) * (-self.integral(path, step) - self.model.int_y(t)).exp()
    }

    /// Zero-coupon bond `P(t_step, big_t)` on the path.
    pub fn bond(&self, path: usize, step: usize, big_t: f64) -> Result<f64, ModelError> {
        self.model.bond(self.times[step], big_t, self.factor(path, step))
    }

    /// Mean and standard error of `payoff(path)` over the paths.
    pub fn estimate(&self, mut payoff: impl FnMut(usize) -> f64) -> (f64, f64) {
        let n = self.n_paths as f64;
        let (mut sum, mut sum_sq) = (0.0, 0.0);
        for p in 0..self.n_paths {
            let v = payoff(p);
            sum += v;
            sum_sq += v * v;
        }
        let mean = sum / n;
        let var = (sum_sq / n - mean * mean).max(0.0);
        (mean, (var / n).sqrt())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_abs_diff_eq;
    use fq_core::time::Date;
    use fq_rates::curve::DiscountCurve;
    use fq_rates::swap::Tenor;
    use fq_rates::swaption::{Swaption, SwaptionType};

    type R = Result<(), ModelError>;

    fn d(y: i32, m: u32, dd: u32) -> Date {
        Date::from_ymd(y, m, dd).expect("valid test date")
    }

    fn model(a: f64, sigma: f64) -> HullWhite {
        let today = d(2026, 10, 13);
        let nodes = [(d(2027, 10, 13), 0.035), (d(2029, 10, 13), 0.038), (d(2031, 10, 13), 0.040), (d(2036, 10, 13), 0.042), (d(2056, 10, 13), 0.043)];
        let curve = DiscountCurve::from_zero_rates(today, &nodes, Default::default()).expect("curve");
        HullWhite::new(curve, a, sigma).expect("model")
    }

    #[test]
    fn shape_and_validation() -> R {
        let m = model(0.05, 0.01);
        let paths = m.simulate(&[0.5, 1.0, 2.0], McConfig { paths: 100, seed: 1, antithetic: true })?;
        assert_eq!(paths.n_paths(), 100);
        assert_eq!(paths.n_steps(), 3);
        assert_eq!(paths.times(), &[0.5, 1.0, 2.0]);
        // Antithetic pairs mirror in the factor.
        assert_abs_diff_eq!(paths.factor(0, 2), -paths.factor(1, 2), epsilon = 1e-15);
        assert!(m.simulate(&[], McConfig::default()).is_err());
        assert!(m.simulate(&[1.0, 0.5], McConfig::default()).is_err());
        assert!(m.simulate(&[0.0], McConfig::default()).is_err());
        assert!(m.simulate(&[1.0], McConfig { paths: 3, seed: 1, antithetic: true }).is_err());
        assert!(m.simulate(&[1.0], McConfig { paths: 3, seed: 1, antithetic: false }).is_ok());
        // Reproducible.
        let again = m.simulate(&[0.5, 1.0, 2.0], McConfig { paths: 100, seed: 1, antithetic: true })?;
        assert_eq!(paths, again);
        Ok(())
    }

    #[test]
    fn factor_moments() -> R {
        let m = model(0.1, 0.01);
        let paths = m.simulate(&[0.25, 1.0, 3.0, 5.0], McConfig { paths: 40_000, seed: 7, antithetic: false })?;
        for (k, &t) in paths.times().iter().enumerate() {
            let (mean, _) = paths.estimate(|p| paths.factor(p, k));
            let (m2, _) = paths.estimate(|p| paths.factor(p, k).powi(2));
            assert!(mean.abs() < 1e-4, "t {t}: mean {mean}");
            assert!((m2 - m.variance(t)).abs() < 0.02 * m.variance(t), "t {t}: {m2} vs {}", m.variance(t));
            let a = m.mean_reversion();
            let var_i = m.sigma().powi(2) / (a * a) * (t - 2.0 * m.b(0.0, t) + (1.0 - (-2.0 * a * t).exp()) / (2.0 * a));
            let (i2, _) = paths.estimate(|p| paths.integral(p, k).powi(2));
            assert!((i2 - var_i).abs() < 0.03 * var_i, "t {t}: {i2} vs {var_i}");
        }
        Ok(())
    }

    #[test]
    fn bank_account_reprices_the_curve_and_its_bonds() -> R {
        let m = model(0.05, 0.012);
        let paths = m.simulate(&[1.0, 2.0, 5.0, 10.0], McConfig { paths: 50_000, seed: 3, antithetic: true })?;
        for (k, &t) in paths.times().iter().enumerate() {
            let (mean, se) = paths.estimate(|p| paths.discount_to(p, k));
            let target = m.curve().discount_t(t);
            assert!((mean - target).abs() < 4.0 * se.max(1e-6), "t {t}: {mean} vs {target} (se {se})");
            let big_t = t + 7.0;
            let (mean, se) = paths.estimate(|p| paths.discount_to(p, k) * paths.bond(p, k, big_t).expect("bond"));
            let target = m.curve().discount_t(big_t);
            assert!((mean - target).abs() < 4.0 * se.max(1e-6), "T {big_t}: {mean} vs {target} (se {se})");
        }
        Ok(())
    }

    #[test]
    fn short_rate_mean_follows_alpha() -> R {
        let m = model(0.1, 0.01);
        let paths = m.simulate(&[2.0], McConfig { paths: 20_000, seed: 11, antithetic: true })?;
        let (mean, _) = paths.estimate(|p| paths.short_rate(p, 0));
        assert_abs_diff_eq!(mean, m.alpha(2.0), epsilon = 1e-12); // antithetic: exact
        Ok(())
    }

    #[test]
    fn european_swaption_by_simulation_matches_closed_form() -> R {
        let m = model(0.05, 0.01);
        let sw = Swaption::usd_sofr(d(2027, 10, 13), Tenor::years(10), SwaptionType::Payer, 1e6, 0.04).expect("swaption");
        let closed = m.european_swaption(&sw)?.pv;
        let t = m.time(sw.expiry());
        let paths = m.simulate(&[t], McConfig { paths: 100_000, seed: 5, antithetic: true })?;
        let v = m.payer_terms(t, sw.underlying(), sw.expiry());
        let (mean, se) = paths.estimate(|p| paths.discount_to(p, 0) * v.value(paths.factor(p, 0)).max(0.0));
        assert!((mean - closed).abs() < 3.0 * se, "{mean} vs {closed} (se {se})");
        assert!(se < 0.01 * closed);
        Ok(())
    }
}
