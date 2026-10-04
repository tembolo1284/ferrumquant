//! Option formulas on a forward: Bachelier (normal) and shifted Black
//! (lognormal), with Greeks and implied volatility. Prices are per unit of
//! numeraire (e.g. per unit annuity for swaptions, per unit discount factor
//! for FX/commodity forwards), so callers multiply by their own numeraire.
//!
//! The normal CDF is Hart's double-precision rational approximation (as in
//! West, "Better approximations to cumulative normal functions"), accurate to
//! roughly 1e-14.

use std::f64::consts::{PI, SQRT_2};

use thiserror::Error;

use super::solver::{SolverConfig, SolverError, brent};

#[derive(Debug, Error, Clone, PartialEq)]
pub enum BlackError {
    #[error("invalid input: {0}")]
    Invalid(String),
    #[error("price {price} is outside the arbitrage bounds [{lower}, {upper}]")]
    PriceOutOfBounds { price: f64, lower: f64, upper: f64 },
    #[error(transparent)]
    Solver(#[from] SolverError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OptionType {
    Call,
    Put,
}

impl OptionType {
    /// +1 for calls, -1 for puts.
    pub const fn sign(self) -> f64 {
        match self {
            Self::Call => 1.0,
            Self::Put => -1.0,
        }
    }
}

/// Forward-measure price and Greeks per unit numeraire.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BlackResult {
    pub price: f64,
    /// dPrice/dForward
    pub delta: f64,
    /// d2Price/dForward2
    pub gamma: f64,
    /// dPrice/dVol (vol in the model's own units)
    pub vega: f64,
}

/// Standard normal density.
pub fn norm_pdf(x: f64) -> f64 {
    (-0.5 * x * x).exp() / (2.0 * PI).sqrt()
}

/// Standard normal CDF (Hart 1968 / West 2005).
pub fn norm_cdf(x: f64) -> f64 {
    let z = x.abs();
    let cum = if z > 37.0 {
        0.0
    } else {
        let e = (-z * z / 2.0).exp();
        if z < 7.071_067_811_865_47 {
            let mut num = 3.526_249_659_989_11e-2 * z + 0.700_383_064_443_688;
            num = num * z + 6.373_962_203_531_65;
            num = num * z + 33.912_866_078_383;
            num = num * z + 112.079_291_497_871;
            num = num * z + 221.213_596_169_931;
            num = num * z + 220.206_867_912_376;
            let mut den = 8.838_834_764_831_84e-2 * z + 1.755_667_163_182_64;
            den = den * z + 16.064_177_579_207;
            den = den * z + 86.780_732_202_946_1;
            den = den * z + 296.564_248_779_674;
            den = den * z + 637.333_633_378_831;
            den = den * z + 793.826_512_519_948;
            den = den * z + 440.413_735_824_752;
            e * num / den
        } else {
            let mut b = z + 0.65;
            b = z + 4.0 / b;
            b = z + 3.0 / b;
            b = z + 2.0 / b;
            b = z + 1.0 / b;
            e / b / 2.506_628_274_631
        }
    };
    if x > 0.0 { 1.0 - cum } else { cum }
}

