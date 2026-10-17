//! One- and two-dimensional interpolation on sorted grids.
//!
//! All interpolators extrapolate flat beyond the grid, which is what vol
//! surfaces and most curve consumers want; callers that need slope
//! extrapolation clamp the input themselves.

use thiserror::Error;

#[derive(Debug, Error, Clone, PartialEq)]
pub enum InterpError {
    #[error("need at least {need} points, got {got}")]
    TooFewPoints { need: usize, got: usize },
    #[error("x values must be strictly increasing (index {0})")]
    NotIncreasing(usize),
    #[error("x and y lengths differ: {x} vs {y}")]
    LengthMismatch { x: usize, y: usize },
    #[error("grid is {rows}x{cols} but {values} values were given")]
    GridMismatch { rows: usize, cols: usize, values: usize },
    #[error("non-finite input")]
    NonFinite,
}

fn check_axis(x: &[f64], min_points: usize) -> Result<(), InterpError> {
    if x.len() < min_points {
        return Err(InterpError::TooFewPoints { need: min_points, got: x.len() });
    }
    for (i, w) in x.windows(2).enumerate() {
        if w[1] <= w[0] {
            return Err(InterpError::NotIncreasing(i + 1));
        }
    }
    if x.iter().any(|v| !v.is_finite()) {
        return Err(InterpError::NonFinite);
    }
    Ok(())
}

/// Index `i` of the segment `[x[i], x[i+1]]` containing `v`, clamped to the
/// grid so the ends extrapolate from the outer segments.
fn segment(x: &[f64], v: f64) -> usize {
    let n = x.len();
    match x.partition_point(|&p| p <= v) {
        0 => 0,
        k if k >= n => n - 2,
        k => k - 1,
    }
}

/// Piecewise-linear interpolation with flat extrapolation.
#[derive(Debug, Clone, PartialEq)]
pub struct Linear {
    x: Vec<f64>,
    y: Vec<f64>,
}

impl Linear {
    pub fn new(x: &[f64], y: &[f64]) -> Result<Self, InterpError> {
        if x.len() != y.len() {
            return Err(InterpError::LengthMismatch { x: x.len(), y: y.len() });
        }
        check_axis(x, 1)?;
        if y.iter().any(|v| !v.is_finite()) {
            return Err(InterpError::NonFinite);
        }
        Ok(Self { x: x.to_vec(), y: y.to_vec() })
    }

    pub fn x(&self) -> &[f64] {
        &self.x
    }

    pub fn y(&self) -> &[f64] {
        &self.y
    }

    pub fn eval(&self, v: f64) -> f64 {
        let n = self.x.len();
        if n == 1 || v <= self.x[0] {
            return self.y[0];
        }
        if v >= self.x[n - 1] {
            return self.y[n - 1];
        }
        let i = segment(&self.x, v);
        let w = (v - self.x[i]) / (self.x[i + 1] - self.x[i]);
        self.y[i] + w * (self.y[i + 1] - self.y[i])
    }
}

/// Natural cubic spline (zero second derivative at the ends) with flat
/// extrapolation.
#[derive(Debug, Clone, PartialEq)]
pub struct CubicSpline {
    x: Vec<f64>,
    y: Vec<f64>,
    /// Second derivatives at the knots.
    m: Vec<f64>,
}

impl CubicSpline {
    pub fn new(x: &[f64], y: &[f64]) -> Result<Self, InterpError> {
        if x.len() != y.len() {
            return Err(InterpError::LengthMismatch { x: x.len(), y: y.len() });
        }
        check_axis(x, 2)?;
        if y.iter().any(|v| !v.is_finite()) {
            return Err(InterpError::NonFinite);
        }
        let n = x.len();
        let mut m = vec![0.0; n];
        if n > 2 {
            // Tridiagonal system for the interior second derivatives (Thomas algorithm).
            let k = n - 2;
            let mut a = vec![0.0; k];
            let mut b = vec![0.0; k];
            let mut c = vec![0.0; k];
            let mut r = vec![0.0; k];
            for i in 0..k {
                let h0 = x[i + 1] - x[i];
                let h1 = x[i + 2] - x[i + 1];
                a[i] = h0;
                b[i] = 2.0 * (h0 + h1);
                c[i] = h1;
                r[i] = 6.0 * ((y[i + 2] - y[i + 1]) / h1 - (y[i + 1] - y[i]) / h0);
            }
            for i in 1..k {
                let w = a[i] / b[i - 1];
                b[i] -= w * c[i - 1];
                r[i] -= w * r[i - 1];
            }
            m[k] = r[k - 1] / b[k - 1];
            for i in (0..k - 1).rev() {
                m[i + 1] = (r[i] - c[i] * m[i + 2]) / b[i];
            }
        }
        Ok(Self { x: x.to_vec(), y: y.to_vec(), m })
    }

