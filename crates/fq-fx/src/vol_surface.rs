//! FX volatility surface from market smile quotes.
//!
//! Desks quote each expiry as an ATM vol plus risk reversals and butterflies
//! at 25 and (optionally) 10 delta:
//!
//! ```text
//! RR_25 = sigma(25d call) - sigma(25d put)
//! BF_25 = [sigma(25d call) + sigma(25d put)] / 2 - sigma(ATM)
//! ```
//!
//! This is the *smile strangle* reading of the butterfly, which is what
//! most data vendors publish; the broker (market) strangle quoted by some
//! interdealer brokers for 10 delta is not supported, so BF_10 is read the
//! same way. Each quote's strike comes from its own vol under the pair's
//! delta and ATM conventions (`vanilla.rs`), the resulting strike/vol
//! points are fitted with lognormal SABR (`beta` fixed, default 1), and a
//! query between pillars interpolates total variance `sigma^2 tau` at the
//! requested strike. Outside the quoted expiries the nearest pillar's smile
//! applies (flat extrapolation).
//!
//! Nothing here checks butterfly or calendar arbitrage; a 3-point smile is
//! fitted exactly, a 5-point one in least squares, and the fit quality is
//! reported per pillar as `rmse`.

use fq_core::math::sabr::{SabrParams, SabrVolType, calibrate};
use fq_core::math::{OptionType, SolverConfig, brent};
use fq_core::time::{BusinessDayConvention, Date, Period};

use crate::currency::CurrencyPair;
use crate::forward::{FxError, FxMarket};
use crate::vanilla::{AtmConvention, DeltaConvention, FxSlice, FxVanilla, FxVanillaValuation, PremiumCurrency, expiry_and_delivery};

/// Market quotes for one expiry, all vols as decimals (0.085 = 8.5%).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SmileQuote {
    pub tenor: Period,
    pub atm: f64,
    pub rr25: f64,
    pub bf25: f64,
    pub rr10: Option<f64>,
    pub bf10: Option<f64>,
}

impl SmileQuote {
    pub const fn new(tenor: Period, atm: f64, rr25: f64, bf25: f64) -> Self {
        Self { tenor, atm, rr25, bf25, rr10: None, bf10: None }
    }

    pub const fn with_10d(mut self, rr10: f64, bf10: f64) -> Self {
        self.rr10 = Some(rr10);
        self.bf10 = Some(bf10);
        self
    }

    /// `(delta, call vol, put vol)` for each quoted delta pair.
    fn wing_vols(&self) -> Result<Vec<(f64, f64, f64)>, FxError> {
        let mut out = vec![(0.25, self.atm + self.bf25 + 0.5 * self.rr25, self.atm + self.bf25 - 0.5 * self.rr25)];
        match (self.rr10, self.bf10) {
            (Some(rr), Some(bf)) => out.push((0.10, self.atm + bf + 0.5 * rr, self.atm + bf - 0.5 * rr)),
            (None, None) => {}
            _ => return Err(FxError::BadSmileQuote { tenor: self.tenor }),
        }
        for &(_, c, p) in &out {
            if !(c > 0.0 && p > 0.0 && c.is_finite() && p.is_finite()) {
                return Err(FxError::BadSmileQuote { tenor: self.tenor });
            }
        }
        if !(self.atm > 0.0 && self.atm.is_finite()) {
            return Err(FxError::BadSmileQuote { tenor: self.tenor });
        }
        Ok(out)
    }
}

/// Quoting conventions a surface is built under.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SurfaceConfig {
    pub premium: PremiumCurrency,
    pub atm: AtmConvention,
    /// Fixed delta convention for every expiry; `None` applies the market
    /// rule (spot delta to 1Y, forward delta beyond).
    pub delta: Option<DeltaConvention>,
    /// SABR beta held fixed in the fit.
    pub beta: f64,
}

