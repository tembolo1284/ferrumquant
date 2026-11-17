//! Two-asset spread options: `max(w1 F1 - w2 F2 - K, 0)`.
//!
//! This one payoff covers most of what a physical energy desk owns: a gas
//! plant is a strip of spark spreads (power minus heat rate times gas), a
//! refinery a crack (product minus crude), pipeline capacity a basis spread
//! (delivery hub minus receipt hub minus fuel), a storage cycle a calendar
//! spread. Three approximations are offered, all in the forward measure and
//! undiscounted so the caller applies its own discount factor:
//!
//! * **Margrabe** is exact when `K = 0`.
//! * **Kirk** (1995) treats `F2 + K` as a lognormal and is the market
//!   standard for small strikes relative to `F2`.
//! * **Bjerksund-Stensland** (2014) is a lower bound that is tighter than
//!   Kirk for large strikes and never worse; prefer it as the default.
//!
//! Weights let you express heat rates and conversion factors without
//! rescaling the curves: a 7.5 heat rate spark is `w1 = 1` on power, `w2 =
//! 7.5` on gas (MWh vs MMBtu), and the effective lognormal vols are
//! unchanged by a positive weight.

use fq_core::math::{OptionType, SolverConfig, brent, norm_cdf};
use fq_core::time::{Date, DayCount};
use fq_rates::curve::DiscountCurve;

use crate::contract::ContractMonth;
use crate::curve::FuturesCurve;
use crate::error::CommodError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SpreadMethod {
    Margrabe,
    Kirk,
    #[default]
    BjerksundStensland,
}

fn check_inputs(f1: f64, f2: f64, s1: f64, s2: f64, rho: f64, t: f64, k: f64) -> Result<(), CommodError> {
    for (name, x) in [("F1", f1), ("F2", f2), ("K", k), ("T", t)] {
        if !x.is_finite() {
            return Err(CommodError::Invalid(format!("non-finite {name} {x}")));
        }
    }
    if f1 <= 0.0 || f2 <= 0.0 {
        return Err(CommodError::Invalid(format!("forwards must be positive, got F1={f1} F2={f2}")));
    }
    for s in [s1, s2] {
        if !(s.is_finite() && s >= 0.0) {
            return Err(CommodError::BadVol(s));
        }
    }
    if !(rho.is_finite() && (-1.0..=1.0).contains(&rho)) {
        return Err(CommodError::BadCorrelation(rho));
    }
    if t < 0.0 {
        return Err(CommodError::Invalid(format!("negative time to expiry {t}")));
    }
    Ok(())
}

fn intrinsic(f1: f64, f2: f64, k: f64) -> f64 {
    (f1 - f2 - k).max(0.0)
}

/// Exchange option `max(F1 - F2, 0)` (Margrabe 1978), exact. `k` is ignored
/// except that a non-zero value is an error, to stop silent misuse.
pub fn margrabe_call(f1: f64, f2: f64, s1: f64, s2: f64, rho: f64, t: f64) -> Result<f64, CommodError> {
    check_inputs(f1, f2, s1, s2, rho, t, 0.0)?;
    let var = s1 * s1 - 2.0 * rho * s1 * s2 + s2 * s2;
    let sd = (var.max(0.0) * t).sqrt();
    if sd == 0.0 {
        return Ok(intrinsic(f1, f2, 0.0));
    }
    let d1 = ((f1 / f2).ln() + 0.5 * sd * sd) / sd;
    Ok(f1 * norm_cdf(d1) - f2 * norm_cdf(d1 - sd))
}

/// Kirk's approximation for `max(F1 - F2 - K, 0)`, `K >= 0`.
pub fn kirk_call(f1: f64, f2: f64, s1: f64, s2: f64, rho: f64, t: f64, k: f64) -> Result<f64, CommodError> {
    check_inputs(f1, f2, s1, s2, rho, t, k)?;
    if k < 0.0 {
        return Err(CommodError::Invalid(format!("Kirk requires K >= 0, got {k}")));
    }
    let a = f2 + k;
    let b = f2 / a;
    let var = s1 * s1 - 2.0 * rho * s1 * s2 * b + s2 * s2 * b * b;
    let sd = (var.max(0.0) * t).sqrt();
    if sd == 0.0 {
        return Ok(intrinsic(f1, f2, k));
    }
    let d1 = ((f1 / a).ln() + 0.5 * sd * sd) / sd;
    Ok(f1 * norm_cdf(d1) - a * norm_cdf(d1 - sd))
}

