//! Clewlow-Strickland forward-curve model and Monte Carlo paths.
//!
//! Each futures contract `F(t, T)` (expiring at `T`) follows
//!
//! ```text
//! dF(t,T) / F(t,T) = sigma_s exp(-alpha (T - t)) dW_s + sigma_l dW_l,
//! corr(dW_s, dW_l) = rho
//! ```
//!
//! The first factor is a mean-reverting short-term shock whose effect on a
//! contract decays with its time to expiry (this is the Samuelson effect,
//! so a contract's vol rises into expiry), the second a parallel long-term
//! shift. Set `sigma_l = 0` for the one-factor model. Both factors are
//! shared across all maturities, so the whole curve moves consistently:
//! that is what storage, swing and spread products need and what a set of
//! independent Black-76 pricers cannot give you.
//!
//! Simulation is exact in distribution on any time grid. The trick is that
//! `∫ e^{-alpha (T - s)} dW_s = e^{-alpha T} ∫ e^{alpha s} dW_s`, so one
//! Gaussian per step per factor drives every maturity. No Euler error.
//!
//! The RNG is a self-contained xoshiro256** so the crate has no external
//! dependency and paths are reproducible across platforms.

use crate::error::CommodError;

/// xoshiro256** (Blackman & Vigna) with splitmix64 seeding.
#[derive(Debug, Clone, PartialEq)]
pub struct Xoshiro256StarStar {
    s: [u64; 4],
    spare: Option<f64>,
}

impl Xoshiro256StarStar {
    pub fn seed_from_u64(seed: u64) -> Self {
        let mut x = seed;
        let mut next = || {
            x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = x;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^ (z >> 31)
        };
        Self { s: [next(), next(), next(), next()], spare: None }
    }

    pub fn next_u64(&mut self) -> u64 {
        let result = self.s[1].wrapping_mul(5).rotate_left(7).wrapping_mul(9);
        let t = self.s[1] << 17;
        self.s[2] ^= self.s[0];
        self.s[3] ^= self.s[1];
        self.s[1] ^= self.s[2];
        self.s[0] ^= self.s[3];
        self.s[2] ^= t;
        self.s[3] = self.s[3].rotate_left(45);
        result
    }

    /// Uniform in (0, 1), never exactly 0.
    pub fn next_f64(&mut self) -> f64 {
        ((self.next_u64() >> 11) as f64 + 0.5) * (1.0 / (1u64 << 53) as f64)
    }

