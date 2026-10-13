//! Sequential bootstrap of a SOFR discount curve from deposits, SR3 futures
//! and OIS par rates.
//!
//! Instruments are sorted by pillar date (deposit end, futures reference-period
//! end, swap final payment); each adds one node at its pillar, solved with
//! Brent on the node's zero rate so the instrument reprices given the nodes
//! already fixed. With log-linear discount interpolation this is the classic
//! piecewise-flat-forward bootstrap and every input reprices to machine
//! precision. Overlapping futures and swaps are allowed as long as pillars
//! strictly increase; the usual choice is deposit + futures out to ~2Y, then
//! swaps.

use std::fmt;

use thiserror::Error;

use fq_core::math::{SolverConfig, SolverError, brent};
use fq_core::time::{Calendar, Date, ImmMonth, Market, Period, PeriodError};

use crate::curve::{CurveError, DiscountCurve, Interpolation};
use crate::future::{SofrFuture, SofrFutureError};
use crate::index::OvernightIndex;
use crate::instrument::{Deposit, DepositError};
use crate::swap::{OisSwap, SwapError, SwapSide, Tenor};

#[derive(Debug, Error, Clone, PartialEq)]
pub enum BootstrapError {
    #[error(transparent)]
    Swap(#[from] SwapError),
    #[error(transparent)]
    Deposit(#[from] DepositError),
    #[error(transparent)]
    Future(#[from] SofrFutureError),
    #[error(transparent)]
    Curve(#[from] CurveError),
    #[error(transparent)]
    Period(#[from] PeriodError),
    #[error("solving node {node} ({key}): {source}")]
    Solver { key: InstrumentKey, node: Date, source: SolverError },
    #[error("no instruments given")]
    NoInstruments,
    #[error("instruments {a} and {b} both pin node {node}")]
    DuplicateNode { a: InstrumentKey, b: InstrumentKey, node: Date },
}

/// One quoted instrument.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BootstrapInstrument {
    /// Cash deposit from today for `tenor` (1D = SOFR overnight), simple ACT/360.
    Deposit { tenor: Period, rate: f64 },
    /// SR3 future for `contract`, price in points (e.g. 96.25), convexity in rate units.
    SofrFuture { contract: ImmMonth, price: f64, convexity: f64 },
    /// Spot-starting USD SOFR OIS at `tenor` with the given par fixed rate.
    Ois { tenor: Tenor, rate: f64 },
}

impl BootstrapInstrument {
    pub const fn deposit(tenor: Period, rate: f64) -> Self {
        Self::Deposit { tenor, rate }
    }

    pub const fn sofr_future(contract: ImmMonth, price: f64, convexity: f64) -> Self {
        Self::SofrFuture { contract, price, convexity }
    }

    pub const fn ois(tenor: Tenor, rate: f64) -> Self {
        Self::Ois { tenor, rate }
    }

    pub fn key(&self) -> InstrumentKey {
        match *self {
            Self::Deposit { tenor, .. } => InstrumentKey::Deposit(tenor),
            Self::SofrFuture { contract, .. } => InstrumentKey::SofrFuture(contract),
            Self::Ois { tenor, .. } => InstrumentKey::Ois(tenor),
        }
    }

    /// Same instrument with its quote moved by `shift` in *rate* terms
    /// (a futures price moves by `-100 * shift`).
    pub fn bumped(&self, shift: f64) -> Self {
        match *self {
            Self::Deposit { tenor, rate } => Self::Deposit { tenor, rate: rate + shift },
            Self::SofrFuture { contract, price, convexity } => Self::SofrFuture { contract, price: price - 100.0 * shift, convexity },
            Self::Ois { tenor, rate } => Self::Ois { tenor, rate: rate + shift },
        }
    }
}

/// Identifies an instrument in results and errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InstrumentKey {
    Deposit(Period),
    SofrFuture(ImmMonth),
    Ois(Tenor),
}

impl fmt::Display for InstrumentKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Deposit(p) => write!(f, "DEPO {p}"),
            Self::SofrFuture(m) => write!(f, "SR3 {m}"),
            Self::Ois(t) => write!(f, "OIS {t}"),
        }
    }
}

/// A quoted instrument turned into its dated form.
#[derive(Debug, Clone, PartialEq)]
pub enum BuiltInstrument {
    Deposit(Deposit),
    SofrFuture(SofrFuture),
    Ois(OisSwap),
}