/// Inverse of `norm_cdf` (Acklam's algorithm with one Newton refinement).
pub fn norm_inv(p: f64) -> f64 {
    const A: [f64; 6] = [-3.969_683_028_665_376e1, 2.209_460_984_245_205e2, -2.759_285_104_469_687e2, 1.383_577_518_672_690e2, -3.066_479_806_614_716e1, 2.506_628_277_459_239];
    const B: [f64; 5] = [-5.447_609_879_822_406e1, 1.615_858_368_580_409e2, -1.556_989_798_598_866e2, 6.680_131_188_771_972e1, -1.328_068_155_288_572e1];
    const C: [f64; 6] = [-7.784_894_002_430_293e-3, -3.223_964_580_411_365e-1, -2.400_758_277_161_838, -2.549_732_539_343_734, 4.374_664_141_464_968, 2.938_163_982_698_783];
    const D: [f64; 4] = [7.784_695_709_041_462e-3, 3.224_671_290_700_398e-1, 2.445_134_137_142_996, 3.754_408_661_907_416];
    if !(0.0..=1.0).contains(&p) {
        return f64::NAN;
    }
    if p == 0.0 {
        return f64::NEG_INFINITY;
    }
    if p == 1.0 {
        return f64::INFINITY;
    }
    let x = if p < 0.02425 {
        let q = (-2.0 * p.ln()).sqrt();
        (((((C[0] * q + C[1]) * q + C[2]) * q + C[3]) * q + C[4]) * q + C[5]) / ((((D[0] * q + D[1]) * q + D[2]) * q + D[3]) * q + 1.0)
    } else if p > 1.0 - 0.02425 {
        let q = (-2.0 * (1.0 - p).ln()).sqrt();
        -(((((C[0] * q + C[1]) * q + C[2]) * q + C[3]) * q + C[4]) * q + C[5]) / ((((D[0] * q + D[1]) * q + D[2]) * q + D[3]) * q + 1.0)
    } else {
        let q = p - 0.5;
        let r = q * q;
        (((((A[0] * r + A[1]) * r + A[2]) * r + A[3]) * r + A[4]) * r + A[5]) * q
            / (((((B[0] * r + B[1]) * r + B[2]) * r + B[3]) * r + B[4]) * r + 1.0)
    };
    // One Halley step brings this to full double precision.
    let e = norm_cdf(x) - p;
    let u = e * (2.0 * PI).sqrt() * (x * x / 2.0).exp();
    x - u / (1.0 + x * u / 2.0)
}

fn check(forward: f64, sigma: f64, t: f64) -> Result<(), BlackError> {
    if !forward.is_finite() || !sigma.is_finite() || !t.is_finite() {
        return Err(BlackError::Invalid("non-finite input".into()));
    }
    if sigma < 0.0 {
        return Err(BlackError::Invalid(format!("negative volatility {sigma}")));
    }
    if t < 0.0 {
        return Err(BlackError::Invalid(format!("negative time to expiry {t}")));
    }
    Ok(())
}

fn intrinsic(kind: OptionType, forward: f64, strike: f64) -> BlackResult {
    let moneyness = kind.sign() * (forward - strike);
    BlackResult {
        price: moneyness.max(0.0),
        delta: if moneyness > 0.0 { kind.sign() } else { 0.0 },
        gamma: 0.0,
        vega: 0.0,
    }
}

/// Bachelier (normal) model: dF = sigma dW. Vol in absolute rate units per sqrt(year).
pub fn bachelier(kind: OptionType, forward: f64, strike: f64, sigma: f64, t: f64) -> Result<BlackResult, BlackError> {
    check(forward, sigma, t)?;
    let sd = sigma * t.sqrt();
    if sd == 0.0 {
        return Ok(intrinsic(kind, forward, strike));
    }
    let d = (forward - strike) / sd;
    let s = kind.sign();
    let nd = norm_pdf(d);
    Ok(BlackResult {
        price: s * (forward - strike) * norm_cdf(s * d) + sd * nd,
        delta: s * norm_cdf(s * d),
        gamma: nd / sd,
        vega: t.sqrt() * nd,
    })
}

/// Shifted Black model: d(F + shift) = sigma (F + shift) dW. `shift` is 0 for
/// plain Black-76.
pub fn shifted_black(kind: OptionType, forward: f64, strike: f64, sigma: f64, t: f64, shift: f64) -> Result<BlackResult, BlackError> {
    check(forward, sigma, t)?;
    let (f, k) = (forward + shift, strike + shift);
    if f <= 0.0 || k <= 0.0 {
        return Err(BlackError::Invalid(format!("shifted forward {f} and strike {k} must be positive")));
    }
    let sd = sigma * t.sqrt();
    if sd == 0.0 {
        return Ok(intrinsic(kind, forward, strike));
    }
    let d1 = ((f / k).ln() + 0.5 * sd * sd) / sd;
    let d2 = d1 - sd;
    let s = kind.sign();
    Ok(BlackResult {
        price: s * (f * norm_cdf(s * d1) - k * norm_cdf(s * d2)),
        delta: s * norm_cdf(s * d1),
        gamma: norm_pdf(d1) / (f * sd),
        vega: f * norm_pdf(d1) * t.sqrt(),
    })
}