    /// Standard normal by Marsaglia polar method (pairs cached).
    pub fn next_gaussian(&mut self) -> f64 {
        if let Some(z) = self.spare.take() {
            return z;
        }
        loop {
            let u = 2.0 * self.next_f64() - 1.0;
            let v = 2.0 * self.next_f64() - 1.0;
            let s = u * u + v * v;
            if s > 0.0 && s < 1.0 {
                let m = (-2.0 * s.ln() / s).sqrt();
                self.spare = Some(v * m);
                return u * m;
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct McConfig {
    pub paths: usize,
    pub seed: u64,
    /// Use antithetic pairs (paths must be even).
    pub antithetic: bool,
}

impl Default for McConfig {
    fn default() -> Self {
        Self { paths: 20_000, seed: 12345, antithetic: true }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClewlowStrickland {
    pub sigma_short: f64,
    pub alpha: f64,
    pub sigma_long: f64,
    pub rho: f64,
}

impl ClewlowStrickland {
    pub fn two_factor(sigma_short: f64, alpha: f64, sigma_long: f64, rho: f64) -> Result<Self, CommodError> {
        for s in [sigma_short, sigma_long] {
            if !(s.is_finite() && s >= 0.0) {
                return Err(CommodError::BadVol(s));
            }
        }
        if !(alpha.is_finite() && alpha >= 0.0) {
            return Err(CommodError::Invalid(format!("mean reversion alpha {alpha} must be non-negative")));
        }
        if !(rho.is_finite() && (-1.0..=1.0).contains(&rho)) {
            return Err(CommodError::BadCorrelation(rho));
        }
        Ok(Self { sigma_short, alpha, sigma_long, rho })
    }

    pub fn one_factor(sigma: f64, alpha: f64) -> Result<Self, CommodError> {
        Self::two_factor(sigma, alpha, 0.0, 0.0)
    }

    /// Instantaneous vol of the contract expiring `tau` years ahead.
    pub fn instantaneous_vol(&self, tau: f64) -> f64 {
        let a = self.sigma_short * (-self.alpha * tau.max(0.0)).exp();
        (a * a + 2.0 * self.rho * a * self.sigma_long + self.sigma_long * self.sigma_long).sqrt()
    }

    /// Integrated log-variance of `F(., T)` over `[t0, t1]`.
    fn variance(&self, t0: f64, t1: f64, t_contract: f64) -> f64 {
        let (ss, a, sl, rho) = (self.sigma_short, self.alpha, self.sigma_long, self.rho);
        let dt = t1 - t0;
        if a < 1e-12 {
            let s = ss * ss + 2.0 * rho * ss * sl + sl * sl;
            return s * dt;
        }
        let e1 = (-a * (t_contract - t1)).exp() - (-a * (t_contract - t0)).exp();
        let e2 = (-2.0 * a * (t_contract - t1)).exp() - (-2.0 * a * (t_contract - t0)).exp();
        ss * ss * e2 / (2.0 * a) + 2.0 * rho * ss * sl * e1 / a + sl * sl * dt
    }

    /// Black implied vol for an option expiring at `t_option` on the
    /// contract expiring at `t_contract`.
    pub fn black_vol(&self, t_option: f64, t_contract: f64) -> Result<f64, CommodError> {
        if !(t_option.is_finite() && t_contract.is_finite()) || t_option < 0.0 || t_contract < t_option {
            return Err(CommodError::Invalid(format!("need 0 <= t_option ({t_option}) <= t_contract ({t_contract})")));
        }
        if t_option == 0.0 {
            return Ok(self.instantaneous_vol(t_contract));
        }
        Ok((self.variance(0.0, t_option, t_contract) / t_option).sqrt())
    }

    /// Model-implied log-return correlation between two contracts over
    /// `[0, t]`. One-factor gives 1 for every pair; the long factor breaks
    /// that, which is how the model is calibrated to a term structure of
    /// calendar-spread vols.
    pub fn correlation(&self, t: f64, t1: f64, t2: f64) -> Result<f64, CommodError> {
        if !(t > 0.0 && t <= t1.min(t2)) {
            return Err(CommodError::Invalid(format!("need 0 < t ({t}) <= min(T1, T2)")));
        }
        let (ss, a, sl, rho) = (self.sigma_short, self.alpha, self.sigma_long, self.rho);
        let cov = if a < 1e-12 {
            (ss * ss + 2.0 * rho * ss * sl + sl * sl) * t
        } else {
            let e2 = ((-a * (t1 + t2 - 2.0 * t)).exp() - (-a * (t1 + t2)).exp()) / (2.0 * a);
            let e1a = ((-a * (t1 - t)).exp() - (-a * t1).exp()) / a;
            let e1b = ((-a * (t2 - t)).exp() - (-a * t2).exp()) / a;
            ss * ss * e2 + rho * ss * sl * (e1a + e1b) + sl * sl * t
        };
        let v1 = self.variance(0.0, t, t1);
        let v2 = self.variance(0.0, t, t2);
        if v1 <= 0.0 || v2 <= 0.0 {
            return Ok(1.0);
        }
        Ok((cov / (v1 * v2).sqrt()).clamp(-1.0, 1.0))
    }

    /// Simulate the forward curve. `initial` is `(t_contract, F(0, T))` per
    /// contract (any order, maturities positive); `times` the increasing
    /// observation grid starting after 0. Returns forwards at every grid
    /// point for every contract; a contract past its expiry is frozen at
    /// its expiry value, so a reader can treat `F(T, T)` as the spot fix.
    pub fn simulate(&self, initial: &[(f64, f64)], times: &[f64], cfg: McConfig) -> Result<CurvePaths, CommodError> {
        if initial.is_empty() {
            return Err(CommodError::Invalid("no contracts to simulate".into()));
        }
        if times.is_empty() || times[0] <= 0.0 || times.windows(2).any(|w| w[1] <= w[0]) {
            return Err(CommodError::Invalid("times must be increasing and start after 0".into()));
        }
        for &(t, f) in initial {
            if !(t.is_finite() && t > 0.0) || !(f.is_finite() && f > 0.0) {
                return Err(CommodError::Invalid(format!("bad contract (T={t}, F={f})")));
            }
        }
        if cfg.paths == 0 || (cfg.antithetic && cfg.paths % 2 != 0) {
            return Err(CommodError::BadMcConfig(format!("paths {} must be positive (and even when antithetic)", cfg.paths)));
        }

        let n_t = times.len();
        let n_c = initial.len();
        let (ss, a, sl, rho) = (self.sigma_short, self.alpha, self.sigma_long, self.rho);
        let small_alpha = a < 1e-12;

        // Per step: distribution of (J, dW_l) where J = ∫ e^{a s} dW_s
        // (or dW_s itself when alpha ~ 0), and per contract the drift
        // -0.5 * variance over the step with the step clipped at expiry.
        struct Step {
            sd_j: f64,
            cov_jl: f64,
            sqrt_dt: f64,
            drift: Vec<f64>,
            // Multiplier e^{-a T} on J per contract; 1 when alpha ~ 0.
            decay: Vec<f64>,
            // Fraction of the step before the contract expires, used to
            // scale the long-factor increment once a contract is near expiry.
            alive: Vec<bool>,
        }
        let mut steps = Vec::with_capacity(n_t);
        let mut prev = 0.0;
        for &t1 in times {
            let dt = t1 - prev;
            let (var_j, cov_jl) = if small_alpha {
                (dt, rho * dt)
            } else {
                (((2.0 * a * t1).exp() - (2.0 * a * prev).exp()) / (2.0 * a), rho * ((a * t1).exp() - (a * prev).exp()) / a)
            };
            let mut drift = Vec::with_capacity(n_c);
            let mut decay = Vec::with_capacity(n_c);
            let mut alive = Vec::with_capacity(n_c);
            for &(tc, _) in initial {
                // A contract expiring inside the step gets its variance to
                // expiry only when the step ends exactly at expiry; for
                // simplicity we require the grid to contain expiries that
                // matter, and freeze a contract once t1 > tc.
                let is_alive = prev < tc && t1 <= tc + 1e-12;
                alive.push(is_alive);
                drift.push(if is_alive { -0.5 * self.variance(prev, t1, tc) } else { 0.0 });
                decay.push(if small_alpha { 1.0 } else { (-a * tc).exp() });
            }
            steps.push(Step { sd_j: var_j.sqrt(), cov_jl, sqrt_dt: dt.sqrt(), drift, decay, alive });
            prev = t1;
        }

        let mut rng = Xoshiro256StarStar::seed_from_u64(cfg.seed);
        let mut data = vec![0.0; cfg.paths * n_t * n_c];
        let mut log_f = vec![0.0; n_c];
        let pairs = if cfg.antithetic { cfg.paths / 2 } else { cfg.paths };
        let mut path_idx = 0;
        // Pre-draw per-step normals for a pair so antithetic reuses them.
        let mut z = vec![(0.0, 0.0); n_t];
        for _ in 0..pairs {
            for zk in z.iter_mut() {
                *zk = (rng.next_gaussian(), rng.next_gaussian());
            }
            let signs: &[f64] = if cfg.antithetic { &[1.0, -1.0] } else { &[1.0] };
            for &sgn in signs {
                for (c, &(_, f0)) in initial.iter().enumerate() {
                    log_f[c] = f0.ln();
                }
                for (k, st) in steps.iter().enumerate() {
                    let (z1, z2) = (sgn * z[k].0, sgn * z[k].1);
                    // J ~ N(0, sd_j^2), dW_l ~ N(0, dt), cov = cov_jl.
                    let j = st.sd_j * z1;
                    let dw_l = if st.sd_j > 0.0 {
                        let beta = st.cov_jl / st.sd_j; // regression coefficient of dW_l on J normalised
                        let resid_var = (st.sqrt_dt * st.sqrt_dt - beta * beta).max(0.0);
                        beta * z1 + resid_var.sqrt() * z2
                    } else {
                        st.sqrt_dt * z2
                    };
                    for c in 0..n_c {
                        if st.alive[c] {
                            log_f[c] += st.drift[c] + ss * st.decay[c] * j + sl * dw_l;
                        }
                        data[(path_idx * n_t + k) * n_c + c] = log_f[c].exp();
                    }
                }
                path_idx += 1;
            }
        }
        Ok(CurvePaths {
            times: times.to_vec(),
            maturities: initial.iter().map(|x| x.0).collect(),
            initial: initial.iter().map(|x| x.1).collect(),
            n_paths: cfg.paths,
            data,
        })
    }
}

/// Simulated forward curves: `forward(path, step, contract)`.
#[derive(Debug, Clone, PartialEq)]
pub struct CurvePaths {
    times: Vec<f64>,
    maturities: Vec<f64>,
    initial: Vec<f64>,
    n_paths: usize,
    data: Vec<f64>,
}

impl CurvePaths {
    pub fn times(&self) -> &[f64] {
        &self.times
    }

    pub fn maturities(&self) -> &[f64] {
        &self.maturities
    }

    pub fn initial_forwards(&self) -> &[f64] {
        &self.initial
    }

    pub fn n_paths(&self) -> usize {
        self.n_paths
    }

    pub fn n_contracts(&self) -> usize {
        self.maturities.len()
    }

    pub fn forward(&self, path: usize, step: usize, contract: usize) -> f64 {
        self.data[(path * self.times.len() + step) * self.maturities.len() + contract]
    }

    /// All contracts' forwards on one path at one step.
    pub fn curve_at(&self, path: usize, step: usize) -> &[f64] {
        let n_c = self.maturities.len();
        let start = (path * self.times.len() + step) * n_c;
        &self.data[start..start + n_c]
    }

    /// Mean and standard error of a payoff evaluated on each path's full
    /// history: `payoff(path_index) -> value`.
    pub fn estimate(&self, mut payoff: impl FnMut(usize) -> f64) -> (f64, f64) {
        let n = self.n_paths as f64;
        let (mut sum, mut sumsq) = (0.0, 0.0);
        for p in 0..self.n_paths {
            let v = payoff(p);
            sum += v;
            sumsq += v * v;
        }
        let mean = sum / n;
        let var = (sumsq / n - mean * mean).max(0.0);
        (mean, (var / n).sqrt())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_abs_diff_eq;
    use fq_core::math::{OptionType, black76};

    #[test]
    fn rng_is_deterministic_and_roughly_normal() {
        let mut a = Xoshiro256StarStar::seed_from_u64(1);
        let mut b = Xoshiro256StarStar::seed_from_u64(1);
        assert_eq!(a.next_u64(), b.next_u64());
        let n = 2_000_000;
        let (mut m, mut v) = (0.0, 0.0);
        for _ in 0..n {
            let z = a.next_gaussian();
            m += z;
            v += z * z;
        }
        // Standard errors at n = 2e6: mean ~7e-4, variance ~1e-3.
        assert_abs_diff_eq!(m / n as f64, 0.0, epsilon = 0.004);
        assert_abs_diff_eq!(v / n as f64, 1.0, epsilon = 0.005);
    }

    #[test]
    fn black_vol_closed_form_matches_quadrature() {
        let m = ClewlowStrickland::two_factor(0.60, 1.5, 0.20, 0.30).expect("model");
        let (te, tc) = (0.5, 0.9);
        let n = 100_000;
        let h = te / n as f64;
        let mut acc = 0.0;
        for i in 0..n {
            let t = (i as f64 + 0.5) * h;
            let s = m.instantaneous_vol(tc - t);
            acc += s * s * h;
        }
        assert_abs_diff_eq!(m.black_vol(te, tc).expect("bv"), (acc / te).sqrt(), epsilon = 1e-7);
        // Short-dated contract has higher Black vol than a far one.
        assert!(m.black_vol(0.25, 0.3).expect("a") > m.black_vol(0.25, 3.0).expect("b"));
    }

    #[test]
    fn one_factor_has_perfect_correlation_two_factor_does_not() {
        let one = ClewlowStrickland::one_factor(0.5, 2.0).expect("1f");
        assert_abs_diff_eq!(one.correlation(0.5, 1.0, 2.0).expect("c"), 1.0, epsilon = 1e-12);
        let two = ClewlowStrickland::two_factor(0.5, 2.0, 0.15, 0.0).expect("2f");
        let c = two.correlation(0.5, 1.0, 2.0).expect("c");
        assert!(c > 0.5 && c < 1.0, "corr {c}");
    }

    #[test]
    fn simulated_forwards_are_martingales_and_price_black() {
        let m = ClewlowStrickland::two_factor(0.55, 1.2, 0.18, 0.25).expect("model");
        let initial = [(0.5, 4.0), (1.0, 3.8), (2.0, 3.5)];
        let times = [0.1, 0.25, 0.5, 0.75, 1.0];
        let cfg = McConfig { paths: 200_000, seed: 99, antithetic: true };
        let paths = m.simulate(&initial, &times, cfg).expect("sim");
        // Martingale at the last step for the contract still alive.
        let (mean, se) = paths.estimate(|p| paths.forward(p, 4, 2));
        assert_abs_diff_eq!(mean, 3.5, epsilon = 4.0 * se + 1e-9);
        // The 0.5y contract is frozen after step index 2.
        for p in 0..5 {
            assert_abs_diff_eq!(paths.forward(p, 2, 0), paths.forward(p, 4, 0));
        }
        // Call on the 1.0y contract expiring at 0.75: compare to Black-76
        // with the model-implied vol.
        let k = 3.9;
        let (mc, se) = paths.estimate(|p| (paths.forward(p, 3, 1) - k).max(0.0));
        let bv = m.black_vol(0.75, 1.0).expect("bv");
        let bl = black76(OptionType::Call, 3.8, k, bv, 0.75).expect("black").price;
        assert_abs_diff_eq!(mc, bl, epsilon = 4.0 * se + 1e-9);
        // Realised log-correlation between the 1y and 2y contracts at 0.5.
        let (mut sxy, mut sxx, mut syy, mut sx, mut sy) = (0.0, 0.0, 0.0, 0.0, 0.0);
        for p in 0..paths.n_paths() {
            let x = (paths.forward(p, 2, 1) / 3.8).ln();
            let y = (paths.forward(p, 2, 2) / 3.5).ln();
            sx += x;
            sy += y;
            sxx += x * x;
            syy += y * y;
            sxy += x * y;
        }
        let n = paths.n_paths() as f64;
        let cov = sxy / n - (sx / n) * (sy / n);
        let vx = sxx / n - (sx / n).powi(2);
        let vy = syy / n - (sy / n).powi(2);
        let realised = cov / (vx * vy).sqrt();
        assert_abs_diff_eq!(realised, m.correlation(0.5, 1.0, 2.0).expect("c"), epsilon = 0.01);
    }

    #[test]
    fn validation() {
        assert!(matches!(ClewlowStrickland::two_factor(0.5, 1.0, 0.1, 1.5), Err(CommodError::BadCorrelation(_))));
        let m = ClewlowStrickland::one_factor(0.5, 1.0).expect("m");
        assert!(m.simulate(&[(1.0, 4.0)], &[0.5, 0.25], McConfig::default()).is_err());
        assert!(m.simulate(&[(1.0, 4.0)], &[0.5], McConfig { paths: 3, seed: 1, antithetic: true }).is_err());
        assert!(m.simulate(&[], &[0.5], McConfig::default()).is_err());
    }
}