    pub fn eval(&self, v: f64) -> f64 {
        let n = self.x.len();
        if v <= self.x[0] {
            return self.y[0];
        }
        if v >= self.x[n - 1] {
            return self.y[n - 1];
        }
        let i = segment(&self.x, v);
        let h = self.x[i + 1] - self.x[i];
        let a = (self.x[i + 1] - v) / h;
        let b = (v - self.x[i]) / h;
        a * self.y[i] + b * self.y[i + 1] + ((a * a * a - a) * self.m[i] + (b * b * b - b) * self.m[i + 1]) * h * h / 6.0
    }

    /// First derivative inside the grid; zero outside (flat extrapolation).
    pub fn derivative(&self, v: f64) -> f64 {
        let n = self.x.len();
        if v <= self.x[0] || v >= self.x[n - 1] {
            return 0.0;
        }
        let i = segment(&self.x, v);
        let h = self.x[i + 1] - self.x[i];
        let a = (self.x[i + 1] - v) / h;
        let b = (v - self.x[i]) / h;
        (self.y[i + 1] - self.y[i]) / h - (3.0 * a * a - 1.0) * h * self.m[i] / 6.0 + (3.0 * b * b - 1.0) * h * self.m[i + 1] / 6.0
    }
}

/// Bilinear interpolation on a rectangular grid, flat beyond the edges.
/// `values` is row-major: `values[i * cols + j]` is at `(xs[i], ys[j])`.
#[derive(Debug, Clone, PartialEq)]
pub struct Bilinear {
    xs: Vec<f64>,
    ys: Vec<f64>,
    values: Vec<f64>,
}

impl Bilinear {
    pub fn new(xs: &[f64], ys: &[f64], values: &[f64]) -> Result<Self, InterpError> {
        check_axis(xs, 1)?;
        check_axis(ys, 1)?;
        if values.len() != xs.len() * ys.len() {
            return Err(InterpError::GridMismatch { rows: xs.len(), cols: ys.len(), values: values.len() });
        }
        if values.iter().any(|v| !v.is_finite()) {
            return Err(InterpError::NonFinite);
        }
        Ok(Self { xs: xs.to_vec(), ys: ys.to_vec(), values: values.to_vec() })
    }

    pub fn xs(&self) -> &[f64] {
        &self.xs
    }

    pub fn ys(&self) -> &[f64] {
        &self.ys
    }

    pub fn at(&self, i: usize, j: usize) -> f64 {
        self.values[i * self.ys.len() + j]
    }

    fn weights(axis: &[f64], v: f64) -> (usize, usize, f64) {
        let n = axis.len();
        if n == 1 || v <= axis[0] {
            return (0, 0, 0.0);
        }
        if v >= axis[n - 1] {
            return (n - 1, n - 1, 0.0);
        }
        let i = segment(axis, v);
        (i, i + 1, (v - axis[i]) / (axis[i + 1] - axis[i]))
    }

    pub fn eval(&self, x: f64, y: f64) -> f64 {
        let (i0, i1, wx) = Self::weights(&self.xs, x);
        let (j0, j1, wy) = Self::weights(&self.ys, y);
        let top = self.at(i0, j0) + wy * (self.at(i0, j1) - self.at(i0, j0));
        let bottom = self.at(i1, j0) + wy * (self.at(i1, j1) - self.at(i1, j0));
        top + wx * (bottom - top)
    }