impl BuiltInstrument {
    fn build(inst: &BootstrapInstrument, trade_date: Date) -> Result<Self, BootstrapError> {
        Ok(match *inst {
            BootstrapInstrument::Deposit { tenor, rate } => {
                if tenor == Period::days(1) {
                    Self::Deposit(Deposit::sofr_overnight(trade_date, rate)?)
                } else {
                    let cal = Calendar::new(Market::UsGovernmentBond);
                    Self::Deposit(Deposit::term(trade_date, 0, tenor, rate, fq_core::time::DayCount::Act360, &cal)?)
                }
            }
            BootstrapInstrument::SofrFuture { contract, price, convexity } => {
                Self::SofrFuture(SofrFuture::new(contract, price, convexity, &Calendar::new(Market::UsGovernmentBond))?)
            }
            BootstrapInstrument::Ois { tenor, rate } => Self::Ois(OisSwap::usd_sofr_spot(trade_date, tenor, SwapSide::ReceiveFixed, 1.0, rate)?),
        })
    }

    /// The date whose discount factor this instrument pins.
    pub fn pillar(&self) -> Date {
        match self {
            Self::Deposit(d) => d.end(),
            Self::SofrFuture(f) => f.end(),
            Self::Ois(s) => s.final_payment_date(),
        }
    }

    /// Bootstrap objective: zero when the curve reprices the instrument.
    pub fn npv(&self, index: &OvernightIndex, curve: &DiscountCurve) -> Result<f64, BootstrapError> {
        Ok(match self {
            Self::Deposit(d) => d.npv(curve)?,
            Self::SofrFuture(f) => f.npv(index, curve)?,
            Self::Ois(s) => s.npv(index, curve)?,
        })
    }

    pub fn as_ois(&self) -> Option<&OisSwap> {
        match self {
            Self::Ois(s) => Some(s),
            _ => None,
        }
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
    pub instruments: Vec<BuiltInstrument>,
    pub nodes: Vec<(InstrumentKey, Date, f64)>,
}

impl BootstrapResult {
    pub fn keys(&self) -> Vec<InstrumentKey> {
        self.nodes.iter().map(|(k, _, _)| *k).collect()
    }

    pub fn ois_swaps(&self) -> impl Iterator<Item = &OisSwap> {
        self.instruments.iter().filter_map(BuiltInstrument::as_ois)
    }
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

    let mut built: Vec<(InstrumentKey, BuiltInstrument, Date)> = Vec::with_capacity(instruments.len());
    for inst in instruments {
        let b = BuiltInstrument::build(inst, trade_date)?;
        let pillar = b.pillar();
        built.push((inst.key(), b, pillar));
    }
    built.sort_by_key(|(_, _, pillar)| *pillar);

    let mut nodes: Vec<(Date, f64)> = Vec::with_capacity(built.len());
    let mut out_nodes = Vec::with_capacity(built.len());
    let mut prev: Option<(InstrumentKey, Date)> = None;

    for (key, inst, node) in &built {
        let node = *node;
        if let Some((prev_key, prev_node)) = prev {
            if prev_node == node {
                return Err(BootstrapError::DuplicateNode { a: prev_key, b: *key, node });
            }
        }
        let t = f64::from(node - trade_date) / 365.0;

        let objective = |z: f64| -> f64 {
            let mut trial = nodes.clone();
            trial.push((node, (-z * t).exp()));
            match DiscountCurve::new(trade_date, &trial, config.interpolation) {
                Ok(curve) => inst.npv(index, &curve).unwrap_or(f64::NAN),
                Err(_) => f64::NAN,
            }
        };
        let (lo, hi) = config.zero_rate_bounds;
        let z = brent(objective, lo, hi, config.solver).map_err(|source| BootstrapError::Solver { key: *key, node, source })?;

        let df = (-z * t).exp();
        nodes.push((node, df));
        out_nodes.push((*key, node, df));
        prev = Some((*key, node));
    }

    let curve = DiscountCurve::new(trade_date, &nodes, config.interpolation)?;
    Ok(BootstrapResult { curve, instruments: built.into_iter().map(|(_, b, _)| b).collect(), nodes: out_nodes })
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_abs_diff_eq;

    type R = Result<(), BootstrapError>;

    fn d(y: i32, m: u32, dd: u32) -> Date {
        Date::from_ymd(y, m, dd).expect("valid test date")
    }

    fn ois_quotes() -> Vec<BootstrapInstrument> {
        [
            ("1M", 0.0430), ("3M", 0.0425), ("6M", 0.0415), ("1Y", 0.0400), ("2Y", 0.0380),
            ("3Y", 0.0375), ("5Y", 0.0378), ("7Y", 0.0385), ("10Y", 0.0395), ("30Y", 0.0410),
        ]
        .into_iter()
        .map(|(t, r)| BootstrapInstrument::ois(t.parse().expect("tenor"), r))
        .collect()
    }

