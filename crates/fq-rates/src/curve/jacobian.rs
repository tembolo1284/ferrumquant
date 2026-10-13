//! Par-quote bucketed risk via a curve Jacobian.
//!
//! Zero-rate bumps on a built curve are cheap (one `with_node_shift` and a
//! reprice per bucket) but a desk hedges in par instruments. The link between
//! the two is the Jacobian `J[i][j] = d zero_i / d quote_j`, obtained by
//! bumping each input quote and re-bootstrapping. `J` depends only on the
//! curve, so it is built once per curve rebuild and shared by every position:
//!
//! ```text
//! dPV/dquote_j = sum_i (dPV/dzero_i) * J[i][j]
//! ```
//!
//! For an *n*-instrument curve the cost is *n* bootstraps per curve tick,
//! then *n* cheap reprices per position; nothing scales with
//! positions × bootstraps.

use fq_core::time::Date;

use crate::curve::{BootstrapConfig, BootstrapError, BootstrapInstrument, BootstrapResult, CurveError, DiscountCurve, InstrumentKey, bootstrap_sofr};
use crate::index::OvernightIndex;

/// Bumped-quote sensitivities of a bootstrapped curve.
#[derive(Debug, Clone, PartialEq)]
pub struct CurveJacobian {
    /// Key of each input quote, in node order.
    pub keys: Vec<InstrumentKey>,
    /// `matrix[i][j]` = change in node `i`'s zero rate per unit change in quote `j`.
    pub matrix: Vec<Vec<f64>>,
    /// Quote bump used to build the Jacobian (rate units, e.g. 1e-4).
    pub bump: f64,
}

impl CurveJacobian {
    pub fn node_count(&self) -> usize {
        self.keys.len()
    }

    /// Convert a zero-rate bucket ladder (`dPV/dzero_i`, one per node) into
    /// a par-quote ladder (`dPV/dquote_j`, one per instrument).
    pub fn to_par_ladder(&self, zero_ladder: &[f64]) -> Result<Vec<f64>, CurveError> {
        let n = self.node_count();
        if zero_ladder.len() != n {
            return Err(CurveError::NodeOutOfRange { index: zero_ladder.len(), nodes: n });
        }
        Ok((0..n).map(|j| (0..n).map(|i| zero_ladder[i] * self.matrix[i][j]).sum()).collect())
    }
}

/// Bootstrap the curve and its Jacobian in one call: `bump` is applied to
/// each quote in turn (central difference), so this runs `2n + 1` bootstraps.
pub fn bootstrap_sofr_with_jacobian(
    trade_date: Date,
    index: &OvernightIndex,
    instruments: &[BootstrapInstrument],
    config: BootstrapConfig,
    bump: f64,
) -> Result<(BootstrapResult, CurveJacobian), BootstrapError> {
    let base = bootstrap_sofr(trade_date, index, instruments, config)?;
    let n = base.nodes.len();
    let keys = base.keys();

    // Instruments in node order so column j lines up with keys[j].
    let mut ordered: Vec<BootstrapInstrument> = instruments.to_vec();
    ordered.sort_by_key(|inst| keys.iter().position(|k| *k == inst.key()).unwrap_or(usize::MAX));

    let mut matrix = vec![vec![0.0; n]; n];
    for j in 0..n {
        let up = bootstrap_sofr(trade_date, index, &bumped(&ordered, j, bump), config)?;
        let down = bootstrap_sofr(trade_date, index, &bumped(&ordered, j, -bump), config)?;
        for (i, row) in matrix.iter_mut().enumerate() {
            row[j] = (up.curve.node_zero_rate(i)? - down.curve.node_zero_rate(i)?) / (2.0 * bump);
        }
    }
    Ok((base, CurveJacobian { keys, matrix, bump }))
}

fn bumped(instruments: &[BootstrapInstrument], j: usize, shift: f64) -> Vec<BootstrapInstrument> {
    instruments.iter().enumerate().map(|(k, inst)| if k == j { inst.bumped(shift) } else { *inst }).collect()
}