impl SurfaceConfig {
    pub fn market_default(pair: CurrencyPair) -> Self {
        let premium = PremiumCurrency::market_default(pair);
        Self { premium, atm: AtmConvention::market_default(premium), delta: None, beta: 1.0 }
    }

    fn delta_for(&self, tenor: Period) -> DeltaConvention {
        self.delta.unwrap_or_else(|| DeltaConvention::market_default(self.premium, tenor))
    }
}

/// One calibrated expiry.
#[derive(Debug, Clone, PartialEq)]
pub struct FxSmile {
    pub tenor: Period,
    pub expiry: Date,
    pub delivery: Date,
    pub slice: FxSlice,
    pub delta_convention: DeltaConvention,
    pub atm_strike: f64,
    pub atm_vol: f64,
    /// `(signed delta, strike, vol)` of the quoted points, puts negative; ATM has delta 0.
    pub points: Vec<(f64, f64, f64)>,
    pub sabr: SabrParams,
    /// RMS vol error of the SABR fit over the quoted points.
    pub rmse: f64,
}

impl FxSmile {
    /// Lognormal vol at a strike.
    pub fn vol(&self, strike: f64) -> Result<f64, FxError> {
        Ok(self.sabr.lognormal_vol(self.slice.forward, strike, self.slice.time_to_expiry)?)
    }

    /// Strike whose delta under this smile's convention is `delta` (calls
    /// positive, puts negative), by fixed-point iteration on the smile vol.
    pub fn strike_for_delta(&self, kind: OptionType, delta: f64) -> Result<f64, FxError> {
        strike_on_smile(&self.slice, self.delta_convention, kind, delta, self.atm_vol, |k| self.vol(k))
    }
}

fn strike_on_smile(
    slice: &FxSlice,
    convention: DeltaConvention,
    kind: OptionType,
    delta: f64,
    vol0: f64,
    vol_at: impl Fn(f64) -> Result<f64, FxError>,
) -> Result<f64, FxError> {
    let mut k = slice.strike_from_delta(kind, delta, vol0, convention)?;
    for _ in 0..100 {
        let next = slice.strike_from_delta(kind, delta, vol_at(k)?, convention)?;
        if (next - k).abs() <= 1e-13 * k.max(1.0) {
            return Ok(next);
        }
        k = next;
    }
    // Fixed point stalled: finish with a bracketed solve on the delta residual.
    let g = |x: f64| vol_at(x).and_then(|v| slice.delta(kind, x, v, convention)).map_or(f64::NAN, |d| d - delta);
    brent(g, k * 0.8, k * 1.25, SolverConfig { tolerance: 1e-13, max_iterations: 200 }).map_err(FxError::Solver)
}

/// Smiles on market tenors with total-variance interpolation between them.
#[derive(Debug, Clone, PartialEq)]
pub struct FxVolSurface {
    market: FxMarket,
    config: SurfaceConfig,
    smiles: Vec<FxSmile>,
}

impl FxVolSurface {
    /// Build with the pair's market conventions.
    pub fn new(market: &FxMarket, quotes: &[SmileQuote]) -> Result<Self, FxError> {
        Self::with_config(market, quotes, SurfaceConfig::market_default(market.pair()))
    }

    pub fn with_config(market: &FxMarket, quotes: &[SmileQuote], config: SurfaceConfig) -> Result<Self, FxError> {
        if quotes.is_empty() {
            return Err(FxError::NoExpiries);
        }
        let mut smiles: Vec<FxSmile> = quotes.iter().map(|q| calibrate_smile(market, q, &config)).collect::<Result<_, _>>()?;
        smiles.sort_by(|a, b| a.slice.time_to_expiry.partial_cmp(&b.slice.time_to_expiry).unwrap_or(std::cmp::Ordering::Equal));
        Ok(Self { market: market.clone(), config, smiles })
    }

    pub const fn market(&self) -> &FxMarket {
        &self.market
    }

    pub const fn config(&self) -> &SurfaceConfig {
        &self.config
    }