    /// Deposit + four SR3 contracts + swaps from 2Y, as a desk would build it.
    fn mixed_quotes() -> Vec<BootstrapInstrument> {
        let mut q = vec![BootstrapInstrument::deposit(Period::days(1), 0.0432)];
        for (c, p) in [("Z26", 95.78), ("H27", 95.95), ("M27", 96.10), ("U27", 96.20)] {
            q.push(BootstrapInstrument::sofr_future(c.parse().expect("imm"), p, 0.0002));
        }
        for (t, r) in [("2Y", 0.0380), ("3Y", 0.0375), ("5Y", 0.0378), ("10Y", 0.0395)] {
            q.push(BootstrapInstrument::ois(t.parse().expect("tenor"), r));
        }
        q
    }

    #[test]
    fn reprices_every_ois_input_at_par() -> R {
        let today = d(2026, 9, 23);
        let index = OvernightIndex::sofr();
        let res = bootstrap_sofr(today, &index, &ois_quotes(), BootstrapConfig::default())?;
        assert_eq!(res.nodes.len(), 10);
        for swap in res.ois_swaps() {
            assert_abs_diff_eq!(swap.npv(&index, &res.curve)?, 0.0, epsilon = 1e-11);
            assert_abs_diff_eq!(swap.value(&index, &res.curve, &res.curve)?.par_rate, swap.fixed_rate(), epsilon = 1e-11);
        }
        Ok(())
    }

    #[test]
    fn mixed_curve_reprices_every_instrument() -> R {
        let today = d(2026, 10, 13);
        let index = OvernightIndex::sofr();
        let res = bootstrap_sofr(today, &index, &mixed_quotes(), BootstrapConfig::default())?;
        assert_eq!(res.nodes.len(), 9);
        // Pillars: ON end, four IMM ends, four swap payments, strictly increasing.
        assert_eq!(res.nodes[0].1, d(2026, 10, 14));
        assert_eq!(res.nodes[1].1, d(2027, 3, 17));
        assert_eq!(res.nodes[4].1, d(2027, 12, 15));
        assert!(res.nodes.windows(2).all(|w| w[0].1 < w[1].1));
        for inst in &res.instruments {
            assert_abs_diff_eq!(inst.npv(&index, &res.curve)?, 0.0, epsilon = 1e-11);
        }
        // The futures strip is reproduced at the quoted prices.
        for inst in &res.instruments {
            if let BuiltInstrument::SofrFuture(f) = inst {
                assert_abs_diff_eq!(f.fair_price(&index, &res.curve)?, f.price(), epsilon = 1e-9);
            }
        }
        // The overnight node is the deposit's own discount factor.
        assert_abs_diff_eq!(res.nodes[0].2, 1.0 / (1.0 + 0.0432 / 360.0), epsilon = 1e-15);
        Ok(())
    }

    #[test]
    fn nodes_increase_and_forwards_are_sane() -> R {
        let today = d(2026, 9, 23);
        let index = OvernightIndex::sofr();
        let res = bootstrap_sofr(today, &index, &ois_quotes(), BootstrapConfig::default())?;
        let mut last_date = today;
        let mut last_df = 1.0;
        for &(_, date, df) in &res.nodes {
            assert!(date > last_date);
            assert!(df < last_df && df > 0.0);
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
        let mut q = ois_quotes();
        q.reverse();
        let res = bootstrap_sofr(today, &index, &q, BootstrapConfig::default())?;
        assert_eq!(res.nodes[0].0, InstrumentKey::Ois("1M".parse::<Tenor>()?));
        assert_eq!(res.nodes[0].0.to_string(), "OIS 1M");

        let dup = [BootstrapInstrument::ois(Tenor::years(1), 0.04), BootstrapInstrument::ois(Tenor::months(12), 0.041)];
        assert!(matches!(bootstrap_sofr(today, &index, &dup, BootstrapConfig::default()), Err(BootstrapError::DuplicateNode { .. })));
        assert!(matches!(bootstrap_sofr(today, &index, &[], BootstrapConfig::default()), Err(BootstrapError::NoInstruments)));
        Ok(())
    }

    #[test]
    fn bumping_moves_each_quote_in_rate_terms() {
        let dep = BootstrapInstrument::deposit(Period::days(1), 0.04).bumped(1e-4);
        assert!(matches!(dep, BootstrapInstrument::Deposit { rate, .. } if (rate - 0.0401).abs() < 1e-15));
        let fut = BootstrapInstrument::sofr_future("Z26".parse().expect("imm"), 96.0, 0.0).bumped(1e-4);
        assert!(matches!(fut, BootstrapInstrument::SofrFuture { price, .. } if (price - 95.99).abs() < 1e-12));
    }
}