/// Zero-rate bucket ladder for any pricer: `dPV/dzero_i` by central
/// difference on each node, in the same sign convention as `OisSwap::dv01`
/// (change per +1 unit of rate; multiply by 1e-4 for per-bp). Works with any
/// pricer error type that wraps `CurveError` (`SwapError`, `BondError`, ...).
pub fn zero_bucket_ladder<E: From<CurveError>>(
    curve: &DiscountCurve,
    bump: f64,
    mut price: impl FnMut(&DiscountCurve) -> Result<f64, E>,
) -> Result<Vec<f64>, E> {
    (0..curve.node_count())
        .map(|i| {
            let up = price(&curve.with_node_shift(i, bump)?)?;
            let down = price(&curve.with_node_shift(i, -bump)?)?;
            Ok((up - down) / (2.0 * bump))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_abs_diff_eq;

    use crate::swap::{OisSwap, SwapSide, Tenor};

    type R = Result<(), Box<dyn std::error::Error>>;

    fn d(y: i32, m: u32, dd: u32) -> Date {
        Date::from_ymd(y, m, dd).expect("valid test date")
    }

    fn quotes() -> Vec<BootstrapInstrument> {
        [("1Y", 0.0400), ("2Y", 0.0380), ("5Y", 0.0378), ("10Y", 0.0395)]
            .into_iter()
            .map(|(t, r)| BootstrapInstrument::ois(t.parse().expect("tenor"), r))
            .collect()
    }

    #[test]
    fn jacobian_is_lower_triangular_and_positive_on_the_diagonal() -> R {
        let today = d(2026, 9, 23);
        let index = OvernightIndex::sofr();
        let (_, jac) = bootstrap_sofr_with_jacobian(today, &index, &quotes(), BootstrapConfig::default(), 1e-4)?;
        let n = jac.node_count();
        assert_eq!(n, 4);
        for i in 0..n {
            for j in 0..n {
                let v = jac.matrix[i][j];
                if j > i {
                    // A later quote cannot move an earlier node in a sequential bootstrap.
                    assert_abs_diff_eq!(v, 0.0, epsilon = 1e-9);
                } else if j == i {
                    assert!(v > 0.5 && v < 1.5, "diag[{i}] = {v}");
                }
            }
        }
        Ok(())
    }

    #[test]
    fn par_ladder_of_an_input_swap_is_its_own_bucket_only() -> R {
        let today = d(2026, 9, 23);
        let index = OvernightIndex::sofr();
        let (res, jac) = bootstrap_sofr_with_jacobian(today, &index, &quotes(), BootstrapConfig::default(), 1e-4)?;
        // A 5Y payer at par: PV moves only with the 5Y quote, by ~annuity.
        let swap = OisSwap::usd_sofr_spot(today, Tenor::years(5), SwapSide::PayFixed, 1e6, 0.0378)?;
        let zero = zero_bucket_ladder(&res.curve, 1e-4, |c| swap.npv(&index, c))?;
        let par = jac.to_par_ladder(&zero)?;
        let annuity = swap.value(&index, &res.curve, &res.curve)?.annuity;
        for (j, key) in jac.keys.iter().enumerate() {
            if *key == InstrumentKey::Ois(Tenor::years(5)) {
                assert_abs_diff_eq!(par[j], annuity, epsilon = 0.02 * annuity);
            } else {
                assert!(par[j].abs() < 0.01 * annuity, "{key}: {}", par[j]);
            }
        }
        // Direct check: bump the 5Y quote, re-bootstrap, reprice.
        let mut q = quotes();
        q[2] = BootstrapInstrument::ois(Tenor::years(5), 0.0379);
        let bumped_res = bootstrap_sofr(today, &index, &q, BootstrapConfig::default())?;
        let direct = (swap.npv(&index, &bumped_res.curve)? - swap.npv(&index, &res.curve)?) / 1e-4;
        assert_abs_diff_eq!(par[2], direct, epsilon = 0.01 * annuity);
        Ok(())
    }

    #[test]
    fn ladder_length_mismatch_is_an_error() -> R {
        let today = d(2026, 9, 23);
        let (_, jac) = bootstrap_sofr_with_jacobian(today, &OvernightIndex::sofr(), &quotes(), BootstrapConfig::default(), 1e-4)?;
        assert!(matches!(jac.to_par_ladder(&[0.0; 3]), Err(CurveError::NodeOutOfRange { .. })));
        Ok(())
    }
}