    pub fn smiles(&self) -> &[FxSmile] {
        &self.smiles
    }

    pub fn smile(&self, tenor: Period) -> Option<&FxSmile> {
        self.smiles.iter().find(|s| s.tenor == tenor)
    }

    /// Forward, discount and time for an arbitrary expiry (delivery is the
    /// spot-lag-adjusted date after expiry).
    pub fn slice_at(&self, expiry: Date) -> Result<FxSlice, FxError> {
        let delivery = delivery_for_expiry(self.market.pair(), expiry)?;
        FxVanilla::new(self.market.pair(), OptionType::Call, 1.0, expiry, delivery, 1.0)?.slice(&self.market)
    }

    /// Lognormal vol at an expiry date and strike.
    pub fn vol(&self, expiry: Date, strike: f64) -> Result<f64, FxError> {
        let t = f64::from(expiry - self.market.trade_date()) / 365.0;
        if t <= 0.0 {
            return Err(FxError::Expired { expiry, trade_date: self.market.trade_date() });
        }
        let n = self.smiles.len();
        let first = &self.smiles[0];
        let last = &self.smiles[n - 1];
        if t <= first.slice.time_to_expiry {
            return first.vol(strike);
        }
        if t >= last.slice.time_to_expiry {
            return last.vol(strike);
        }
        let i = self.smiles.partition_point(|s| s.slice.time_to_expiry <= t);
        let (a, b) = (&self.smiles[i - 1], &self.smiles[i]);
        let (ta, tb) = (a.slice.time_to_expiry, b.slice.time_to_expiry);
        let w = (t - ta) / (tb - ta);
        let va = a.vol(strike)?.powi(2) * ta;
        let vb = b.vol(strike)?.powi(2) * tb;
        Ok((((1.0 - w) * va + w * vb) / t).sqrt())
    }

    /// Vol and strike at an expiry for a signed delta under the convention
    /// the surface would apply to that expiry.
    pub fn vol_for_delta(&self, expiry: Date, kind: OptionType, delta: f64) -> Result<(f64, f64), FxError> {
        let slice = self.slice_at(expiry)?;
        let months = ((f64::from(expiry - self.market.trade_date()) / 30.4375).round() as i32).max(1);
        let convention = self.config.delta_for(Period::months(months));
        let atm_guess = self.vol(expiry, slice.forward)?;
        let k = strike_on_smile(&slice, convention, kind, delta, atm_guess, |k| self.vol(expiry, k))?;
        Ok((k, self.vol(expiry, k)?))
    }

    /// Price a vanilla off the surface.
    pub fn value(&self, option: &FxVanilla) -> Result<FxVanillaValuation, FxError> {
        let vol = self.vol(option.expiry(), option.strike())?;
        option.value(&self.market, vol)
    }

    /// Same surface with every pillar's ATM vol shifted (smile shape kept), for vega.
    pub fn with_parallel_bump(&self, shift: f64) -> Result<Self, FxError> {
        let quotes: Vec<SmileQuote> = self.smiles.iter().map(|s| s.quote_with_atm(s.atm_vol + shift)).collect::<Result<_, _>>()?;
        Self::with_config(&self.market, &quotes, self.config)
    }
}

impl FxSmile {
    /// The quote this smile was built from, with a new ATM vol and the wings
    /// moved in parallel (RR and BF kept).
    fn quote_with_atm(&self, atm: f64) -> Result<SmileQuote, FxError> {
        let wing = |delta: f64| {
            let c = self.points.iter().find(|p| (p.0 - delta).abs() < 1e-12).map(|p| p.2);
            let p = self.points.iter().find(|p| (p.0 + delta).abs() < 1e-12).map(|p| p.2);
            c.zip(p).map(|(c, p)| (c - p, 0.5 * (c + p) - self.atm_vol))
        };
        let (rr25, bf25) = wing(0.25).ok_or(FxError::BadSmileQuote { tenor: self.tenor })?;
        let mut q = SmileQuote::new(self.tenor, atm, rr25, bf25);
        if let Some((rr10, bf10)) = wing(0.10) {
            q = q.with_10d(rr10, bf10);
        }
        Ok(q)
    }
}

