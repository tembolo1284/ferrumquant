//! Swaption volatility cube: an ATM normal-vol grid over (expiry, tenor)
//! with a SABR smile at each node.
//!
//! Quotes come in as ATM normal vols plus strike-offset vols
//! (ATM ± 25/50/100bp, say) per node. Each node's smile is calibrated to
//! normal SABR (β = 0) with the ATM vol held exactly. Querying
//! `(expiry, tenor, forward, strike)` interpolates the *smile parameters*
//! bilinearly in (expiry time, tenor years) and evaluates the SABR vol at
//! the requested strike, which keeps the surface arbitrage-consistent at
//! every strike rather than only at the quoted offsets.
//!
//! `node_weights` exposes the four grid nodes and weights a query touches,
//! the hook for bucketed vega in a risk engine.

use thiserror::Error;

use fq_core::math::interpolation::{Bilinear, InterpError};
use fq_core::math::sabr::{SabrError, SabrParams, SabrVolType, calibrate};
use fq_core::time::{Date, DateError, Period};

use crate::swaption::VolQuote;

#[derive(Debug, Error, Clone, PartialEq)]
pub enum VolCubeError {
    #[error(transparent)]
    Date(#[from] DateError),
    #[error(transparent)]
    Interp(#[from] InterpError),
    #[error(transparent)]
    Sabr(#[from] SabrError),
    #[error("expected {expected} nodes ({expiries} expiries x {tenors} tenors), got {got}")]
    NodeCount { expected: usize, expiries: usize, tenors: usize, got: usize },
    #[error("node ({expiry}, {tenor}) has no smile quotes and no default SABR parameters")]
    NoSmile { expiry: Period, tenor: Period },
    #[error("expiry {0} is not after the reference date")]
    NonPositiveExpiry(Date),
}

/// Market quotes for one (expiry, tenor) node.
#[derive(Debug, Clone, PartialEq)]
pub struct NodeQuotes {
    /// Forward swap rate the smile is quoted around.
    pub forward: f64,
    /// ATM normal vol (rate units, e.g. 0.0080 = 80bp).
    pub atm_vol: f64,
    /// (strike offset from ATM, normal vol) pairs; may be empty.
    pub smile: Vec<(f64, f64)>,
}

/// Calibrated state of one node.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NodeSmile {
    pub forward: f64,
    pub atm_vol: f64,
    pub sabr: SabrParams,
    pub rmse: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SwaptionVolCube {
    reference: Date,
    expiries: Vec<Period>,
    tenors: Vec<Period>,
    expiry_times: Vec<f64>,
    tenor_years: Vec<f64>,
    nodes: Vec<NodeSmile>,
    atm: Bilinear,
    rho: Bilinear,
    nu: Bilinear,
}

/// Default smile shape used when a node has no strike quotes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DefaultSmile {
    pub rho: f64,
    pub nu: f64,
}

impl SwaptionVolCube {
    /// `quotes` is row-major over `expiries` then `tenors`. Nodes without
    /// smile quotes use `default_smile` (rho, nu) with alpha set from ATM.
    pub fn new(reference: Date, expiries: &[Period], tenors: &[Period], quotes: &[NodeQuotes], default_smile: Option<DefaultSmile>) -> Result<Self, VolCubeError> {
        let (ne, nt) = (expiries.len(), tenors.len());
        if quotes.len() != ne * nt {
            return Err(VolCubeError::NodeCount { expected: ne * nt, expiries: ne, tenors: nt, got: quotes.len() });
        }
        let expiry_times: Vec<f64> = expiries.iter().map(|p| Self::years(reference, *p)).collect::<Result<_, _>>()?;
        let tenor_years: Vec<f64> = tenors.iter().map(|p| Self::years(reference, *p)).collect::<Result<_, _>>()?;

        let mut nodes = Vec::with_capacity(quotes.len());
        for (idx, q) in quotes.iter().enumerate() {
            let t = expiry_times[idx / nt];
            if t <= 0.0 {
                return Err(VolCubeError::NonPositiveExpiry(expiries[idx / nt].add_to(reference)?));
            }
            let sabr = if q.smile.len() >= 2 {
                // Fit rho, nu to the full strip (ATM included), then re-anchor alpha to the ATM vol.
                let mut strip: Vec<(f64, f64)> = q.smile.iter().map(|&(dk, v)| (q.forward + dk, v)).collect();
                strip.push((q.forward, q.atm_vol));
                let fitted = calibrate(q.forward, t, 0.0, 0.0, &strip, SabrVolType::Normal, Some((q.atm_vol, 0.0, 0.3)))?;
                anchor_alpha(fitted.params, q.atm_vol, t)?
            } else {
                let d = default_smile.ok_or(VolCubeError::NoSmile { expiry: expiries[idx / nt], tenor: tenors[idx % nt] })?;
                anchor_alpha(SabrParams::new(q.atm_vol, 0.0, d.rho, d.nu, 0.0)?, q.atm_vol, t)?
            };
            let rmse = q.smile.iter().map(|&(dk, v)| (sabr.normal_vol(q.forward, q.forward + dk, t) - v).powi(2)).sum::<f64>();
            let rmse = if q.smile.is_empty() { 0.0 } else { (rmse / q.smile.len() as f64).sqrt() };
            nodes.push(NodeSmile { forward: q.forward, atm_vol: q.atm_vol, sabr, rmse });
        }

        let grid = |f: &dyn Fn(&NodeSmile) -> f64| -> Result<Bilinear, InterpError> {
            let vals: Vec<f64> = nodes.iter().map(f).collect();
            Bilinear::new(&expiry_times, &tenor_years, &vals)
        };
        Ok(Self {
            atm: grid(&|n| n.atm_vol)?,
            rho: grid(&|n| n.sabr.rho)?,
            nu: grid(&|n| n.sabr.nu)?,
            reference,
            expiries: expiries.to_vec(),
            tenors: tenors.to_vec(),
            expiry_times,
            tenor_years,
            nodes,
        })
    }

    fn years(reference: Date, p: Period) -> Result<f64, DateError> {
        Ok(f64::from(p.add_to(reference)? - reference) / 365.0)
    }

    pub fn reference_date(&self) -> Date {
        self.reference
    }

    pub fn expiries(&self) -> &[Period] {
        &self.expiries
    }

    pub fn tenors(&self) -> &[Period] {
        &self.tenors
    }

    pub fn node(&self, expiry_idx: usize, tenor_idx: usize) -> &NodeSmile {
        &self.nodes[expiry_idx * self.tenors.len() + tenor_idx]
    }

    /// ATM normal vol at an arbitrary (expiry time, tenor years).
    pub fn atm_vol(&self, expiry_time: f64, tenor_years: f64) -> f64 {
        self.atm.eval(expiry_time, tenor_years)
    }

    /// Interpolated SABR parameters with alpha re-anchored so the smile's
    /// ATM vol equals the interpolated ATM vol (β = 0, so this does not
    /// depend on the forward).
    pub fn sabr_at(&self, expiry_time: f64, tenor_years: f64) -> Result<SabrParams, VolCubeError> {
        let atm = self.atm.eval(expiry_time, tenor_years);
        let rho = self.rho.eval(expiry_time, tenor_years).clamp(-0.999, 0.999);
        let nu = self.nu.eval(expiry_time, tenor_years).max(0.0);
        Ok(anchor_alpha(SabrParams::new(atm, 0.0, rho, nu, 0.0)?, atm, expiry_time.max(1e-8))?)
    }

    /// Normal vol for a swaption expiring at `expiry` on a `tenor` swap with
    /// the given forward and strike.
    pub fn normal_vol(&self, expiry: Date, tenor: Period, forward: f64, strike: f64) -> Result<f64, VolCubeError> {
        let t = f64::from(expiry - self.reference) / 365.0;
        if t <= 0.0 {
            return Err(VolCubeError::NonPositiveExpiry(expiry));
        }
        let ty = Self::years(self.reference, tenor)?;
        Ok(self.sabr_at(t, ty)?.normal_vol(forward, strike, t))
    }

    /// `VolQuote::Normal` ready for `Swaption::price`.
    pub fn quote(&self, expiry: Date, tenor: Period, forward: f64, strike: f64) -> Result<VolQuote, VolCubeError> {
        Ok(VolQuote::Normal(self.normal_vol(expiry, tenor, forward, strike)?))
    }

    /// Grid nodes `(expiry_idx, tenor_idx, weight)` an ATM query at
    /// `(expiry, tenor)` draws on. Weights sum to one.
    pub fn node_weights(&self, expiry: Date, tenor: Period) -> Result<Vec<(usize, usize, f64)>, VolCubeError> {
        let t = f64::from(expiry - self.reference) / 365.0;
        let ty = Self::years(self.reference, tenor)?;
        Ok(self.atm.node_weights(t, ty))
    }

    /// Same cube with one ATM node bumped by `shift` (rate units), smile
    /// parameters unchanged; for node-level vega by bump-and-reprice.
    pub fn with_atm_bump(&self, expiry_idx: usize, tenor_idx: usize, shift: f64) -> Result<Self, VolCubeError> {
        let mut c = self.clone();
        let k = expiry_idx * self.tenors.len() + tenor_idx;
        let n = &mut c.nodes[k];
        n.atm_vol += shift;
        n.sabr = anchor_alpha(n.sabr, n.atm_vol, self.expiry_times[expiry_idx])?;
        let vals: Vec<f64> = c.nodes.iter().map(|n| n.atm_vol).collect();
        c.atm = Bilinear::new(&self.expiry_times, &self.tenor_years, &vals)?;
        Ok(c)
    }
}

/// Rescale alpha so the normal-SABR ATM vol equals `atm` exactly. With
/// β = 0 the ATM vol is `α (1 + (2 − 3ρ²) ν² t / 24)`, so this is a division.
fn anchor_alpha(p: SabrParams, atm: f64, t: f64) -> Result<SabrParams, SabrError> {
    let corr = 1.0 + (2.0 - 3.0 * p.rho * p.rho) / 24.0 * p.nu * p.nu * t;
    SabrParams::new(atm / corr, 0.0, p.rho, p.nu, p.shift)
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_abs_diff_eq;

    type R = Result<(), Box<dyn std::error::Error>>;

    fn d(y: i32, m: u32, dd: u32) -> Date {
        Date::from_ymd(y, m, dd).expect("valid test date")
    }

    fn periods(s: &[&str]) -> Vec<Period> {
        s.iter().map(|p| p.parse().expect("period")).collect()
    }

    /// 2x3 grid, every node with a 5-point smile generated from a known SABR.
    fn cube() -> Result<SwaptionVolCube, VolCubeError> {
        let reference = d(2026, 10, 17);
        let expiries = periods(&["1Y", "5Y"]);
        let tenors = periods(&["2Y", "5Y", "10Y"]);
        let mut quotes = Vec::new();
        for (ei, e) in expiries.iter().enumerate() {
            let t = f64::from(e.add_to(reference).expect("date") - reference) / 365.0;
            for ti in 0..tenors.len() {
                let forward = 0.035 + 0.002 * ti as f64 + 0.001 * ei as f64;
                let atm = 0.0070 + 0.0005 * ti as f64 - 0.0003 * ei as f64;
                let truth = SabrParams::new(1.0, 0.0, -0.25 + 0.05 * ti as f64, 0.40 - 0.05 * ei as f64, 0.0).expect("sabr");
                let truth = anchor_alpha(truth, atm, t).expect("anchor");
                let smile: Vec<(f64, f64)> = [-0.01, -0.005, 0.005, 0.01].iter().map(|&dk| (dk, truth.normal_vol(forward, forward + dk, t))).collect();
                quotes.push(NodeQuotes { forward, atm_vol: atm, smile });
            }
        }
        SwaptionVolCube::new(reference, &expiries, &tenors, &quotes, None)
    }

    #[test]
    fn nodes_reprice_their_quotes() -> R {
        let c = cube()?;
        for ei in 0..2 {
            for ti in 0..3 {
                let n = c.node(ei, ti);
                assert!(n.rmse < 1e-7, "node ({ei},{ti}) rmse {}", n.rmse);
                let t = f64::from(c.expiries()[ei].add_to(c.reference_date())? - c.reference_date()) / 365.0;
                assert_abs_diff_eq!(n.sabr.normal_vol(n.forward, n.forward, t), n.atm_vol, epsilon = 1e-14);
            }
        }
        Ok(())
    }

    #[test]
    fn grid_queries_hit_nodes_and_interpolate_between() -> R {
        let c = cube()?;
        let reference = c.reference_date();
        let e1 = Period::years(1).add_to(reference)?;
        let n = c.node(0, 1);
        assert_abs_diff_eq!(c.normal_vol(e1, Period::years(5), n.forward, n.forward)?, n.atm_vol, epsilon = 1e-12);
        assert_abs_diff_eq!(c.normal_vol(e1, Period::years(5), n.forward, n.forward + 0.01)?, n.sabr.normal_vol(n.forward, n.forward + 0.01, 1.0), epsilon = 1e-10);
        // Halfway in expiry between 1Y and 5Y: ATM is the average of the nodes.
        let e3 = Period::years(3).add_to(reference)?;
        let mid = c.atm_vol(f64::from(e3 - reference) / 365.0, 5.0);
        let (a, b) = (c.node(0, 1).atm_vol, c.node(1, 1).atm_vol);
        assert!(mid < a.max(b) && mid > a.min(b));
        // Beyond the grid: flat.
        let e10 = Period::years(10).add_to(reference)?;
        assert_abs_diff_eq!(c.atm_vol(f64::from(e10 - reference) / 365.0, 5.0), c.node(1, 1).atm_vol, epsilon = 1e-15);
        Ok(())
    }

    #[test]
    fn smile_skews_with_rho_and_quote_wraps_normal() -> R {
        let c = cube()?;
        let e1 = Period::years(1).add_to(c.reference_date())?;
        let n = c.node(0, 0); // rho = -0.25
        let lo = c.normal_vol(e1, Period::years(2), n.forward, n.forward - 0.01)?;
        let hi = c.normal_vol(e1, Period::years(2), n.forward, n.forward + 0.01)?;
        assert!(lo > hi, "negative rho: low strikes richer");
        assert!(matches!(c.quote(e1, Period::years(2), n.forward, n.forward)?, VolQuote::Normal(v) if (v - n.atm_vol).abs() < 1e-12));
        Ok(())
    }

    #[test]
    fn node_weights_and_atm_bump() -> R {
        let c = cube()?;
        let reference = c.reference_date();
        let e3 = Period::years(3).add_to(reference)?;
        let w = c.node_weights(e3, Period::years(5))?;
        assert_eq!(w.len(), 2);
        assert_abs_diff_eq!(w.iter().map(|x| x.2).sum::<f64>(), 1.0, epsilon = 1e-15);
        assert!(w.iter().all(|&(_, ti, _)| ti == 1));
        let bumped = c.with_atm_bump(0, 1, 0.0001)?;
        let n = c.node(0, 1);
        let e1 = Period::years(1).add_to(reference)?;
        let before = c.normal_vol(e1, Period::years(5), n.forward, n.forward)?;
        let after = bumped.normal_vol(e1, Period::years(5), n.forward, n.forward)?;
        assert_abs_diff_eq!(after - before, 0.0001, epsilon = 1e-12);
        // Untouched node unchanged.
        assert_abs_diff_eq!(bumped.node(1, 2).atm_vol, c.node(1, 2).atm_vol, epsilon = 1e-15);
        Ok(())
    }

    #[test]
    fn default_smile_and_errors() -> R {
        let reference = d(2026, 10, 17);
        let (e, t) = (periods(&["1Y"]), periods(&["5Y"]));
        let bare = [NodeQuotes { forward: 0.04, atm_vol: 0.008, smile: vec![] }];
        assert!(matches!(SwaptionVolCube::new(reference, &e, &t, &bare, None), Err(VolCubeError::NoSmile { .. })));
        let c = SwaptionVolCube::new(reference, &e, &t, &bare, Some(DefaultSmile { rho: -0.2, nu: 0.3 }))?;
        let e1 = Period::years(1).add_to(reference)?;
        assert_abs_diff_eq!(c.normal_vol(e1, Period::years(5), 0.04, 0.04)?, 0.008, epsilon = 1e-14);
        assert!(matches!(SwaptionVolCube::new(reference, &e, &t, &[], None), Err(VolCubeError::NodeCount { .. })));
        assert!(matches!(c.normal_vol(reference, Period::years(5), 0.04, 0.04), Err(VolCubeError::NonPositiveExpiry(_))));
        Ok(())
    }
}
