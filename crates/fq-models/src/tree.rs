//! Hull-White trinomial tree on the factor `x` (Hull & White 1994).
//!
//! Stage one builds the recombining lattice for the zero-mean
//! Ornstein-Uhlenbeck factor: from node `x_j` at slice `i` the mean of the
//! next value is `x_j e^{-a dt}` and its variance `v = sigma^2 (1 - e^{-2 a dt}) / (2a)`;
//! the next slice's spacing is `dx' = sqrt(3 v)` and the branch is centred on
//! the node nearest the mean, with probabilities matching mean and variance.
//! Centring the branch is what truncates the lattice: once
//! `|j| (1 - e^{-a dt}) > 1/2` the branch from node `j` is pulled back
//! toward zero, so the width stops growing at `|j| = 1 / (2 a dt)`.
//! Time steps may be non-uniform, so exercise dates land exactly on slices.
//!
//! Stage two fits the curve: the one-period discount at node `(i, j)` is
//! `exp(-(alpha_i + x_j) dt_i)` with `alpha_i` chosen, slice by slice through
//! the Arrow-Debreu prices, so that the tree reprices `P(0, t_{i+1})` exactly.

use crate::error::ModelError;
use crate::hull_white::HullWhite;

#[derive(Debug, Clone, PartialEq)]
struct Slice {
    time: f64,
    dx: f64,
    /// Node index range `j_min..=j_max` (x = j dx).
    j_min: i64,
    j_max: i64,
    /// Per node (offset by j_min): centre index of the branch at the next slice.
    centre: Vec<i64>,
    /// Per node: (p_up, p_mid, p_down).
    probs: Vec<(f64, f64, f64)>,
    /// Fitted shift for the one-period discount from this slice.
    alpha: f64,
    /// Arrow-Debreu prices.
    arrow_debreu: Vec<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TrinomialTree {
    slices: Vec<Slice>,
}

impl TrinomialTree {
    /// Builds the tree out to the last of `mandatory_times`, with roughly
    /// `steps_per_year` slices per year and every mandatory time on a slice.
    pub fn build(model: &HullWhite, mandatory_times: &[f64], steps_per_year: usize) -> Result<Self, ModelError> {
        if steps_per_year == 0 {
            return Err(ModelError::BadTreeSteps(steps_per_year));
        }
        let mut must: Vec<f64> = mandatory_times.iter().copied().filter(|t| *t > 0.0).collect();
        must.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        must.dedup();
        if must.is_empty() {
            return Err(ModelError::BadTreeSteps(steps_per_year));
        }
        // Time grid: subdivide each gap between mandatory times evenly.
        let mut times = vec![0.0];
        let mut prev = 0.0;
        for &t in &must {
            let n = ((t - prev) * steps_per_year as f64).ceil().max(1.0) as usize;
            for k in 1..=n {
                times.push(prev + (t - prev) * k as f64 / n as f64);
            }
            prev = t;
        }

        let a = model.mean_reversion();
        let sigma = model.sigma();
        let n_slices = times.len();
        let mut slices: Vec<Slice> = Vec::with_capacity(n_slices);
        // Slice 0: single node at x = 0. Its dx is irrelevant but set for completeness.
        let dt0 = times[1] - times[0];
        let v0 = sigma * sigma * (1.0 - (-2.0 * a * dt0).exp()) / (2.0 * a);
        slices.push(Slice { time: 0.0, dx: (3.0 * v0).sqrt(), j_min: 0, j_max: 0, centre: vec![], probs: vec![], alpha: 0.0, arrow_debreu: vec![1.0] });

        // Stage one: geometry and probabilities.
        for i in 0..n_slices - 1 {
            let dt = times[i + 1] - times[i];
            let v = sigma * sigma * (1.0 - (-2.0 * a * dt).exp()) / (2.0 * a);
            let dx_next = (3.0 * v).sqrt();
            let decay = (-a * dt).exp();
            let cur = &slices[i];
            let n_nodes = (cur.j_max - cur.j_min + 1) as usize;
            let mut centre = Vec::with_capacity(n_nodes);
            let mut probs = Vec::with_capacity(n_nodes);
            let (mut next_min, mut next_max) = (i64::MAX, i64::MIN);
            for j in cur.j_min..=cur.j_max {
                let x = j as f64 * cur.dx;
                let mean = x * decay;
                let k = (mean / dx_next).round() as i64;
                // Offset of the mean from the branch centre as a fraction of the
                // spacing, in [-1/2, 1/2]; matching mean and variance gives
                // pu - pd = e and pu + pd = 1/3 + e^2.
                let e = (mean - k as f64 * dx_next) / dx_next;
                let e2 = e * e;
                probs.push(((1.0 + 3.0 * e2 + 3.0 * e) / 6.0, (2.0 - 3.0 * e2) / 3.0, (1.0 + 3.0 * e2 - 3.0 * e) / 6.0));
                centre.push(k);
                next_min = next_min.min(k - 1);
                next_max = next_max.max(k + 1);
            }
            slices[i].centre = centre;
            slices[i].probs = probs;
            let n_next = (next_max - next_min + 1) as usize;
            slices.push(Slice {
                time: times[i + 1],
                dx: dx_next,
                j_min: next_min,
                j_max: next_max,
                centre: vec![],
                probs: vec![],
                alpha: 0.0,
                arrow_debreu: vec![0.0; n_next],
            });
        }

        // Stage two: alphas and Arrow-Debreu prices.
        for i in 0..n_slices - 1 {
            let dt = times[i + 1] - times[i];
            let target = model.curve().discount_t(times[i + 1]);
            let (j_min, dx) = (slices[i].j_min, slices[i].dx);
            let sum: f64 = slices[i].arrow_debreu.iter().enumerate().map(|(idx, q)| q * (-(j_min + idx as i64) as f64 * dx * dt).exp()).sum();
            let alpha = (sum.ln() - target.ln()) / dt;
            slices[i].alpha = alpha;
            let next_min = slices[i + 1].j_min;
            let mut next_q = vec![0.0; slices[i + 1].arrow_debreu.len()];
            for (idx, q) in slices[i].arrow_debreu.iter().enumerate() {
                let x = (j_min + idx as i64) as f64 * dx;
                let disc = (-(alpha + x) * dt).exp();
                let k = slices[i].centre[idx];
                let (pu, pm, pd) = slices[i].probs[idx];
                let base = (k - next_min) as usize;
                next_q[base + 1] += q * disc * pu;
                next_q[base] += q * disc * pm;
                next_q[base - 1] += q * disc * pd;
            }
            slices[i + 1].arrow_debreu = next_q;
        }
        Ok(Self { slices })
    }

