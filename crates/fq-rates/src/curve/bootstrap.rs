//! Sequential bootstrap of a SOFR discount curve from OIS par rates.
//!
//! Instruments are sorted by maturity; each adds one node at its final
//! payment date, solved (Brent on the node's zero rate) so the swap reprices
//! at par given the nodes already fixed. With log-linear discount
//! interpolation this is the classic piecewise-flat-forward bootstrap, and
//! every input reprices to machine precision.

use thiserror::Error;

use fq_core::math::{SolverConfig, SolverError, brent};
use fq_core::time::Date;

use crate::curve::{CurveError, DiscountCurve, Interpolation};
use crate::index::OvernightIndex;
use crate::swap::{OisSwap, SwapError, SwapSide, Tenor};

#[derive(Debug, Error, Clone, PartialEq)]
pub enum BootstrapError {
    #[error(transparent)]
    Swap(#[from] SwapError),
    #[error(transparent)]
    Curve(#[from] CurveError),
    #[error("solving node {node} ({tenor}): {source}")]
    Solver { tenor: Tenor, node: Date, source: SolverError },
    #[error("no instruments given")]
    NoInstruments,
    #[error("instruments {a} and {b} both pin node {node}")]
    DuplicateNode { a: Tenor, b: Tenor, node: Date },
}

/// One quoted instrument. Deposits and SOFR futures are the planned additions.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BootstrapInstrument {
    /// Spot-starting USD SOFR OIS at `tenor` with the given par fixed rate.
    Ois { tenor: Tenor, rate: f64 },
}

impl BootstrapInstrument {
    pub const fn ois(tenor: Tenor, rate: f64) -> Self {
        Self::Ois { tenor, rate }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BootstrapConfig {
    pub interpolation: Interpolation,
    pub solver: SolverConfig,
    /// Zero-rate bracket handed to Brent for each node.
    pub zero_rate_bounds: (f64, f64),
}

impl Default for BootstrapConfig {
    fn default() -> Self {
        Self {
            interpolation: Interpolation::LogLinearDiscount,
            solver: SolverConfig { tolerance: 1e-14, max_iterations: 200 },
            zero_rate_bounds: (-0.5, 1.0),
        }
    }
}

/// Bootstrapped curve plus the instruments as built, in node order.
#[derive(Debug, Clone, PartialEq)]
pub struct BootstrapResult {
    pub curve: DiscountCurve,
    pub swaps: Vec<OisSwap>,
    pub nodes: Vec<(Tenor, Date, f64)>,
}

pub fn bootstrap_sofr(
    trade_date: Date,
    index: &OvernightIndex,
    instruments: &[BootstrapInstrument],
    config: BootstrapConfig,
) -> Result<BootstrapResult, BootstrapError> {
    if instruments.is_empty() {
        return Err(BootstrapError::NoInstruments);
    }

    // Build unit-notional receiver swaps and order them by final payment date.
    let mut swaps: Vec<(Tenor, OisSwap, Date)> = Vec::with_capacity(instruments.len());
    for inst in instruments {
        match *inst {
            BootstrapInstrument::Ois { tenor, rate } => {
                let swap = OisSwap::usd_sofr_spot(trade_date, tenor, SwapSide::ReceiveFixed, 1.0, rate)?;
                let node = final_payment(&swap)?;
                swaps.push((tenor, swap, node));
            }
        }
    }
    swaps.sort_by_key(|(_, _, node)| *node);

    let mut nodes: Vec<(Date, f64)> = Vec::with_capacity(swaps.len());
    let mut out_nodes = Vec::with_capacity(swaps.len());
    let mut prev: Option<(Tenor, Date)> = None;

    for (tenor, swap, node) in &swaps {
        let node = *node;
        if let Some((prev_tenor, prev_node)) = prev {
            if prev_node == node {
                return Err(BootstrapError::DuplicateNode { a: prev_tenor, b: *tenor, node });
            }
        }
        let t = f64::from(node - trade_date) / 365.0;

        let objective = |z: f64| -> f64 {
            let mut trial = nodes.clone();
            trial.push((node, (-z * t).exp()));
            match DiscountCurve::new(trade_date, &trial, config.interpolation) {
                Ok(curve) => swap.npv(index, &curve).unwrap_or(f64::NAN),
                Err(_) => f64::NAN,
            }
        };
        let (lo, hi) = config.zero_rate_bounds;
        let z = brent(objective, lo, hi, config.solver).map_err(|source| BootstrapError::Solver { tenor: *tenor, node, source })?;

        let df = (-z * t).exp();
        nodes.push((node, df));
        out_nodes.push((*tenor, node, df));
        prev = Some((*tenor, node));
    }

    let curve = DiscountCurve::new(trade_date, &nodes, config.interpolation)?;
    Ok(BootstrapResult { curve, swaps: swaps.into_iter().map(|(_, s, _)| s).collect(), nodes: out_nodes })
}

fn final_payment(swap: &OisSwap) -> Result<Date, SwapError> {
    Ok(swap.periods()?.last().map(|p| p.2).expect("schedule has at least one period"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_abs_diff_eq;

    type R = Result<(), BootstrapError>;

    fn d(y: i32, m: u32, dd: u32) -> Date {
        Date::from_ymd(y, m, dd).expect("valid test date")
    }

    fn quotes() -> Vec<BootstrapInstrument> {
        [
            ("1M", 0.0430), ("3M", 0.0425), ("6M", 0.0415), ("1Y", 0.0400), ("2Y", 0.0380),
            ("3Y", 0.0375), ("5Y", 0.0378), ("7Y", 0.0385), ("10Y", 0.0395), ("30Y", 0.0410),
        ]
        .into_iter()
        .map(|(t, r)| BootstrapInstrument::ois(t.parse().expect("tenor"), r))
        .collect()
    }

    #[test]
    fn reprices_every_input_at_par() -> R {
        let today = d(2026, 9, 23);
        let index = OvernightIndex::sofr();
        let res = bootstrap_sofr(today, &index, &quotes(), BootstrapConfig::default())?;
        assert_eq!(res.nodes.len(), 10);
        for swap in &res.swaps {
            let npv = swap.npv(&index, &res.curve)?;
            assert_abs_diff_eq!(npv, 0.0, epsilon = 1e-11);
            let par = swap.value(&index, &res.curve, &res.curve)?.par_rate;
            assert_abs_diff_eq!(par, swap.fixed_rate(), epsilon = 1e-11);
        }
        Ok(())
    }

    #[test]
    fn nodes_increase_and_forwards_are_sane() -> R {
        let today = d(2026, 9, 23);
        let index = OvernightIndex::sofr();
        let res = bootstrap_sofr(today, &index, &quotes(), BootstrapConfig::default())?;
        let mut last_date = today;
        let mut last_df = 1.0;
        for &(_, date, df) in &res.nodes {
            assert!(date > last_date);
            assert!(df < last_df && df > 0.0);
            // Continuously-compounded forward over the gap (simple rates would
            // balloon on the 20-year 10Y->30Y segment).
            let dt = f64::from(date - last_date) / 365.0;
            let fwd = (last_df / df).ln() / dt;
            assert!(fwd > 0.03 && fwd < 0.05, "forward {fwd} over {last_date}..{date}");
            last_date = date;
            last_df = df;
        }
        Ok(())
    }

    #[test]
    fn unsorted_input_is_fine_and_duplicates_are_not() -> R {
        let today = d(2026, 9, 23);
        let index = OvernightIndex::sofr();
        let mut q = quotes();
        q.reverse();
        let res = bootstrap_sofr(today, &index, &q, BootstrapConfig::default())?;
        assert_eq!(res.nodes[0].0, "1M".parse::<Tenor>()?);

        let dup = [BootstrapInstrument::ois(Tenor::years(1), 0.04), BootstrapInstrument::ois(Tenor::months(12), 0.041)];
        assert!(matches!(bootstrap_sofr(today, &index, &dup, BootstrapConfig::default()), Err(BootstrapError::DuplicateNode { .. })));
        assert!(matches!(bootstrap_sofr(today, &index, &[], BootstrapConfig::default()), Err(BootstrapError::NoInstruments)));
        Ok(())
    }

    #[test]
    fn flat_quotes_give_flat_forwards() -> R {
        let today = d(2026, 9, 23);
        let index = OvernightIndex::sofr();
        let flat: Vec<_> = ["1Y", "2Y", "5Y", "10Y"].iter().map(|t| BootstrapInstrument::ois(t.parse().expect("tenor"), 0.04)).collect();
        let res = bootstrap_sofr(today, &index, &flat, BootstrapConfig::default())?;
        // Annual ACT/360 par of 4% flat means yearly forwards near 4% * 360/365 in continuous terms.
        let z = res.curve.zero_rate(res.nodes[3].1)?;
        assert!(z > 0.038 && z < 0.040, "zero {z}");
        Ok(())
    }
}
