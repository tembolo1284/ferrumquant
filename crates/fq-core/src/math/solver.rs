//! One-dimensional root finders.
//!
//! - `brent`: derivative-free, guaranteed convergence on a bracket.
//! - `newton_safe`: Newton with bisection fallback inside a bracket
//!   (Numerical Recipes `rtsafe`), for when an analytic derivative is cheap,
//!   e.g. bond yield from price.
//! - `bracket`: expands an initial interval until it brackets a root.

use thiserror::Error;

#[derive(Debug, Error, Clone, PartialEq)]
pub enum SolverError {
    #[error("root not bracketed: f({lo}) = {f_lo}, f({hi}) = {f_hi}")]
    NotBracketed { lo: f64, hi: f64, f_lo: f64, f_hi: f64 },
    #[error("no convergence after {iterations} iterations (last x = {last})")]
    MaxIterations { iterations: usize, last: f64 },
    #[error("objective returned a non-finite value at x = {0}")]
    NonFinite(f64),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SolverConfig {
    /// Absolute tolerance on x.
    pub tolerance: f64,
    pub max_iterations: usize,
}

impl Default for SolverConfig {
    fn default() -> Self {
        Self { tolerance: 1e-12, max_iterations: 100 }
    }
}

fn finite(x: f64, fx: f64) -> Result<f64, SolverError> {
    if fx.is_finite() { Ok(fx) } else { Err(SolverError::NonFinite(x)) }
}

fn check_bracket(lo: f64, hi: f64, f_lo: f64, f_hi: f64) -> Result<(), SolverError> {
    if (f_lo > 0.0 && f_hi > 0.0) || (f_lo < 0.0 && f_hi < 0.0) {
        Err(SolverError::NotBracketed { lo, hi, f_lo, f_hi })
    } else {
        Ok(())
    }
}

/// Brent's method on [lo, hi]; `f(lo)` and `f(hi)` must differ in sign.
#[allow(clippy::float_cmp)] // exact comparisons are part of the algorithm
pub fn brent(mut f: impl FnMut(f64) -> f64, lo: f64, hi: f64, cfg: SolverConfig) -> Result<f64, SolverError> {
    let (mut a, mut b) = (lo, hi);
    let mut fa = finite(a, f(a))?;
    let mut fb = finite(b, f(b))?;
    if fa == 0.0 {
        return Ok(a);
    }
    if fb == 0.0 {
        return Ok(b);
    }
    check_bracket(a, b, fa, fb)?;

    let (mut c, mut fc) = (b, fb);
    let mut d = b - a;
    let mut e = d;

    for _ in 0..cfg.max_iterations {
        if (fb > 0.0 && fc > 0.0) || (fb < 0.0 && fc < 0.0) {
            c = a;
            fc = fa;
            d = b - a;
            e = d;
        }
        if fc.abs() < fb.abs() {
            a = b;
            b = c;
            c = a;
            fa = fb;
            fb = fc;
            fc = fa;
        }
        let tol1 = 2.0 * f64::EPSILON * b.abs() + 0.5 * cfg.tolerance;
        let xm = 0.5 * (c - b);
        if xm.abs() <= tol1 || fb == 0.0 {
            return Ok(b);
        }
        if e.abs() >= tol1 && fa.abs() > fb.abs() {
            // Inverse quadratic interpolation (or secant if a == c).
            let s = fb / fa;
            let (mut p, mut q);
            if a == c {
                p = 2.0 * xm * s;
                q = 1.0 - s;
            } else {
                let q0 = fa / fc;
                let r = fb / fc;
                p = s * (2.0 * xm * q0 * (q0 - r) - (b - a) * (r - 1.0));
                q = (q0 - 1.0) * (r - 1.0) * (s - 1.0);
            }
            if p > 0.0 {
                q = -q;
            }
            p = p.abs();
            let min1 = 3.0 * xm * q - (tol1 * q).abs();
            let min2 = (e * q).abs();
            if 2.0 * p < min1.min(min2) {
                e = d;
                d = p / q;
            } else {
                d = xm;
                e = d;
            }
        } else {
            d = xm;
            e = d;
        }
        a = b;
        fa = fb;
        b += if d.abs() > tol1 { d } else { tol1.copysign(xm) };
        fb = finite(b, f(b))?;
    }
    Err(SolverError::MaxIterations { iterations: cfg.max_iterations, last: b })
}

/// Safeguarded Newton on [lo, hi]. `f` returns (value, derivative).
/// Falls back to bisection when a Newton step leaves the bracket or stalls.
#[allow(clippy::float_cmp)]
pub fn newton_safe(
    mut f: impl FnMut(f64) -> (f64, f64),
    guess: f64,
    lo: f64,
    hi: f64,
    cfg: SolverConfig,
) -> Result<f64, SolverError> {
    let f_lo = finite(lo, f(lo).0)?;
    let f_hi = finite(hi, f(hi).0)?;
    if f_lo == 0.0 {
        return Ok(lo);
    }
    if f_hi == 0.0 {
        return Ok(hi);
    }
    check_bracket(lo, hi, f_lo, f_hi)?;

    // Orient so that f(xl) < 0 < f(xh).
    let (mut xl, mut xh) = if f_lo < 0.0 { (lo, hi) } else { (hi, lo) };
    let mut x = guess.clamp(lo.min(hi), lo.max(hi));
    let mut dx_old = (hi - lo).abs();
    let mut dx = dx_old;
    let (mut fx, mut dfx) = f(x);
    finite(x, fx)?;

    for _ in 0..cfg.max_iterations {
        let newton_out_of_range = ((x - xh) * dfx - fx) * ((x - xl) * dfx - fx) > 0.0;
        let newton_too_slow = (2.0 * fx).abs() > (dx_old * dfx).abs();
        if newton_out_of_range || newton_too_slow {
            dx_old = dx;
            dx = 0.5 * (xh - xl);
            x = xl + dx;
            if xl == x {
                return Ok(x);
            }
        } else {
            dx_old = dx;
            dx = fx / dfx;
            let prev = x;
            x -= dx;
            if prev == x {
                return Ok(x);
            }
        }
        if dx.abs() < cfg.tolerance {
            return Ok(x);
        }
        (fx, dfx) = f(x);
        finite(x, fx)?;
        if fx < 0.0 {
            xl = x;
        } else {
            xh = x;
        }
    }
    Err(SolverError::MaxIterations { iterations: cfg.max_iterations, last: x })
}

/// Geometrically expand [lo, hi] (factor 1.6) until it brackets a root.
pub fn bracket(
    mut f: impl FnMut(f64) -> f64,
    lo: f64,
    hi: f64,
    max_expansions: usize,
) -> Result<(f64, f64), SolverError> {
    const FACTOR: f64 = 1.6;
    let (mut a, mut b) = (lo, hi);
    let mut fa = finite(a, f(a))?;
    let mut fb = finite(b, f(b))?;
    for _ in 0..max_expansions {
        if fa * fb <= 0.0 {
            return Ok((a, b));
        }
        if fa.abs() < fb.abs() {
            a += FACTOR * (a - b);
            fa = finite(a, f(a))?;
        } else {
            b += FACTOR * (b - a);
            fb = finite(b, f(b))?;
        }
    }
    check_bracket(a, b, fa, fb)?;
    Ok((a, b))
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_abs_diff_eq;

    type R = Result<(), SolverError>;
    const CFG: SolverConfig = SolverConfig { tolerance: 1e-14, max_iterations: 100 };

    #[test]
    fn brent_sqrt2() -> R {
        let x = brent(|x| x * x - 2.0, 0.0, 2.0, CFG)?;
        assert_abs_diff_eq!(x, 2f64.sqrt(), epsilon = 1e-13);
        Ok(())
    }

    #[test]
    fn brent_cubic_with_flat_region() -> R {
        // A triple root defeats interpolation, so Brent degrades to bisection
        // and needs ~145 iterations at this tolerance.
        let cfg = SolverConfig { tolerance: 1e-14, max_iterations: 200 };
        let x = brent(|x| (x - 1.0).powi(3), -3.0, 4.0, cfg)?;
        assert_abs_diff_eq!(x, 1.0, epsilon = 1e-4);
        Ok(())
    }

    #[test]
    fn newton_safe_sqrt2() -> R {
        let x = newton_safe(|x| (x * x - 2.0, 2.0 * x), 1.0, 0.0, 2.0, CFG)?;
        assert_abs_diff_eq!(x, 2f64.sqrt(), epsilon = 1e-13);
        Ok(())
    }

    #[test]
    fn newton_safe_survives_bad_guess() -> R {
        // Derivative ~0 near the guess would send plain Newton far away.
        let x = newton_safe(|x| (x.atan(), 1.0 / (1.0 + x * x)), 9.0, -10.0, 10.0, CFG)?;
        assert_abs_diff_eq!(x, 0.0, epsilon = 1e-12);
        Ok(())
    }

    #[test]
    fn not_bracketed_is_an_error() {
        let r = brent(|x| x * x + 1.0, -1.0, 1.0, CFG);
        assert!(matches!(r, Err(SolverError::NotBracketed { .. })));
    }

    #[test]
    fn bracket_expands() -> R {
        let (a, b) = bracket(|x| x - 50.0, 0.0, 1.0, 50)?;
        assert!(a <= 50.0 && 50.0 <= b);
        Ok(())
    }
}
