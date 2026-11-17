//! Derivative-free minimisation.
//!
//! `nelder_mead` is the simplex method in any dimension, used by the model
//! calibrations (Hull-White to swaptions) where the objective is a sum of
//! squared vol errors and gradients are not worth the code. The SABR
//! calibration in `sabr` keeps its own fixed-size copy.

/// Why the minimiser stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Termination {
    /// The simplex collapsed: objective spread below tolerance.
    Converged,
    MaxIterations,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Minimum {
    pub x: Vec<f64>,
    pub value: f64,
    pub iterations: usize,
    pub termination: Termination,
}

impl Minimum {
    pub fn converged(&self) -> bool {
        self.termination == Termination::Converged
    }
}

/// Nelder-Mead simplex minimiser. `start` seeds the simplex and `step[i]`
/// is the initial displacement along coordinate `i`; both must have the
/// same length. The objective may return `INFINITY` to reject a point.
/// Stops when the spread of objective values over the simplex is below
/// `tol * (1 + |best|)` or after `max_iter` iterations.
pub fn nelder_mead(f: impl Fn(&[f64]) -> f64, start: &[f64], step: &[f64], max_iter: usize, tol: f64) -> Minimum {
    let n = start.len();
    assert_eq!(n, step.len(), "start and step must have the same length");
    let mut simplex: Vec<(Vec<f64>, f64)> = Vec::with_capacity(n + 1);
    simplex.push((start.to_vec(), f(start)));
    for i in 0..n {
        let mut p = start.to_vec();
        p[i] += step[i];
        let v = f(&p);
        simplex.push((p, v));
    }
    let (alpha, gamma, rho, sigma) = (1.0, 2.0, 0.5, 0.5);
    let mut iter = 0;
    let mut termination = Termination::MaxIterations;
    let cmp = |a: &(Vec<f64>, f64), b: &(Vec<f64>, f64)| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal);
    while iter < max_iter {
        iter += 1;
        simplex.sort_by(cmp);
        let spread = simplex[n].1 - simplex[0].1;
        if spread.abs() < tol * (1.0 + simplex[0].1.abs()) {
            termination = Termination::Converged;
            break;
        }
        let mut centroid = vec![0.0; n];
        for p in &simplex[..n] {
            for (c, x) in centroid.iter_mut().zip(&p.0) {
                *c += x / n as f64;
            }
        }
        let worst = simplex[n].clone();
        let reflect = |c: f64| -> Vec<f64> { centroid.iter().zip(&worst.0).map(|(ci, wi)| ci + c * (ci - wi)).collect() };
        let xr = reflect(alpha);
        let fr = f(&xr);
        if fr < simplex[0].1 {
            let xe = reflect(gamma);
            let fe = f(&xe);
            simplex[n] = if fe < fr { (xe, fe) } else { (xr, fr) };
        } else if fr < simplex[n - 1].1 {
            simplex[n] = (xr, fr);
        } else {
            let xc = if fr < worst.1 { reflect(rho) } else { reflect(-rho) };
            let fc = f(&xc);
            if fc < fr.min(worst.1) {
                simplex[n] = (xc, fc);
            } else {
                let best = simplex[0].0.clone();
                for p in simplex.iter_mut().skip(1) {
                    for (pi, bi) in p.0.iter_mut().zip(&best) {
                        *pi = bi + sigma * (*pi - bi);
                    }
                    p.1 = f(&p.0);
                }
            }
        }
    }
    simplex.sort_by(cmp);
    let (x, value) = simplex.swap_remove(0);
    Minimum { x, value, iterations: iter, termination }
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_abs_diff_eq;

    #[test]
    fn rosenbrock_2d() {
        let f = |p: &[f64]| (1.0 - p[0]).powi(2) + 100.0 * (p[1] - p[0] * p[0]).powi(2);
        let m = nelder_mead(f, &[-1.2, 1.0], &[0.5, 0.5], 5000, 1e-16);
        assert!(m.converged());
        assert_abs_diff_eq!(m.x[0], 1.0, epsilon = 1e-5);
        assert_abs_diff_eq!(m.x[1], 1.0, epsilon = 1e-5);
    }

    #[test]
    fn one_dimension_and_rejected_points() {
        let f = |p: &[f64]| if p[0] < 0.0 { f64::INFINITY } else { (p[0] - 2.0).powi(2) };
        let m = nelder_mead(f, &[5.0], &[1.0], 500, 1e-16);
        assert_abs_diff_eq!(m.x[0], 2.0, epsilon = 1e-6);
        assert!(m.iterations < 500);
    }

    #[test]
    fn max_iterations_reported() {
        let m = nelder_mead(|p: &[f64]| p[0] * p[0] + p[1] * p[1], &[3.0, 4.0], &[1.0, 1.0], 3, 1e-16);
        assert_eq!(m.termination, Termination::MaxIterations);
        assert_eq!(m.iterations, 3);
    }
}