    pub fn n_slices(&self) -> usize {
        self.slices.len()
    }

    pub fn times(&self) -> Vec<f64> {
        self.slices.iter().map(|s| s.time).collect()
    }

    /// Node count at slice `i`.
    pub fn width(&self, i: usize) -> usize {
        (self.slices[i].j_max - self.slices[i].j_min + 1) as usize
    }

    /// Factor values at slice `i`.
    pub fn nodes(&self, i: usize) -> Vec<f64> {
        let s = &self.slices[i];
        (s.j_min..=s.j_max).map(|j| j as f64 * s.dx).collect()
    }

    /// Index of the slice at time `t` (within 1e-10), if any.
    pub fn slice_at(&self, t: f64) -> Option<usize> {
        self.slices.iter().position(|s| (s.time - t).abs() < 1e-10)
    }

    /// Arrow-Debreu prices at slice `i`; they sum to `P(0, t_i)`.
    pub fn arrow_debreu(&self, i: usize) -> &[f64] {
        &self.slices[i].arrow_debreu
    }

    /// Discounted expectation of a claim exercisable at the given slices.
    /// `payoff(k, x)` is the exercise value at the `k`-th exercise slice for
    /// factor `x`; the claim is worth `max(continuation, payoff)` there and
    /// rolls back to the root. `exercise_slices` must be increasing.
    pub fn rollback(&self, exercise_slices: &[usize], mut payoff: impl FnMut(usize, f64) -> f64) -> f64 {
        let last = match exercise_slices.last() {
            Some(&l) => l,
            None => return 0.0,
        };
        let mut values: Vec<f64> = self.nodes(last).into_iter().map(|x| payoff(exercise_slices.len() - 1, x)).collect();
        let mut next_exercise = exercise_slices.len().saturating_sub(1);
        for i in (0..last).rev() {
            let s = &self.slices[i];
            let dt = self.slices[i + 1].time - s.time;
            let next_min = self.slices[i + 1].j_min;
            let mut cur = Vec::with_capacity(self.width(i));
            for (idx, j) in (s.j_min..=s.j_max).enumerate() {
                let x = j as f64 * s.dx;
                let (pu, pm, pd) = s.probs[idx];
                let base = (s.centre[idx] - next_min) as usize;
                let cont = (-(s.alpha + x) * dt).exp() * (pu * values[base + 1] + pm * values[base] + pd * values[base - 1]);
                cur.push(cont);
            }
            if next_exercise > 0 && exercise_slices[next_exercise - 1] == i {
                next_exercise -= 1;
                for (idx, x) in self.nodes(i).into_iter().enumerate() {
                    cur[idx] = cur[idx].max(payoff(next_exercise, x));
                }
            }
            values = cur;
        }
        values[0]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_abs_diff_eq;
    use fq_core::math::OptionType;
    use fq_core::time::Date;
    use fq_rates::curve::DiscountCurve;

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
    fn geometry_and_probabilities() -> R {
        let m = model(0.1, 0.01);
        let tree = TrinomialTree::build(&m, &[10.0], 12)?;
        assert_eq!(tree.n_slices(), 121);
        assert_abs_diff_eq!(tree.times()[120], 10.0, epsilon = 1e-12);
        assert_eq!(tree.width(0), 1);
        assert_eq!(tree.width(1), 3);
        // Width stops growing once |j| (1 - e^{-a dt}) > 1/2, i.e. at
        // j = 0.5 / (a dt) = 60 here: mean reversion truncates the lattice.
        let widths: Vec<usize> = (0..121).map(|i| tree.width(i)).collect();
        assert!(widths.windows(2).all(|w| w[1] >= w[0]));
        assert_eq!(widths[120], widths[70]);
        assert!(widths[120] <= 123, "{}", widths[120]);
        assert_eq!(widths[30], 61);
        for s in &tree.slices[..120] {
            for &(pu, pm, pd) in &s.probs {
                assert!(pu > 0.0 && pm > 0.0 && pd > 0.0);
                assert_abs_diff_eq!(pu + pm + pd, 1.0, epsilon = 1e-14);
            }
        }
        assert!(TrinomialTree::build(&m, &[5.0], 0).is_err());
        assert!(TrinomialTree::build(&m, &[0.0], 12).is_err());
        assert_eq!(tree.slice_at(2.5), Some(30));
        assert_eq!(tree.slice_at(2.51), None);
        assert_eq!(tree.nodes(1).len(), 3);
        assert_abs_diff_eq!(tree.nodes(1)[1], 0.0, epsilon = 1e-15);
        Ok(())
    }

    #[test]
    fn mandatory_times_land_on_slices() -> R {
        let m = model(0.05, 0.01);
        let tree = TrinomialTree::build(&m, &[1.3, 2.0, 2.0, 3.7], 10)?;
        for t in [1.3, 2.0, 3.7] {
            assert!(tree.slice_at(t).is_some(), "{t}");
        }
        let times = tree.times();
        assert!(times.windows(2).all(|w| w[1] > w[0]));
        Ok(())
    }

    #[test]
    fn arrow_debreu_reprice_the_curve() -> R {
        let m = model(0.05, 0.012);
        let tree = TrinomialTree::build(&m, &[10.0], 24)?;
        for i in 1..tree.n_slices() {
            let t = tree.times()[i];
            let sum: f64 = tree.arrow_debreu(i).iter().sum();
            assert_abs_diff_eq!(sum, m.curve().discount_t(t), epsilon = 1e-13);
        }
        Ok(())
    }

    #[test]
    fn factor_moments_on_the_tree() -> R {
        // Arrow-Debreu prices normalised by their sum are the t-forward
        // density of the factor, so its mean is -y(t) and its variance v(t).
        let m = model(0.1, 0.01);
        let tree = TrinomialTree::build(&m, &[4.0], 50)?;
        let i = tree.n_slices() - 1;
        let t = tree.times()[i];
        let q = tree.arrow_debreu(i);
        let nodes = tree.nodes(i);
        let total: f64 = q.iter().sum();
        let mean: f64 = q.iter().zip(&nodes).map(|(w, x)| w * x).sum::<f64>() / total;
        let var: f64 = q.iter().zip(&nodes).map(|(w, x)| w * (x - mean) * (x - mean)).sum::<f64>() / total;
        assert_abs_diff_eq!(mean, m.forward_measure_mean(t), epsilon = 2e-5);
        assert_abs_diff_eq!(var, m.variance(t), epsilon = 2e-6);
        Ok(())
    }

    #[test]
    fn zcb_option_on_the_tree_converges() -> R {
        let m = model(0.05, 0.01);
        let (t, s, k) = (2.0, 7.0, 0.80);
        let closed = m.zcb_option(OptionType::Put, t, s, k)?;
        let tree = TrinomialTree::build(&m, &[t], 100)?;
        let slice = tree.slice_at(t).expect("slice");
        let value = tree.rollback(&[slice], |_, x| (k - m.bond_unchecked(t, s, x)).max(0.0));
        assert!((value - closed).abs() / closed < 2e-3, "{value} vs {closed}");
        Ok(())
    }
}