fn calibrate_smile(market: &FxMarket, q: &SmileQuote, config: &SurfaceConfig) -> Result<FxSmile, FxError> {
    let wings = q.wing_vols()?;
    let (expiry, delivery) = expiry_and_delivery(market.pair(), market.trade_date(), q.tenor)?;
    let probe = FxVanilla::new(market.pair(), OptionType::Call, 1.0, expiry, delivery, 1.0)?;
    let slice = probe.slice(market)?;
    let convention = config.delta_for(q.tenor);
    let atm_strike = slice.atm_strike(config.atm, q.atm);

    let mut points = vec![(0.0, atm_strike, q.atm)];
    for (delta, call_vol, put_vol) in wings {
        points.push((delta, slice.strike_from_delta(OptionType::Call, delta, call_vol, convention)?, call_vol));
        points.push((-delta, slice.strike_from_delta(OptionType::Put, -delta, put_vol, convention)?, put_vol));
    }
    points.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));

    let strip: Vec<(f64, f64)> = points.iter().map(|p| (p.1, p.2)).collect();
    let (f, t) = (slice.forward, slice.time_to_expiry);
    let seed = (q.atm * f.powf(1.0 - config.beta), -q.rr25.signum() * 0.1 * if q.rr25 == 0.0 { 0.0 } else { 1.0 }, 0.5);
    let fit = calibrate(f, t, config.beta, 0.0, &strip, SabrVolType::Lognormal, Some(seed))?;
    if !fit.converged() {
        return Err(FxError::SmileNotConverged { tenor: q.tenor, rmse: fit.rmse });
    }
    Ok(FxSmile { tenor: q.tenor, expiry, delivery, slice, delta_convention: convention, atm_strike, atm_vol: q.atm, points, sabr: fit.params, rmse: fit.rmse })
}