/// Bjerksund-Stensland (2014) closed-form lower bound for
/// `max(F1 - F2 - K, 0)`, `K >= 0`. Exact for `K = 0`.
pub fn bjerksund_stensland_call(f1: f64, f2: f64, s1: f64, s2: f64, rho: f64, t: f64, k: f64) -> Result<f64, CommodError> {
    check_inputs(f1, f2, s1, s2, rho, t, k)?;
    if k < 0.0 {
        return Err(CommodError::Invalid(format!("Bjerksund-Stensland requires K >= 0, got {k}")));
    }
    let a = f2 + k;
    let b = f2 / a;
    let var = s1 * s1 - 2.0 * b * rho * s1 * s2 + b * b * s2 * s2;
    let sd = (var.max(0.0) * t).sqrt();
    if sd == 0.0 {
        return Ok(intrinsic(f1, f2, k));
    }
    let lf = (f1 / a).ln();
    let d1 = (lf + (0.5 * s1 * s1 - b * rho * s1 * s2 + 0.5 * b * b * s2 * s2) * t) / sd;
    let d2 = (lf + (-0.5 * s1 * s1 + rho * s1 * s2 + 0.5 * b * b * s2 * s2 - b * s2 * s2) * t) / sd;
    let d3 = (lf + (-0.5 * s1 * s1 + 0.5 * b * b * s2 * s2) * t) / sd;
    Ok(f1 * norm_cdf(d1) - f2 * norm_cdf(d2) - k * norm_cdf(d3))
}

/// Undiscounted spread option price for either side via put-call parity on
/// the forward spread: `put = call - (F1 - F2 - K)`.
#[allow(clippy::too_many_arguments)]
pub fn spread_price(method: SpreadMethod, kind: OptionType, f1: f64, f2: f64, s1: f64, s2: f64, rho: f64, t: f64, k: f64) -> Result<f64, CommodError> {
    let call = match method {
        SpreadMethod::Margrabe => {
            if k != 0.0 {
                return Err(CommodError::Invalid(format!("Margrabe is only exact for K = 0, got {k}; use Kirk or Bjerksund-Stensland")));
            }
            margrabe_call(f1, f2, s1, s2, rho, t)?
        }
        SpreadMethod::Kirk => kirk_call(f1, f2, s1, s2, rho, t, k)?,
        SpreadMethod::BjerksundStensland => bjerksund_stensland_call(f1, f2, s1, s2, rho, t, k)?,
    };
    Ok(match kind {
        OptionType::Call => call,
        OptionType::Put => call - (f1 - f2 - k),
    })
}

/// Correlation that reproduces an undiscounted spread premium. Spread
/// option price is decreasing in `rho`, so a bracket on [-1, 1] suffices.
#[allow(clippy::too_many_arguments)]
pub fn implied_correlation(method: SpreadMethod, kind: OptionType, f1: f64, f2: f64, s1: f64, s2: f64, t: f64, k: f64, price: f64) -> Result<f64, CommodError> {
    let lo = spread_price(method, kind, f1, f2, s1, s2, 1.0, t, k)?;
    let hi = spread_price(method, kind, f1, f2, s1, s2, -1.0, t, k)?;
    if !(lo - 1e-12..=hi + 1e-12).contains(&price) {
        return Err(CommodError::Invalid(format!("price {price} outside attainable range [{lo}, {hi}] for rho in [-1, 1]")));
    }
    let mut err = None;
    let obj = |rho: f64| match spread_price(method, kind, f1, f2, s1, s2, rho, t, k) {
        Ok(p) => p - price,
        Err(e) => {
            err = Some(e);
            f64::NAN
        }
    };
    let rho = brent(obj, -1.0, 1.0, SolverConfig { tolerance: 1e-12, max_iterations: 200 });
    match (rho, err) {
        (_, Some(e)) => Err(e),
        (Ok(r), None) => Ok(r),
        (Err(e), None) => Err(e.into()),
    }
}