    /// Weights `(i, j, w)` with `Σ w = 1` that `eval(x, y)` uses; the
    /// bucketing hook for node-level vega.
    pub fn node_weights(&self, x: f64, y: f64) -> Vec<(usize, usize, f64)> {
        let (i0, i1, wx) = Self::weights(&self.xs, x);
        let (j0, j1, wy) = Self::weights(&self.ys, y);
        let mut out = Vec::with_capacity(4);
        for (i, wi) in [(i0, 1.0 - wx), (i1, wx)] {
            for (j, wj) in [(j0, 1.0 - wy), (j1, wy)] {
                let w = wi * wj;
                if w != 0.0 {
                    if let Some(e) = out.iter_mut().find(|(a, b, _): &&mut (usize, usize, f64)| *a == i && *b == j) {
                        e.2 += w;
                    } else {
                        out.push((i, j, w));
                    }
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_abs_diff_eq;

    type R = Result<(), InterpError>;

    #[test]
    fn linear_hits_knots_and_extrapolates_flat() -> R {
        let l = Linear::new(&[1.0, 2.0, 4.0], &[10.0, 20.0, 40.0])?;
        assert_abs_diff_eq!(l.eval(1.0), 10.0, epsilon = 1e-15);
        assert_abs_diff_eq!(l.eval(3.0), 30.0, epsilon = 1e-15);
        assert_abs_diff_eq!(l.eval(0.0), 10.0, epsilon = 1e-15);
        assert_abs_diff_eq!(l.eval(9.0), 40.0, epsilon = 1e-15);
        assert!(Linear::new(&[1.0, 1.0], &[0.0, 0.0]).is_err());
        assert!(Linear::new(&[1.0], &[0.0, 0.0]).is_err());
        Ok(())
    }

    #[test]
    fn spline_reproduces_a_cubic_and_is_smooth() -> R {
        // Natural spline is exact for data from a line; close for a smooth curve.
        let x: Vec<f64> = (0..=10).map(|i| i as f64).collect();
        let line: Vec<f64> = x.iter().map(|v| 2.0 * v + 1.0).collect();
        let s = CubicSpline::new(&x, &line)?;
        assert_abs_diff_eq!(s.eval(3.3), 7.6, epsilon = 1e-12);
        assert_abs_diff_eq!(s.derivative(3.3), 2.0, epsilon = 1e-12);

        let sine: Vec<f64> = x.iter().map(|v| (v * 0.5).sin()).collect();
        let s = CubicSpline::new(&x, &sine)?;
        for v in [0.5, 2.25, 7.9] {
            assert_abs_diff_eq!(s.eval(v), (v * 0.5).sin(), epsilon = 3e-3);
            assert_abs_diff_eq!(s.derivative(v), 0.5 * (v * 0.5).cos(), epsilon = 1e-2);
        }
        // Continuity of value and slope at a knot.
        assert_abs_diff_eq!(s.eval(4.0 - 1e-9), s.eval(4.0 + 1e-9), epsilon = 1e-7);
        assert_abs_diff_eq!(s.derivative(4.0 - 1e-9), s.derivative(4.0 + 1e-9), epsilon = 1e-6);
        // Two points degrade to a straight line.
        let two = CubicSpline::new(&[0.0, 1.0], &[0.0, 2.0])?;
        assert_abs_diff_eq!(two.eval(0.25), 0.5, epsilon = 1e-15);
        Ok(())
    }

    #[test]
    fn bilinear_corners_centre_and_weights() -> R {
        let b = Bilinear::new(&[1.0, 2.0], &[10.0, 20.0, 30.0], &[1.0, 2.0, 3.0, 4.0, 5.0, 6.0])?;
        assert_abs_diff_eq!(b.eval(1.0, 10.0), 1.0, epsilon = 1e-15);
        assert_abs_diff_eq!(b.eval(2.0, 30.0), 6.0, epsilon = 1e-15);
        assert_abs_diff_eq!(b.eval(1.5, 15.0), 3.0, epsilon = 1e-15);
        assert_abs_diff_eq!(b.eval(0.0, 100.0), 3.0, epsilon = 1e-15); // flat to (1, 30)
        let w = b.node_weights(1.5, 15.0);
        assert_eq!(w.len(), 4);
        assert_abs_diff_eq!(w.iter().map(|t| t.2).sum::<f64>(), 1.0, epsilon = 1e-15);
        let recon: f64 = w.iter().map(|&(i, j, wt)| wt * b.at(i, j)).sum();
        assert_abs_diff_eq!(recon, b.eval(1.5, 15.0), epsilon = 1e-15);
        assert_eq!(b.node_weights(1.0, 10.0), vec![(0, 0, 1.0)]);
        assert!(Bilinear::new(&[1.0, 2.0], &[1.0], &[1.0]).is_err());
        Ok(())
    }
}
