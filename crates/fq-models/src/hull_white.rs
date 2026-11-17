//! Hull-White one-factor short-rate model, fitted to a discount curve.
//!
//! ```text
//! dr = (theta(t) - a r) dt + sigma dW
//! ```
//!
//! The implementation never evaluates `theta` or the instantaneous forward
//! curve. Writing `r(t) = x(t) + alpha(t)` with `x` the zero-mean
//! Ornstein-Uhlenbeck factor (`x(0) = 0`) and `alpha` the deterministic fit
//! to the curve, every bond price is
//!
//! ```text
//! P(t, T | x) = P(0, T) / P(0, t) * exp(-B(t,T) x - B(t,T) y(t) - B(t,T)^2 v(t) / 2)
//! B(t, T) = (1 - e^{-a (T - t)}) / a
//! v(t)    = sigma^2 (1 - e^{-2 a t}) / (2 a)          variance of x(t)
//! y(t)    = sigma^2 (1 - e^{-a t})^2 / (2 a^2)         alpha(t) - f(0, t)
//! ```
//!
//! which depends on the curve only through discount factors. The same
//! factor drives the analytic ZCB options and caplets here, the swaption
//! formulas in `swaption`, the trinomial tree in `tree` and the exact
//! simulation in `simulation`, so all four agree by construction.
//!
//! Under the `T`-forward measure `x(T)` is Gaussian with mean `-y(T)` and
//! variance `v(T)`; that is what prices anything paid at `T`.

use fq_core::math::{OptionType, norm_cdf};
use fq_core::time::Date;
use fq_rates::curve::DiscountCurve;

use crate::error::ModelError;

/// Cap or floor side of a caplet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CapletKind {
    Caplet,
    Floorlet,
}