/// Plain Black-76 (zero shift).
pub fn black76(kind: OptionType, forward: f64, strike: f64, sigma: f64, t: f64) -> Result<BlackResult, BlackError> {
    shifted_black(kind, forward, strike, sigma, t, 0.0)
}

fn implied_vol(price: f64, lower: f64, upper: f64, hi_vol: f64, mut f: impl FnMut(f64) -> Result<f64, BlackError>) -> Result<f64, BlackError> {
    if !(lower - 1e-14..=upper + 1e-14).contains(&price) {
        return Err(BlackError::PriceOutOfBounds { price, lower, upper });
    }
    if price - lower <= 1e-14 {
        return Ok(0.0);
    }
    let mut err = None;
    let objective = |v: f64| match f(v) {
        Ok(p) => p - price,
        Err(e) => {
            err = Some(e);
            f64::NAN
        }
    };
    let cfg = SolverConfig { tolerance: 1e-12, max_iterations: 200 };
    let v = brent(objective, 0.0, hi_vol, cfg);
    match (v, err) {
        (_, Some(e)) => Err(e),
        (Ok(v), None) => Ok(v),
        (Err(e), None) => Err(e.into()),
    }
}

/// Normal vol matching `price` (per unit numeraire). Upper search bound is
/// `hi_vol` in absolute rate units (e.g. 1.0 = 10,000bp).
pub fn bachelier_implied_vol(kind: OptionType, forward: f64, strike: f64, t: f64, price: f64, hi_vol: f64) -> Result<f64, BlackError> {
    let lower = intrinsic(kind, forward, strike).price;
    implied_vol(price, lower, f64::INFINITY, hi_vol, |v| Ok(bachelier(kind, forward, strike, v, t)?.price))
}