/// Delivery date for an expiry: `spot_lag` business days later on the
/// pair's calendar, then a good settlement day in both currencies and USD.
pub fn delivery_for_expiry(pair: CurrencyPair, expiry: Date) -> Result<Date, FxError> {
    let candidate = pair.calendar().advance(expiry, pair.spot_lag())?;
    Ok(pair.settlement_calendar().adjust(candidate, BusinessDayConvention::Following)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_abs_diff_eq;

    use fq_core::math::OptionType::{Call, Put};
    use fq_rates::curve::DiscountCurve;

    type R = Result<(), FxError>;

    fn d(y: i32, m: u32, dd: u32) -> Date {
        Date::from_ymd(y, m, dd).expect("valid test date")
    }

    fn eurusd() -> Result<FxMarket, FxError> {
        let today = d(2026, 10, 13);
        let eur = DiscountCurve::flat(today, 0.02, d(2040, 1, 1))?;
        let usd = DiscountCurve::flat(today, 0.04, d(2040, 1, 1))?;
        FxMarket::new("EURUSD".parse()?, today, 1.1750, eur, usd)
    }

    fn usdjpy() -> Result<FxMarket, FxError> {
        let today = d(2026, 10, 13);
        let usd = DiscountCurve::flat(today, 0.04, d(2040, 1, 1))?;
        let jpy = DiscountCurve::flat(today, 0.005, d(2040, 1, 1))?;
        FxMarket::new("USDJPY".parse()?, today, 152.30, usd, jpy)
    }

    fn quotes() -> Vec<SmileQuote> {
        vec![
            SmileQuote::new("1M".parse().expect("tenor"), 0.0720, -0.0040, 0.0015),
            SmileQuote::new("3M".parse().expect("tenor"), 0.0750, -0.0060, 0.0020).with_10d(-0.0110, 0.0065),
            SmileQuote::new("1Y".parse().expect("tenor"), 0.0800, -0.0090, 0.0025).with_10d(-0.0165, 0.0080),
            SmileQuote::new("2Y".parse().expect("tenor"), 0.0820, -0.0100, 0.0028),
        ]
    }

    #[test]
    fn pillars_reprice_their_quotes() -> R {
        let m = eurusd()?;
        let s = FxVolSurface::new(&m, &quotes())?;
        assert_eq!(s.smiles().len(), 4);
        assert!(s.smiles().windows(2).all(|w| w[0].slice.time_to_expiry < w[1].slice.time_to_expiry));
        for smile in s.smiles() {
            assert!(smile.rmse < 5e-4, "{} rmse {}", smile.tenor, smile.rmse);
            // Each quoted point is reproduced by the fitted smile.
            for &(delta, k, v) in &smile.points {
                let tol = if smile.points.len() == 3 { 1e-7 } else { 1e-3 };
                assert_abs_diff_eq!(smile.vol(k)?, v, epsilon = tol);
                // And the strike really has that delta under the pillar's convention.
                if delta != 0.0 {
                    let kind = if delta > 0.0 { Call } else { Put };
                    assert_abs_diff_eq!(smile.slice.delta(kind, k, v, smile.delta_convention)?, delta, epsilon = 1e-10);
                }
            }
        }
        // Negative risk reversal: puts richer than calls.
        let one_y = s.smile("1Y".parse().expect("tenor")).expect("pillar");
        let kc = one_y.points.last().expect("call").1;
        let kp = one_y.points.first().expect("put").1;
        assert!(one_y.vol(kp)? > one_y.vol(kc)?);
        // Conventions: EURUSD is quote-premium, spot delta to 1Y, forward delta at 2Y, DNS ATM above the forward.
        assert_eq!(one_y.delta_convention, DeltaConvention::Spot);
        assert_eq!(s.smile("2Y".parse().expect("tenor")).expect("pillar").delta_convention, DeltaConvention::Forward);
        assert!(one_y.atm_strike > one_y.slice.forward);
        Ok(())
    }

    #[test]
    fn interpolation_hits_pillars_and_is_flat_outside() -> R {
        let m = eurusd()?;
        let s = FxVolSurface::new(&m, &quotes())?;
        let three_m = s.smile("3M".parse().expect("tenor")).expect("pillar");
        let k = three_m.atm_strike;
        assert_abs_diff_eq!(s.vol(three_m.expiry, k)?, three_m.vol(k)?, epsilon = 1e-15);
        // Six months sits between 3M and 1Y: total variance is between the two.
        let (e6, _) = expiry_and_delivery(m.pair(), m.trade_date(), Period::months(6))?;
        let one_y = s.smile("1Y".parse().expect("tenor")).expect("pillar");
        let v6 = s.vol(e6, 1.18)?;
        let (va, vb) = (three_m.vol(1.18)?, one_y.vol(1.18)?);
        assert!(v6 > va.min(vb) && v6 < va.max(vb), "{v6} not between {va} and {vb}");
        // Beyond 2Y and before 1M: nearest pillar.
        let two_y = s.smile("2Y".parse().expect("tenor")).expect("pillar");
        assert_abs_diff_eq!(s.vol(d(2031, 10, 15), 1.25)?, two_y.vol(1.25)?, epsilon = 1e-15);
        let one_m = s.smile("1M".parse().expect("tenor")).expect("pillar");
        assert_abs_diff_eq!(s.vol(d(2026, 10, 20), 1.17)?, one_m.vol(1.17)?, epsilon = 1e-15);
        assert!(matches!(s.vol(m.trade_date(), 1.17), Err(FxError::Expired { .. })));
        Ok(())
    }

    #[test]
    fn delta_queries_and_pricing() -> R {
        let m = eurusd()?;
        let s = FxVolSurface::new(&m, &quotes())?;
        let one_y = s.smile("1Y".parse().expect("tenor")).expect("pillar");
        // On a pillar, the 25d strikes from the smile match the calibration points.
        let k25c = one_y.strike_for_delta(Call, 0.25)?;
        let quoted = one_y.points.iter().find(|p| (p.0 - 0.25).abs() < 1e-12).expect("25d call").1;
        assert_abs_diff_eq!(k25c, quoted, epsilon = 2e-3);
        let (k, v) = s.vol_for_delta(one_y.expiry, Put, -0.25)?;
        assert_abs_diff_eq!(one_y.slice.delta(Put, k, v, DeltaConvention::Spot)?, -0.25, epsilon = 1e-10);
        // Pricing off the surface equals pricing with the surface vol.
        let opt = FxVanilla::new(m.pair(), Call, k25c, one_y.expiry, one_y.delivery, 1e6)?;
        let v = s.value(&opt)?;
        assert_abs_diff_eq!(v.pv_quote, opt.value(&m, s.vol(one_y.expiry, k25c)?)?.pv_quote, epsilon = 1e-9);
        assert_abs_diff_eq!(v.spot_delta, 0.25, epsilon = 2e-3);
        // Parallel bump moves every pillar's ATM by the shift and keeps the skew.
        let up = s.with_parallel_bump(0.01)?;
        for (a, b) in s.smiles().iter().zip(up.smiles()) {
            assert_abs_diff_eq!(b.atm_vol - a.atm_vol, 0.01, epsilon = 1e-15);
            let q = b.quote_with_atm(b.atm_vol)?;
            assert_abs_diff_eq!(q.rr25, a.quote_with_atm(a.atm_vol)?.rr25, epsilon = 1e-12);
        }
        Ok(())
    }

    #[test]
    fn premium_adjusted_pair_uses_pa_conventions() -> R {
        let m = usdjpy()?;
        let q = vec![
            SmileQuote::new("1M".parse().expect("tenor"), 0.0950, 0.0080, 0.0030),
            SmileQuote::new("1Y".parse().expect("tenor"), 0.1000, 0.0120, 0.0040).with_10d(0.0220, 0.0120),
        ];
        let s = FxVolSurface::new(&m, &q)?;
        let one_y = s.smile("1Y".parse().expect("tenor")).expect("pillar");
        assert_eq!(one_y.delta_convention, DeltaConvention::SpotPremiumAdjusted);
        assert!(one_y.atm_strike < one_y.slice.forward, "pa DNS strike sits below the forward");
        for &(delta, k, v) in &one_y.points {
            if delta != 0.0 {
                let kind = if delta > 0.0 { Call } else { Put };
                assert_abs_diff_eq!(one_y.slice.delta(kind, k, v, DeltaConvention::SpotPremiumAdjusted)?, delta, epsilon = 1e-10);
            }
        }
        // Positive RR for USDJPY in this quote set: calls richer than puts.
        let kc = one_y.points.last().expect("call").1;
        let kp = one_y.points.first().expect("put").1;
        assert!(one_y.vol(kc)? > one_y.vol(kp)?);
        Ok(())
    }

    #[test]
    fn bad_quotes_are_rejected() -> R {
        let m = eurusd()?;
        assert!(matches!(FxVolSurface::new(&m, &[]), Err(FxError::NoExpiries)));
        let half = SmileQuote { rr10: Some(-0.01), ..SmileQuote::new("3M".parse().expect("tenor"), 0.075, -0.006, 0.002) };
        assert!(matches!(FxVolSurface::new(&m, &[half]), Err(FxError::BadSmileQuote { .. })));
        let neg = SmileQuote::new("3M".parse().expect("tenor"), 0.01, -0.05, 0.0);
        assert!(matches!(FxVolSurface::new(&m, &[neg]), Err(FxError::BadSmileQuote { .. })));
        Ok(())
    }
}