/// A spread option on two contract months, possibly at different hubs.
/// Payoff per unit: `max(sign * (w1 F1 - w2 F2 - K), 0)`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpreadOption {
    pub kind: OptionType,
    pub long_contract: ContractMonth,
    pub long_weight: f64,
    pub short_contract: ContractMonth,
    pub short_weight: f64,
    pub strike: f64,
    pub expiry: Date,
    pub quantity: f64,
    pub method: SpreadMethod,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpreadValuation {
    /// `w1 F1` and `w2 F2`.
    pub long_forward: f64,
    pub short_forward: f64,
    pub forward_spread: f64,
    pub time_to_expiry: f64,
    pub discount: f64,
    pub forward_price: f64,
    pub pv: f64,
    /// dPV / dF1 and dPV / dF2 per unit price move on the raw (unweighted)
    /// forwards, by central difference, scaled by quantity.
    pub delta_long: f64,
    pub delta_short: f64,
    /// dPV / d(rho) by central difference.
    pub corr_sensitivity: f64,
}

impl SpreadOption {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        kind: OptionType,
        long_contract: ContractMonth,
        long_weight: f64,
        short_contract: ContractMonth,
        short_weight: f64,
        strike: f64,
        expiry: Date,
        quantity: f64,
    ) -> Result<Self, CommodError> {
        for (name, w) in [("long weight", long_weight), ("short weight", short_weight), ("quantity", quantity)] {
            if !(w.is_finite() && w > 0.0) {
                return Err(CommodError::Invalid(format!("{name} {w} must be positive")));
            }
        }
        if !strike.is_finite() || strike < 0.0 {
            return Err(CommodError::Invalid(format!("strike {strike} must be finite and non-negative")));
        }
        let limit = long_contract.first_day()?.min(short_contract.first_day()?);
        if expiry > limit {
            return Err(CommodError::ExpiryAfterContract { expiry, limit });
        }
        Ok(Self { kind, long_contract, long_weight, short_contract, short_weight, strike, expiry, quantity, method: SpreadMethod::default() })
    }

    pub fn with_method(mut self, method: SpreadMethod) -> Self {
        self.method = method;
        self
    }

    /// `long` and `short` curves may be the same curve (calendar spread) or
    /// different hubs (basis, spark, crack). Vols are the Black vols of the
    /// two contracts; `rho` their log-return correlation.
    #[allow(clippy::too_many_arguments)]
    pub fn value(
        &self,
        long: &FuturesCurve,
        short: &FuturesCurve,
        long_vol: f64,
        short_vol: f64,
        rho: f64,
        discount: &DiscountCurve,
        dc: DayCount,
    ) -> Result<SpreadValuation, CommodError> {
        let valuation = long.reference_date();
        if short.reference_date() != valuation {
            return Err(CommodError::CurveMismatch(long.hub().to_string(), short.hub().to_string()));
        }
        if self.expiry <= valuation {
            return Err(CommodError::Expired { expiry: self.expiry, valuation });
        }
        let t = dc.year_fraction(valuation, self.expiry, None)?;
        let f1_raw = long.price(self.long_contract)?;
        let f2_raw = short.price(self.short_contract)?;
        let df = discount.discount(self.expiry)?;
        let scale = self.quantity * df;

        let price_at = |f1: f64, f2: f64, r: f64| {
            spread_price(self.method, self.kind, self.long_weight * f1, self.short_weight * f2, long_vol, short_vol, r, t, self.strike)
        };
        let forward_price = price_at(f1_raw, f2_raw, rho)?;

        let h1 = 1e-4 * f1_raw;
        let h2 = 1e-4 * f2_raw;
        let delta_long = (price_at(f1_raw + h1, f2_raw, rho)? - price_at(f1_raw - h1, f2_raw, rho)?) / (2.0 * h1) * scale;
        let delta_short = (price_at(f1_raw, f2_raw + h2, rho)? - price_at(f1_raw, f2_raw - h2, rho)?) / (2.0 * h2) * scale;
        let hr = 1e-4;
        let (r_up, r_dn) = ((rho + hr).min(1.0), (rho - hr).max(-1.0));
        let corr_sensitivity = (price_at(f1_raw, f2_raw, r_up)? - price_at(f1_raw, f2_raw, r_dn)?) / (r_up - r_dn) * scale;

        Ok(SpreadValuation {
            long_forward: self.long_weight * f1_raw,
            short_forward: self.short_weight * f2_raw,
            forward_spread: self.long_weight * f1_raw - self.short_weight * f2_raw,
            time_to_expiry: t,
            discount: df,
            forward_price,
            pv: forward_price * scale,
            delta_long,
            delta_short,
            corr_sensitivity,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::{Commodity, Hub, Unit};
    use crate::curve::{CurveInterpolation, FuturesQuote};
    use crate::model::Xoshiro256StarStar;
    use approx::assert_abs_diff_eq;
    use fq_rates::curve::Interpolation;

    fn ymd(y: i32, m: u32, d: u32) -> Date {
        Date::from_ymd(y, m, d).expect("valid date")
    }

    fn cm(y: i32, m: u32) -> ContractMonth {
        ContractMonth::new(y, m).expect("month")
    }

    #[allow(clippy::too_many_arguments)]
    fn mc_spread_call(f1: f64, f2: f64, s1: f64, s2: f64, rho: f64, t: f64, k: f64, paths: usize, seed: u64) -> f64 {
        let mut rng = Xoshiro256StarStar::seed_from_u64(seed);
        let mut sum = 0.0;
        let c = (1.0 - rho * rho).sqrt();
        for _ in 0..paths / 2 {
            let z1 = rng.next_gaussian();
            let z2 = rho * z1 + c * rng.next_gaussian();
            for sgn in [1.0, -1.0] {
                let x1 = f1 * (-0.5 * s1 * s1 * t + s1 * t.sqrt() * sgn * z1).exp();
                let x2 = f2 * (-0.5 * s2 * s2 * t + s2 * t.sqrt() * sgn * z2).exp();
                sum += (x1 - x2 - k).max(0.0);
            }
        }
        sum / paths as f64
    }

    #[test]
    fn zero_strike_methods_agree_with_margrabe() {
        let (f1, f2, s1, s2, rho, t) = (50.0, 42.0, 0.55, 0.45, 0.6, 0.5);
        let m = margrabe_call(f1, f2, s1, s2, rho, t).expect("m");
        let k = kirk_call(f1, f2, s1, s2, rho, t, 0.0).expect("k");
        let b = bjerksund_stensland_call(f1, f2, s1, s2, rho, t, 0.0).expect("b");
        assert_abs_diff_eq!(m, k, epsilon = 1e-12);
        assert_abs_diff_eq!(m, b, epsilon = 1e-12);
        // Margrabe with F2 fixed (s2=0) is Black on F1 with strike F2.
        let black = fq_core::math::black76(OptionType::Call, f1, f2, s1, t).expect("black").price;
        assert_abs_diff_eq!(margrabe_call(f1, f2, s1, 0.0, 0.0, t).expect("m"), black, epsilon = 1e-12);
    }

    #[test]
    fn kirk_and_bs_track_monte_carlo() {
        // Spark spread: power 60 $/MWh, gas 4 $/MMBtu * 7.5 HR = 30, VOM 3.
        let (f1, f2, s1, s2, rho, t, k) = (60.0, 30.0, 0.70, 0.55, 0.45, 0.25, 3.0);
        let mc = mc_spread_call(f1, f2, s1, s2, rho, t, k, 2_000_000, 7);
        let kirk = kirk_call(f1, f2, s1, s2, rho, t, k).expect("kirk");
        let bs = bjerksund_stensland_call(f1, f2, s1, s2, rho, t, k).expect("bs");
        assert_abs_diff_eq!(kirk, mc, epsilon = 0.08);
        assert_abs_diff_eq!(bs, mc, epsilon = 0.08);
        // BS is a lower bound and should not exceed MC by more than noise.
        assert!(bs <= mc + 0.05);
        // Large strike: BS stays close, Kirk drifts.
        let k_big = 25.0;
        let mc_big = mc_spread_call(f1, f2, s1, s2, rho, t, k_big, 2_000_000, 11);
        let bs_big = bjerksund_stensland_call(f1, f2, s1, s2, rho, t, k_big).expect("bs");
        let kirk_big = kirk_call(f1, f2, s1, s2, rho, t, k_big).expect("kirk");
        assert!((bs_big - mc_big).abs() <= (kirk_big - mc_big).abs() + 0.02, "bs {bs_big} kirk {kirk_big} mc {mc_big}");
    }

    #[test]
    fn parity_monotonicity_and_implied_correlation() {
        let (f1, f2, s1, s2, t, k) = (60.0, 30.0, 0.70, 0.55, 0.25, 3.0);
        let call = spread_price(SpreadMethod::BjerksundStensland, OptionType::Call, f1, f2, s1, s2, 0.4, t, k).expect("c");
        let put = spread_price(SpreadMethod::BjerksundStensland, OptionType::Put, f1, f2, s1, s2, 0.4, t, k).expect("p");
        assert_abs_diff_eq!(call - put, f1 - f2 - k, epsilon = 1e-12);
        let lo_rho = spread_price(SpreadMethod::Kirk, OptionType::Call, f1, f2, s1, s2, -0.5, t, k).expect("c");
        let hi_rho = spread_price(SpreadMethod::Kirk, OptionType::Call, f1, f2, s1, s2, 0.9, t, k).expect("c");
        assert!(lo_rho > hi_rho);
        let rho = implied_correlation(SpreadMethod::BjerksundStensland, OptionType::Call, f1, f2, s1, s2, t, k, call).expect("rho");
        assert_abs_diff_eq!(rho, 0.4, epsilon = 1e-9);
        assert!(spread_price(SpreadMethod::Margrabe, OptionType::Call, f1, f2, s1, s2, 0.4, t, k).is_err());
        assert!(matches!(kirk_call(f1, f2, s1, s2, 1.2, t, k), Err(CommodError::BadCorrelation(_))));
    }

    #[test]
    fn spark_spread_on_curves() {
        let val = ymd(2026, 11, 9);
        let pjm = Hub::new("PJMW", Commodity::Power, Unit::MWh);
        let tetco = Hub::new("TETM3", Commodity::NaturalGas, Unit::MmBtu);
        let power = FuturesCurve::new(pjm, val, vec![FuturesQuote::new(cm(2027, 1), 95.0), FuturesQuote::new(cm(2027, 7), 55.0)], CurveInterpolation::Linear)
            .expect("pw");
        let gas = FuturesCurve::new(tetco, val, vec![FuturesQuote::new(cm(2027, 1), 8.0), FuturesQuote::new(cm(2027, 7), 3.0)], CurveInterpolation::Linear)
            .expect("gas");
        let nodes: Vec<(Date, f64)> = (1..=12).map(|k| (val.add_months(k).expect("d"), (-0.045 * k as f64 / 12.0).exp())).collect();
        let disc = DiscountCurve::new(val, &nodes, Interpolation::LogLinearDiscount).expect("disc");
        // Jan-27 7.5 HR spark, $3 VOM, on 16 hours x 21 days x 50 MW.
        let qty = 16.0 * 21.0 * 50.0;
        let opt = SpreadOption::new(OptionType::Call, cm(2027, 1), 1.0, cm(2027, 1), 7.5, 3.0, ymd(2026, 12, 28), qty).expect("opt");
        let v = opt.value(&power, &gas, 0.80, 0.90, 0.55, &disc, DayCount::Act365Fixed).expect("v");
        assert_abs_diff_eq!(v.forward_spread, 95.0 - 60.0, epsilon = 1e-12);
        assert!(v.forward_price > 35.0 - 3.0); // above intrinsic
        assert!(v.delta_long > 0.0 && v.delta_short < 0.0);
        assert!(v.corr_sensitivity < 0.0);
        assert_abs_diff_eq!(v.pv, v.forward_price * v.discount * qty, epsilon = 1e-6);
        // Delta on gas is roughly -7.5x delta on power scaled by respective sensitivities.
        assert!(v.delta_short.abs() > v.delta_long.abs() * 5.0);
    }
}