/// Shifted-lognormal vol matching `price` (per unit numeraire).
pub fn shifted_black_implied_vol(kind: OptionType, forward: f64, strike: f64, t: f64, shift: f64, price: f64, hi_vol: f64) -> Result<f64, BlackError> {
    let lower = intrinsic(kind, forward, strike).price;
    let upper = match kind {
        OptionType::Call => forward + shift,
        OptionType::Put => strike + shift,
    };
    implied_vol(price, lower, upper, hi_vol, |v| Ok(shifted_black(kind, forward, strike, v, t, shift)?.price))
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_abs_diff_eq;

    type R = Result<(), BlackError>;

    #[test]
    fn normal_cdf_reference_values() {
        assert_abs_diff_eq!(norm_cdf(0.0), 0.5, epsilon = 1e-15);
        assert_abs_diff_eq!(norm_cdf(1.96), 0.975_002_104_851_779_5, epsilon = 1e-13);
        assert_abs_diff_eq!(norm_cdf(-1.0), 0.158_655_253_931_457_07, epsilon = 1e-13);
        assert_abs_diff_eq!(norm_cdf(3.0), 0.998_650_101_968_369_9, epsilon = 1e-13);
        assert_abs_diff_eq!(norm_cdf(-8.0), 6.220_960_574_271_8e-16, epsilon = 1e-22);
        for x in [-3.0, -0.7, 0.0, 0.3, 2.5] {
            assert_abs_diff_eq!(norm_inv(norm_cdf(x)), x, epsilon = 1e-12);
        }
    }

    #[test]
    fn bachelier_parity_and_atm() -> R {
        let (f, k, v, t) = (0.04, 0.035, 0.008, 2.0);
        let c = bachelier(OptionType::Call, f, k, v, t)?;
        let p = bachelier(OptionType::Put, f, k, v, t)?;
        assert_abs_diff_eq!(c.price - p.price, f - k, epsilon = 1e-15);
        assert_abs_diff_eq!(c.delta - p.delta, 1.0, epsilon = 1e-15);
        let atm = bachelier(OptionType::Call, f, f, v, t)?;
        assert_abs_diff_eq!(atm.price, v * t.sqrt() / (2.0 * PI).sqrt(), epsilon = 1e-15);
        assert_abs_diff_eq!(atm.delta, 0.5, epsilon = 1e-15);
        Ok(())
    }

    #[test]
    fn bachelier_greeks_match_finite_differences() -> R {
        let (f, k, v, t, h) = (0.04, 0.045, 0.007, 1.5, 1e-6);
        let base = bachelier(OptionType::Put, f, k, v, t)?;
        let up = bachelier(OptionType::Put, f + h, k, v, t)?.price;
        let dn = bachelier(OptionType::Put, f - h, k, v, t)?.price;
        assert_abs_diff_eq!(base.delta, (up - dn) / (2.0 * h), epsilon = 1e-7);
        assert_abs_diff_eq!(base.gamma, (up + dn - 2.0 * base.price) / (h * h), epsilon = 1e-3);
        let vup = bachelier(OptionType::Put, f, k, v + h, t)?.price;
        let vdn = bachelier(OptionType::Put, f, k, v - h, t)?.price;
        assert_abs_diff_eq!(base.vega, (vup - vdn) / (2.0 * h), epsilon = 1e-7);
        Ok(())
    }

    #[test]
    fn black76_reference_and_parity() -> R {
        // F=100, K=95, sigma=25%, T=0.5 -> undiscounted call 9.65336 (erfc reference).
        let c = black76(OptionType::Call, 100.0, 95.0, 0.25, 0.5)?;
        assert_abs_diff_eq!(c.price, 9.653_359_842, epsilon = 1e-8);
        let p = black76(OptionType::Put, 100.0, 95.0, 0.25, 0.5)?;
        assert_abs_diff_eq!(c.price - p.price, 5.0, epsilon = 1e-12);
        Ok(())
    }

    #[test]
    fn shifted_black_handles_negative_rates() -> R {
        let r = shifted_black(OptionType::Call, -0.002, -0.001, 0.30, 1.0, 0.03)?;
        assert!(r.price > 0.0 && r.delta > 0.0 && r.delta < 1.0);
        assert!(shifted_black(OptionType::Call, -0.05, 0.0, 0.3, 1.0, 0.03).is_err());
        Ok(())
    }

    #[test]
    fn implied_vols_round_trip() -> R {
        let (f, k, t) = (0.04, 0.038, 3.0);
        let price = bachelier(OptionType::Call, f, k, 0.0065, t)?.price;
        assert_abs_diff_eq!(bachelier_implied_vol(OptionType::Call, f, k, t, price, 1.0)?, 0.0065, epsilon = 1e-10);
        let price = shifted_black(OptionType::Put, f, k, 0.22, t, 0.01)?.price;
        assert_abs_diff_eq!(shifted_black_implied_vol(OptionType::Put, f, k, t, 0.01, price, 5.0)?, 0.22, epsilon = 1e-10);
        assert!(matches!(bachelier_implied_vol(OptionType::Call, f, k, t, 0.0001, 1.0), Err(BlackError::PriceOutOfBounds { .. })));
        Ok(())
    }

    #[test]
    fn zero_vol_or_time_is_intrinsic() -> R {
        let r = bachelier(OptionType::Put, 0.03, 0.035, 0.0, 2.0)?;
        assert_abs_diff_eq!(r.price, 0.005, epsilon = 1e-15);
        assert_abs_diff_eq!(r.delta, -1.0, epsilon = 1e-15);
        let r = black76(OptionType::Call, 100.0, 95.0, 0.3, 0.0)?;
        assert_abs_diff_eq!(r.price, 5.0, epsilon = 1e-15);
        assert!(bachelier(OptionType::Call, 0.03, 0.03, -0.1, 1.0).is_err());
        Ok(())
    }
}