impl CapletKind {
    fn sign(self) -> f64 {
        match self {
            Self::Caplet => 1.0,
            Self::Floorlet => -1.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct HullWhite {
    curve: DiscountCurve,
    a: f64,
    sigma: f64,
}

impl HullWhite {
    /// `a` is the mean-reversion speed, `sigma` the absolute short-rate vol
    /// (e.g. 0.01 for 100bp); both must be positive.
    pub fn new(curve: DiscountCurve, a: f64, sigma: f64) -> Result<Self, ModelError> {
        if !(a.is_finite() && a > 0.0) {
            return Err(ModelError::BadMeanReversion(a));
        }
        if !(sigma.is_finite() && sigma > 0.0) {
            return Err(ModelError::BadVolatility(sigma));
        }
        Ok(Self { curve, a, sigma })
    }

    pub fn curve(&self) -> &DiscountCurve {
        &self.curve
    }

    pub fn mean_reversion(&self) -> f64 {
        self.a
    }

    pub fn sigma(&self) -> f64 {
        self.sigma
    }

    /// Same curve, new parameters.
    pub fn with_params(&self, a: f64, sigma: f64) -> Result<Self, ModelError> {
        Self::new(self.curve.clone(), a, sigma)
    }

    /// ACT/365F years from the curve's reference date.
    pub fn time(&self, d: Date) -> f64 {
        self.curve.time(d)
    }

    /// `(1 - e^{-a tau}) / a`.
    pub fn b(&self, t: f64, big_t: f64) -> f64 {
        (1.0 - (-self.a * (big_t - t)).exp()) / self.a
    }

    /// Variance of the factor `x(t)` (and of the short rate) at `t`.
    pub fn variance(&self, t: f64) -> f64 {
        self.sigma * self.sigma * (1.0 - (-2.0 * self.a * t).exp()) / (2.0 * self.a)
    }

    /// `alpha(t) - f(0, t) = sigma^2 (1 - e^{-a t})^2 / (2 a^2)`.
    pub fn y(&self, t: f64) -> f64 {
        let e = 1.0 - (-self.a * t).exp();
        self.sigma * self.sigma * e * e / (2.0 * self.a * self.a)
    }

    /// `int_0^t y(u) du`, closed form.
    pub fn int_y(&self, t: f64) -> f64 {
        let a = self.a;
        self.sigma * self.sigma / (2.0 * a * a) * (t - 2.0 * (1.0 - (-a * t).exp()) / a + (1.0 - (-2.0 * a * t).exp()) / (2.0 * a))
    }

    /// Mean of `x(T)` under the `T`-forward measure.
    pub fn forward_measure_mean(&self, big_t: f64) -> f64 {
        -self.y(big_t)
    }

    /// Zero-coupon bond price `P(t, T)` given the factor `x(t) = x`.
    pub fn bond(&self, t: f64, big_t: f64, x: f64) -> Result<f64, ModelError> {
        if !(t >= 0.0 && big_t >= t) {
            return Err(ModelError::BadTimes { t, big_t });
        }
        Ok(self.bond_unchecked(t, big_t, x))
    }

    pub(crate) fn bond_unchecked(&self, t: f64, big_t: f64, x: f64) -> f64 {
        let (c, beta) = self.bond_affine(t, big_t);
        c * (-beta * x).exp()
    }

    /// `P(t, T | x) = c * exp(-beta x)`: the constant and exponent.
    pub(crate) fn bond_affine(&self, t: f64, big_t: f64) -> (f64, f64) {
        let b = self.b(t, big_t);
        let ln_c = self.curve.discount_t(big_t).ln() - self.curve.discount_t(t).ln() - b * self.y(t) - 0.5 * b * b * self.variance(t);
        (ln_c.exp(), b)
    }

    /// Instantaneous forward `f(0, t)` from the curve by central difference
    /// (one-sided at `t = 0`). Only `alpha` and `short_rate_mean` need it;
    /// pricing does not.
    pub fn instantaneous_forward(&self, t: f64) -> f64 {
        const H: f64 = 1.0 / 3650.0;
        let lo = (t - H).max(0.0);
        let hi = t + H;
        -(self.curve.discount_t(hi).ln() - self.curve.discount_t(lo).ln()) / (hi - lo)
    }

    /// `alpha(t) = f(0, t) + y(t)`: the deterministic shift that fits the curve.
    pub fn alpha(&self, t: f64) -> f64 {
        self.instantaneous_forward(t) + self.y(t)
    }

    /// Risk-neutral mean of the short rate at `t`.
    pub fn short_rate_mean(&self, t: f64) -> f64 {
        self.alpha(t)
    }

    /// Option expiring at `t` on the zero-coupon bond maturing at `big_t`,
    /// strike `k` per unit face. Jamshidian's closed form.
    pub fn zcb_option(&self, kind: OptionType, t: f64, big_t: f64, k: f64) -> Result<f64, ModelError> {
        if !(t > 0.0 && big_t > t) {
            return Err(ModelError::BadTimes { t, big_t });
        }
        if !(k.is_finite() && k > 0.0) {
            return Err(ModelError::BadStrike(k));
        }
        let p_t = self.curve.discount_t(t);
        let p_big = self.curve.discount_t(big_t);
        let sigma_p = self.b(t, big_t) * self.variance(t).sqrt();
        let h = (p_big / (k * p_t)).ln() / sigma_p + 0.5 * sigma_p;
        Ok(match kind {
            OptionType::Call => p_big * norm_cdf(h) - k * p_t * norm_cdf(h - sigma_p),
            OptionType::Put => k * p_t * norm_cdf(-h + sigma_p) - p_big * norm_cdf(-h),
        })
    }

    /// Caplet or floorlet on the daily-compounded (backward-looking) rate over
    /// `[start, end]`, year fraction `tau`, paid at `pay >= end`, per unit
    /// notional:
    ///
    /// ```text
    /// tau * max(R - K, 0),   1 + tau R = exp(int_start^end r du)
    /// ```
    ///
    /// Exact in the model: the payoff is an exchange of two lognormals,
    /// `exp(-int_0^start r)` against `(1 + K tau) exp(-int_0^end r)`, whose
    /// log-variance is `B(tau)^2 v(start)` (the forward-looking part) plus
    /// the variance accrued inside the period. A payment lag beyond `end`
    /// is a deterministic `P(0, pay) / P(0, end)` factor.
    pub fn compounded_caplet(&self, kind: CapletKind, start: f64, end: f64, pay: f64, strike: f64, tau: f64) -> Result<f64, ModelError> {
        self.caplet_impl(kind, start, end, pay, strike, tau, true)
    }

    /// Caplet or floorlet on the forward-looking term rate for `[start, end]`
    /// (set at `start`), equivalent to `(1 + K tau)` ZCB puts struck at
    /// `1 / (1 + K tau)`. Provided for comparison with the compounded one.
    pub fn term_caplet(&self, kind: CapletKind, start: f64, end: f64, pay: f64, strike: f64, tau: f64) -> Result<f64, ModelError> {
        self.caplet_impl(kind, start, end, pay, strike, tau, false)
    }

    #[allow(clippy::too_many_arguments)]
    fn caplet_impl(&self, kind: CapletKind, start: f64, end: f64, pay: f64, strike: f64, tau: f64, in_arrears: bool) -> Result<f64, ModelError> {
        if !(start >= 0.0 && end > start && pay >= end) {
            return Err(ModelError::BadTimes { t: start, big_t: end });
        }
        if !(strike.is_finite() && tau > 0.0) {
            return Err(ModelError::BadStrike(strike));
        }
        let p_s = self.curve.discount_t(start);
        let p_e = self.curve.discount_t(end);
        let lag = self.curve.discount_t(pay) / p_e;
        let c = 1.0 + strike * tau;
        let period = end - start;
        let b = self.b(0.0, period);
        let mut var = b * b * self.variance(start);
        if in_arrears {
            let a = self.a;
            var += self.sigma * self.sigma / (a * a) * (period - 2.0 * b + (1.0 - (-2.0 * a * period).exp()) / (2.0 * a));
        }
        if var <= 0.0 {
            // No randomness left (start = 0 and forward-looking): intrinsic.
            let intrinsic = (kind.sign() * (p_s - c * p_e)).max(0.0);
            return Ok(lag * intrinsic);
        }
        let s = var.sqrt();
        let d1 = ((p_s / (c * p_e)).ln() + 0.5 * var) / s;
        let d2 = d1 - s;
        let w = kind.sign();
        Ok(lag * w * (p_s * norm_cdf(w * d1) - c * p_e * norm_cdf(w * d2)))
    }

    /// Dated wrapper for `compounded_caplet`.
    pub fn compounded_caplet_on(&self, kind: CapletKind, start: Date, end: Date, pay: Date, strike: f64, tau: f64) -> Result<f64, ModelError> {
        if end <= start {
            return Err(ModelError::BadAccrual { start, end });
        }
        self.compounded_caplet(kind, self.time(start), self.time(end), self.time(pay), strike, tau)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_abs_diff_eq;
    use fq_core::time::Date;

    type R = Result<(), ModelError>;

    fn d(y: i32, m: u32, dd: u32) -> Date {
        Date::from_ymd(y, m, dd).expect("valid test date")
    }

    pub(crate) fn flat_model(a: f64, sigma: f64) -> HullWhite {
        let today = d(2026, 10, 13);
        let curve = DiscountCurve::flat(today, 0.04, d(2066, 10, 13)).expect("flat curve");
        HullWhite::new(curve, a, sigma).expect("model")
    }

    pub(crate) fn sloped_model(a: f64, sigma: f64) -> HullWhite {
        let today = d(2026, 10, 13);
        let nodes = [(d(2027, 10, 13), 0.035), (d(2029, 10, 13), 0.038), (d(2031, 10, 13), 0.040), (d(2036, 10, 13), 0.042), (d(2056, 10, 13), 0.043)];
        let curve = DiscountCurve::from_zero_rates(today, &nodes, Default::default()).expect("curve");
        HullWhite::new(curve, a, sigma).expect("model")
    }

    #[test]
    fn construction() {
        let m = flat_model(0.05, 0.01);
        assert_abs_diff_eq!(m.mean_reversion(), 0.05, epsilon = 0.0);
        assert_abs_diff_eq!(m.sigma(), 0.01, epsilon = 0.0);
        assert!(m.with_params(0.0, 0.01).is_err());
        assert!(m.with_params(0.1, -0.01).is_err());
        assert!(m.with_params(f64::NAN, 0.01).is_err());
    }

    #[test]
    fn bond_at_zero_is_the_curve() -> R {
        let m = sloped_model(0.05, 0.01);
        for t in [0.5, 1.0, 3.0, 7.0, 20.0] {
            assert_abs_diff_eq!(m.bond(0.0, t, 0.0)?, m.curve().discount_t(t), epsilon = 1e-15);
        }
        assert_abs_diff_eq!(m.bond(2.0, 2.0, 0.3)?, 1.0, epsilon = 1e-15);
        assert!(m.bond(3.0, 2.0, 0.0).is_err());
        Ok(())
    }

    #[test]
    fn forward_measure_reproduces_the_curve() -> R {
        // E^T[P(T, S | x_T)] = P(0, S) / P(0, T) with x_T ~ N(-y(T), v(T)).
        let m = sloped_model(0.03, 0.012);
        for (t, s) in [(1.0, 2.0), (2.0, 7.0), (5.0, 30.0)] {
            let (c, beta) = m.bond_affine(t, s);
            let mean = m.forward_measure_mean(t);
            let expectation = c * (-beta * mean + 0.5 * beta * beta * m.variance(t)).exp();
            assert_abs_diff_eq!(expectation, m.curve().discount_t(s) / m.curve().discount_t(t), epsilon = 1e-14);
        }
        Ok(())
    }

    #[test]
    fn risk_neutral_bank_account_reproduces_the_curve() {
        // E^Q[exp(-int_0^t r)] = P(0, t) with int r = int x + int alpha and
        // int alpha = -ln P(0, t) + int y. Var(int_0^t x) is the closed form.
        let m = sloped_model(0.05, 0.01);
        for t in [1.0, 5.0, 12.0] {
            let a = m.mean_reversion();
            let var_int_x = m.sigma().powi(2) / (a * a) * (t - 2.0 * m.b(0.0, t) + (1.0 - (-2.0 * a * t).exp()) / (2.0 * a));
            let expectation = m.curve().discount_t(t) * (-m.int_y(t) + 0.5 * var_int_x).exp();
            assert_abs_diff_eq!(expectation, m.curve().discount_t(t), epsilon = 1e-14);
        }
    }

    #[test]
    fn alpha_on_a_flat_curve() {
        let m = flat_model(0.1, 0.01);
        assert_abs_diff_eq!(m.instantaneous_forward(3.0), 0.04, epsilon = 1e-9);
        assert_abs_diff_eq!(m.alpha(3.0), 0.04 + m.y(3.0), epsilon = 1e-9);
        assert_abs_diff_eq!(m.short_rate_mean(0.0), 0.04, epsilon = 1e-9);
        assert!(m.y(3.0) > 0.0);
    }

    #[test]
    fn zcb_option_parity_and_limits() -> R {
        let m = sloped_model(0.05, 0.01);
        let (t, s, k) = (2.0, 7.0, 0.80);
        let call = m.zcb_option(OptionType::Call, t, s, k)?;
        let put = m.zcb_option(OptionType::Put, t, s, k)?;
        assert_abs_diff_eq!(call - put, m.curve().discount_t(s) - k * m.curve().discount_t(t), epsilon = 1e-15);
        assert!(call > 0.0 && put > 0.0);
        // Tiny vol: intrinsic on the forward bond price.
        let tiny = m.with_params(0.05, 1e-6)?;
        let fwd = m.curve().discount_t(s) / m.curve().discount_t(t);
        assert_abs_diff_eq!(tiny.zcb_option(OptionType::Call, t, s, k)?, m.curve().discount_t(t) * (fwd - k).max(0.0), epsilon = 1e-9);
        // More vol, more value.
        assert!(m.with_params(0.05, 0.02)?.zcb_option(OptionType::Put, t, s, k)? > put);
        assert!(m.zcb_option(OptionType::Call, 0.0, s, k).is_err());
        assert!(m.zcb_option(OptionType::Call, t, s, 0.0).is_err());
        Ok(())
    }

    #[test]
    fn caplet_parity_and_ordering() -> R {
        let m = sloped_model(0.05, 0.01);
        let (s, e, p, k, tau) = (1.0, 1.25, 1.25 + 2.0 / 365.0, 0.04, 91.0 / 360.0);
        let cap = m.compounded_caplet(CapletKind::Caplet, s, e, p, k, tau)?;
        let floor = m.compounded_caplet(CapletKind::Floorlet, s, e, p, k, tau)?;
        let p_s = m.curve().discount_t(s);
        let p_e = m.curve().discount_t(e);
        let lag = m.curve().discount_t(p) / p_e;
        // cap - floor = P(s) - (1 + K tau) P(e), the forward swaplet.
        assert_abs_diff_eq!(cap - floor, lag * (p_s - (1.0 + k * tau) * p_e), epsilon = 1e-15);
        // The compounded (in-arrears) caplet carries more variance than the term one.
        let term = m.term_caplet(CapletKind::Caplet, s, e, p, k, tau)?;
        assert!(cap > term, "{cap} vs {term}");
        // And the term caplet is (1 + K tau) ZCB puts struck at 1 / (1 + K tau).
        let c = 1.0 + k * tau;
        assert_abs_diff_eq!(term, lag * c * m.zcb_option(OptionType::Put, s, e, 1.0 / c)?, epsilon = 1e-15);
        // Dated wrapper agrees.
        let today = m.curve().reference_date();
        let ds = today.checked_add_days(365).expect("date");
        let de = ds.checked_add_days(91).expect("date");
        let dp = de.checked_add_days(2).expect("date");
        let dated = m.compounded_caplet_on(CapletKind::Caplet, ds, de, dp, k, 91.0 / 360.0)?;
        let direct = m.compounded_caplet(CapletKind::Caplet, m.time(ds), m.time(de), m.time(dp), k, 91.0 / 360.0)?;
        assert_abs_diff_eq!(dated, direct, epsilon = 1e-15);
        assert!(m.compounded_caplet_on(CapletKind::Caplet, de, ds, dp, k, tau).is_err());
        assert!(m.compounded_caplet(CapletKind::Caplet, s, e, e - 0.01, k, tau).is_err());
        Ok(())
    }

    #[test]
    fn caplet_starting_today_is_still_random_when_compounded() -> R {
        let m = flat_model(0.05, 0.01);
        let (e, k, tau) = (0.25, 0.04, 0.25);
        let term = m.term_caplet(CapletKind::Caplet, 0.0, e, e, k, tau)?;
        let p_e = m.curve().discount_t(e);
        assert_abs_diff_eq!(term, (1.0 - (1.0 + k * tau) * p_e).max(0.0), epsilon = 1e-15);
        assert!(m.compounded_caplet(CapletKind::Caplet, 0.0, e, e, k, tau)? > term);
        Ok(())
    }
}
