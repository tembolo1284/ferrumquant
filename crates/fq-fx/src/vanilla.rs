//! Vanilla FX options: Garman-Kohlhagen on the FX forward, with the Greeks
//! and delta conventions the FX market quotes in.
//!
//! An option on `base/quote` struck at `K` with base notional `N` pays
//! `N * max(phi (S_T - K), 0)` in quote currency at delivery, so its value
//! at the trade date is `N * DF_q(T_d) * Black(F, K, sigma, tau)` with `F`
//! the outright to the delivery date, `DF_q` the quote-currency discount
//! factor from the trade date to delivery, and `tau` the ACT/365F time to
//! expiry. Premium can be expressed in quote currency (the default for
//! USD-quoted pairs such as EURUSD) or in base currency (USDJPY, EUR crosses),
//! and the premium currency decides whether deltas are premium-adjusted.
//!
//! Deltas follow Reiswich and Wystup, "FX volatility smile construction"
//! (2010): spot delta `dV/dS`, forward delta `dV/dF` (per unit of quote
//! discount factor), and the premium-adjusted versions that subtract the
//! premium itself when it is paid in base currency. Spot delta here is the
//! exact derivative of the trade-date PV with respect to the spot rate, so it
//! includes the spot-to-delivery carry ratio `F / S`; on flat curves this is
//! the textbook `exp(-r_b tau) N(d1)`.

use fq_core::math::{BlackError, OptionType, SolverConfig, black76, brent, norm_cdf, norm_inv, norm_pdf};
use fq_core::time::{Date, Period};

use crate::currency::{Currency, CurrencyPair};
use crate::forward::{FxError, FxMarket, FxNdf};

/// Where the premium is paid; decides premium adjustment of deltas.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PremiumCurrency {
    Base,
    Quote,
}

impl PremiumCurrency {
    /// Market default: the premium is paid in USD when the pair has it
    /// (EURUSD: quote; USDJPY: base) and in the base currency on crosses.
    pub fn market_default(pair: CurrencyPair) -> Self {
        if pair.quote() == Currency::USD { Self::Quote } else { Self::Base }
    }

    pub const fn is_premium_adjusted(self) -> bool {
        matches!(self, Self::Base)
    }
}

/// Which delta a quote refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DeltaConvention {
    Spot,
    Forward,
    SpotPremiumAdjusted,
    ForwardPremiumAdjusted,
}

impl DeltaConvention {
    /// Market default: spot delta up to and including one year, forward delta
    /// beyond; premium-adjusted when the premium is paid in base currency.
    pub fn market_default(premium: PremiumCurrency, tenor: Period) -> Self {
        let long = tenor.in_months().is_some_and(|m| m > 12);
        match (long, premium.is_premium_adjusted()) {
            (false, false) => Self::Spot,
            (false, true) => Self::SpotPremiumAdjusted,
            (true, false) => Self::Forward,
            (true, true) => Self::ForwardPremiumAdjusted,
        }
    }

    pub const fn is_premium_adjusted(self) -> bool {
        matches!(self, Self::SpotPremiumAdjusted | Self::ForwardPremiumAdjusted)
    }

    pub const fn is_spot(self) -> bool {
        matches!(self, Self::Spot | Self::SpotPremiumAdjusted)
    }
}

/// How the at-the-money strike is defined.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AtmConvention {
    /// `K = S`.
    Spot,
    /// `K = F`.
    Forward,
    /// Zero straddle delta: `K = F exp(sigma^2 tau / 2)`.
    DeltaNeutral,
    /// Zero premium-adjusted straddle delta: `K = F exp(-sigma^2 tau / 2)`.
    DeltaNeutralPremiumAdjusted,
}

impl AtmConvention {
    /// Market default: delta-neutral straddle, premium-adjusted when deltas are.
    pub fn market_default(premium: PremiumCurrency) -> Self {
        if premium.is_premium_adjusted() { Self::DeltaNeutralPremiumAdjusted } else { Self::DeltaNeutral }
    }
}

/// The forward-measure inputs a smile calculation needs for one expiry.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FxSlice {
    pub spot: f64,
    pub forward: f64,
    /// Quote-currency discount factor from the trade date to delivery.
    pub discount_quote: f64,
    /// ACT/365F time from the trade date to expiry.
    pub time_to_expiry: f64,
}

impl FxSlice {
    /// `dF/dS`: the carry ratio between spot and the delivery date.
    pub fn carry(&self) -> f64 {
        self.forward / self.spot
    }

    /// Multiplier turning a forward delta into a spot delta: `DF_q * F / S`
    /// (the base-currency discount factor on flat curves).
    pub fn spot_factor(&self) -> f64 {
        self.discount_quote * self.carry()
    }

    /// Delta of an option under a convention, for a flat vol.
    pub fn delta(&self, kind: OptionType, strike: f64, vol: f64, convention: DeltaConvention) -> Result<f64, FxError> {
        let (d1, d2) = self.d1_d2(strike, vol)?;
        let phi = kind.sign();
        let fwd = if convention.is_premium_adjusted() { phi * strike / self.forward * norm_cdf(phi * d2) } else { phi * norm_cdf(phi * d1) };
        Ok(if convention.is_spot() { fwd * self.spot_factor() } else { fwd })
    }

    /// Strike with the given (signed) delta under a convention and flat vol.
    /// Premium-adjusted call deltas are not monotone in strike; the strike on
    /// the high side of the delta maximum is returned, as the market does.
    pub fn strike_from_delta(&self, kind: OptionType, delta: f64, vol: f64, convention: DeltaConvention) -> Result<f64, FxError> {
        if !delta.is_finite() || !vol.is_finite() || vol <= 0.0 || self.time_to_expiry <= 0.0 {
            return Err(FxError::InvalidSmileInput(format!("delta {delta}, vol {vol}, time {}", self.time_to_expiry)));
        }
        let phi = kind.sign();
        let sd = vol * self.time_to_expiry.sqrt();
        let scale = if convention.is_spot() { self.spot_factor() } else { 1.0 };
        let target = phi * delta / scale; // N(phi d1) or K/F N(phi d2), in (0, 1)
        if target <= 0.0 || target >= 1.0 {
            return Err(FxError::DeltaOutOfRange { delta, kind, max: scale });
        }
        if !convention.is_premium_adjusted() {
            let d1 = phi * norm_inv(target);
            return Ok(self.forward * (-d1 * sd + 0.5 * sd * sd).exp());
        }
        let f = self.forward;
        let k_from_d2 = |d2: f64| f * (-d2 * sd - 0.5 * sd * sd).exp();
        // |delta| = K/F N(phi d2) for both calls and puts; solve g(K) = 0 on a bracket in K.
        let g = |k: f64| {
            let d2 = ((f / k).ln() - 0.5 * sd * sd) / sd;
            k / f * norm_cdf(phi * d2) - target
        };
        let cfg = SolverConfig { tolerance: 1e-14, max_iterations: 200 };
        let (lo, hi) = match kind {
            OptionType::Put => (k_from_d2(10.0), k_from_d2(-10.0)),
            OptionType::Call => {
                // d(K N(d2))/dK = 0 where N(d2) sd = pdf(d2); the delta maximum.
                let dmax = brent(|d2| norm_cdf(d2) * sd - norm_pdf(d2), -6.0, 6.0, cfg).map_err(FxError::Solver)?;
                (k_from_d2(dmax), k_from_d2(-10.0))
            }
        };
        if g(lo) * g(hi) > 0.0 {
            return Err(FxError::DeltaOutOfRange { delta, kind, max: scale * phi * (g(lo) + target).max(g(hi) + target) });
        }
        brent(g, lo, hi, cfg).map_err(FxError::Solver)
    }

    /// At-the-money strike for a convention and (ATM) vol.
    pub fn atm_strike(&self, convention: AtmConvention, vol: f64) -> f64 {
        let v = vol * vol * self.time_to_expiry;
        match convention {
            AtmConvention::Spot => self.spot,
            AtmConvention::Forward => self.forward,
            AtmConvention::DeltaNeutral => self.forward * (0.5 * v).exp(),
            AtmConvention::DeltaNeutralPremiumAdjusted => self.forward * (-0.5 * v).exp(),
        }
    }

    fn d1_d2(&self, strike: f64, vol: f64) -> Result<(f64, f64), FxError> {
        if strike <= 0.0 || !strike.is_finite() || vol <= 0.0 || !vol.is_finite() || self.time_to_expiry <= 0.0 {
            return Err(FxError::InvalidSmileInput(format!("strike {strike}, vol {vol}, time {}", self.time_to_expiry)));
        }
        let sd = vol * self.time_to_expiry.sqrt();
        let d1 = ((self.forward / strike).ln() + 0.5 * sd * sd) / sd;
        Ok((d1, d1 - sd))
    }
}

/// Price and Greeks of a vanilla, from the holder's side.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FxVanillaValuation {
    pub forward: f64,
    pub discount_quote: f64,
    pub time_to_expiry: f64,
    pub vol: f64,
    /// Premium in quote currency at the trade date.
    pub pv_quote: f64,
    /// Premium in base currency (`pv_quote / spot`).
    pub pv_base: f64,
    /// Premium as a fraction of base notional (`pv_base / N`), the "% base" quote.
    pub pct_base: f64,
    /// Premium in quote pips per unit base (`pv_quote / N / pip`).
    pub quote_pips: f64,
    /// `dPV_quote/dS` per unit base notional.
    pub spot_delta: f64,
    /// `dPV_quote/dF / DF_q` per unit base notional: `phi N(phi d1)`.
    pub forward_delta: f64,
    /// Spot delta less the base-currency premium: `spot_delta - pv_quote / (N S)`.
    pub spot_delta_pa: f64,
    /// `phi K/F N(phi d2)`.
    pub forward_delta_pa: f64,
    /// `d2 PV_quote / dS2`, in quote currency on the full notional.
    pub gamma: f64,
    /// `dPV_quote/dsigma` on the full notional (per unit vol; times 0.01 per vol point).
    pub vega: f64,
    /// `d2 PV_quote / dS dsigma` on the full notional.
    pub vanna: f64,
    /// `d2 PV_quote / dsigma2` on the full notional.
    pub volga: f64,
}

/// European vanilla on `base/quote`: call or put on one unit of base.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FxVanilla {
    pair: CurrencyPair,
    kind: OptionType,
    strike: f64,
    expiry: Date,
    delivery: Date,
    notional_base: f64,
    premium: PremiumCurrency,
}

impl FxVanilla {
    pub fn new(pair: CurrencyPair, kind: OptionType, strike: f64, expiry: Date, delivery: Date, notional_base: f64) -> Result<Self, FxError> {
        if notional_base <= 0.0 || !notional_base.is_finite() {
            return Err(FxError::NonPositiveNotional(notional_base));
        }
        if strike <= 0.0 || !strike.is_finite() {
            return Err(FxError::NonPositiveSpot(strike));
        }
        if delivery < expiry {
            return Err(FxError::SettlementBeforeFixing { fixing: expiry, settlement: delivery });
        }
        Ok(Self { pair, kind, strike, expiry, delivery, notional_base, premium: PremiumCurrency::market_default(pair) })
    }

    /// Market-tenor option: delivery on the forward date for `tenor`, expiry
    /// `spot_lag` business days earlier on the pair's calendar.
    pub fn from_tenor(pair: CurrencyPair, trade_date: Date, tenor: Period, kind: OptionType, strike: f64, notional_base: f64) -> Result<Self, FxError> {
        let (expiry, delivery) = expiry_and_delivery(pair, trade_date, tenor)?;
        Self::new(pair, kind, strike, expiry, delivery, notional_base)
    }

    pub fn with_premium_currency(mut self, premium: PremiumCurrency) -> Self {
        self.premium = premium;
        self
    }

    pub const fn pair(&self) -> CurrencyPair {
        self.pair
    }

    pub const fn kind(&self) -> OptionType {
        self.kind
    }

    pub const fn strike(&self) -> f64 {
        self.strike
    }

    pub const fn expiry(&self) -> Date {
        self.expiry
    }

    pub const fn delivery(&self) -> Date {
        self.delivery
    }

    pub const fn notional_base(&self) -> f64 {
        self.notional_base
    }

    pub const fn premium_currency(&self) -> PremiumCurrency {
        self.premium
    }

    /// Forward, discount factor and time to expiry for this option on a market.
    pub fn slice(&self, market: &FxMarket) -> Result<FxSlice, FxError> {
        if self.expiry <= market.trade_date() {
            return Err(FxError::Expired { expiry: self.expiry, trade_date: market.trade_date() });
        }
        Ok(FxSlice {
            spot: market.spot(),
            forward: market.forward(self.delivery)?,
            discount_quote: market.discount(self.pair.quote(), self.delivery)?,
            time_to_expiry: f64::from(self.expiry - market.trade_date()) / 365.0,
        })
    }

    /// Garman-Kohlhagen value and Greeks for a flat lognormal vol.
    pub fn value(&self, market: &FxMarket, vol: f64) -> Result<FxVanillaValuation, FxError> {
        let s = self.slice(market)?;
        let (f, k, tau, df_q) = (s.forward, self.strike, s.time_to_expiry, s.discount_quote);
        let b = black76(self.kind, f, k, vol, tau).map_err(FxError::Black)?;
        let n = self.notional_base;
        let carry = s.carry();
        let pv_quote = n * df_q * b.price;
        let (d1, d2) = if vol > 0.0 { s.d1_d2(k, vol)? } else { (f64::NAN, f64::NAN) };
        let sd = vol * tau.sqrt();
        let (vanna, volga) = if sd > 0.0 { (n * df_q * carry * (-norm_pdf(d1) * d2 / vol), n * df_q * b.vega * d1 * d2 / vol) } else { (0.0, 0.0) };
        let forward_delta_pa = if sd > 0.0 { self.kind.sign() * k / f * norm_cdf(self.kind.sign() * d2) } else { b.delta * k / f };
        Ok(FxVanillaValuation {
            forward: f,
            discount_quote: df_q,
            time_to_expiry: tau,
            vol,
            pv_quote,
            pv_base: pv_quote / s.spot,
            pct_base: pv_quote / (n * s.spot),
            quote_pips: pv_quote / n / self.pair.pip(),
            spot_delta: df_q * carry * b.delta,
            forward_delta: b.delta,
            spot_delta_pa: df_q * carry * b.delta - pv_quote / (n * s.spot),
            forward_delta_pa,
            gamma: n * df_q * carry * carry * b.gamma,
            vega: n * df_q * b.vega,
            vanna,
            volga,
        })
    }

    /// Delta under a given convention (per unit base notional).
    pub fn delta(&self, market: &FxMarket, vol: f64, convention: DeltaConvention) -> Result<f64, FxError> {
        let v = self.value(market, vol)?;
        Ok(match convention {
            DeltaConvention::Spot => v.spot_delta,
            DeltaConvention::Forward => v.forward_delta,
            DeltaConvention::SpotPremiumAdjusted => v.spot_delta_pa,
            DeltaConvention::ForwardPremiumAdjusted => v.forward_delta_pa,
        })
    }

    /// Flat vol reproducing a quote-currency premium at the trade date.
    pub fn implied_vol(&self, market: &FxMarket, pv_quote: f64) -> Result<f64, FxError> {
        let s = self.slice(market)?;
        let undiscounted = pv_quote / (self.notional_base * s.discount_quote);
        fq_core::math::shifted_black_implied_vol(self.kind, s.forward, self.strike, s.time_to_expiry, 0.0, undiscounted, 5.0).map_err(FxError::Black)
    }
}

/// `(expiry, delivery)` for a market tenor: delivery is the forward value
/// date, expiry is `spot_lag` business days before it on the pair's calendar.
pub fn expiry_and_delivery(pair: CurrencyPair, trade_date: Date, tenor: Period) -> Result<(Date, Date), FxError> {
    let delivery = pair.forward_date(trade_date, tenor)?;
    let expiry = FxNdf::fixing_for_settlement(pair, delivery)?;
    Ok((expiry, delivery))
}

impl From<BlackError> for FxError {
    fn from(e: BlackError) -> Self {
        Self::Black(e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_abs_diff_eq;

    use fq_core::math::OptionType::{Call, Put};
    use fq_rates::curve::DiscountCurve;

    use crate::forward::{FxDirection, FxForward};

    type R = Result<(), FxError>;

    fn d(y: i32, m: u32, dd: u32) -> Date {
        Date::from_ymd(y, m, dd).expect("valid test date")
    }

    /// EURUSD 13 Oct 2026, spot 1.1750, EUR 2% and USD 4% flat continuous.
    fn eurusd() -> Result<FxMarket, FxError> {
        let today = d(2026, 10, 13);
        let eur = DiscountCurve::flat(today, 0.02, d(2040, 1, 1))?;
        let usd = DiscountCurve::flat(today, 0.04, d(2040, 1, 1))?;
        FxMarket::new("EURUSD".parse()?, today, 1.1750, eur, usd)
    }

    /// Golden from QuantLib 1.44 `AnalyticEuropeanEngine` on a
    /// `GarmanKohlagenProcess`: expiry = delivery = 2027-10-13 (365 days),
    /// EUR 2% / USD 4% flat ACT/365F, QuantLib spot set to
    /// `1.1750 * exp(-(0.04 - 0.02) * 2/365)` so that its trade-date forward
    /// equals this market's spot-date forward.
    #[test]
    fn garman_kohlhagen_matches_quantlib() -> R {
        let m = eurusd()?;
        let expiry = d(2027, 10, 13);
        let cases = [
            // (kind, strike, vol, QuantLib NPV per unit base)
            (Call, 1.1750, 0.08, 0.048_840_311_670_47),
            (Put, 1.1750, 0.08, 0.026_160_671_986_65),
            (Call, 1.2500, 0.10, 0.026_288_635_780_63),
            (Put, 1.1000, 0.12, 0.018_407_925_882_44),
        ];
        for (kind, k, vol, npv) in cases {
            let opt = FxVanilla::new(m.pair(), kind, k, expiry, expiry, 1.0)?;
            let v = opt.value(&m, vol)?;
            assert_abs_diff_eq!(v.pv_quote, npv, epsilon = 2e-10);
        }
        Ok(())
    }

    #[test]
    fn put_call_parity_against_the_forward() -> R {
        let m = eurusd()?;
        let opt = FxVanilla::from_tenor(m.pair(), m.trade_date(), Period::months(6), Call, 1.19, 10e6)?;
        let put = FxVanilla::new(m.pair(), Put, 1.19, opt.expiry(), opt.delivery(), 10e6)?;
        let fwd = FxForward::new(m.pair(), FxDirection::BuyBase, 10e6, 1.19, opt.delivery())?;
        let (c, p) = (opt.value(&m, 0.09)?, put.value(&m, 0.09)?);
        assert_abs_diff_eq!(c.pv_quote - p.pv_quote, fwd.npv_quote(&m)?, epsilon = 1e-6);
        assert_abs_diff_eq!(c.forward_delta - p.forward_delta, 1.0, epsilon = 1e-14);
        assert_abs_diff_eq!(c.vega, p.vega, epsilon = 1e-6);
        assert_abs_diff_eq!(c.gamma, p.gamma, epsilon = 1e-6);
        // Expiry/delivery from the tenor: 6M delivery 15 Apr 2027, expiry two TARGET+USNY business days earlier.
        assert_eq!(opt.delivery(), d(2027, 4, 15));
        assert_eq!(opt.expiry(), d(2027, 4, 13));
        Ok(())
    }

    #[test]
    fn greeks_match_finite_differences() -> R {
        let m = eurusd()?;
        let opt = FxVanilla::from_tenor(m.pair(), m.trade_date(), Period::years(1), Call, 1.20, 1e6)?;
        let vol = 0.085;
        let v = opt.value(&m, vol)?;
        let h = 1e-4;
        let up = opt.value(&m.with_spot(1.175 + h)?, vol)?;
        let dn = opt.value(&m.with_spot(1.175 - h)?, vol)?;
        assert_abs_diff_eq!((up.pv_quote - dn.pv_quote) / (2.0 * h), v.spot_delta * 1e6, epsilon = 5e-2);
        assert_abs_diff_eq!((up.pv_quote + dn.pv_quote - 2.0 * v.pv_quote) / (h * h), v.gamma, epsilon = 1e-6 * v.gamma);
        let hv = 1e-5;
        let vu = opt.value(&m, vol + hv)?;
        let vd = opt.value(&m, vol - hv)?;
        assert_abs_diff_eq!((vu.pv_quote - vd.pv_quote) / (2.0 * hv), v.vega, epsilon = 1e-2);
        assert_abs_diff_eq!((vu.pv_quote + vd.pv_quote - 2.0 * v.pv_quote) / (hv * hv), v.volga, epsilon = 5.0);
        assert_abs_diff_eq!((up.vega - dn.vega) / (2.0 * h), v.vanna, epsilon = 1e-1);
        // Spot delta on flat curves is exp(-r_b tau_delivery) N(d1) up to the spot-window adjustment.
        let s = opt.slice(&m)?;
        assert_abs_diff_eq!(v.spot_delta, s.spot_factor() * v.forward_delta, epsilon = 1e-15);
        assert_abs_diff_eq!(v.spot_delta_pa, v.spot_delta - v.pct_base, epsilon = 1e-15);
        assert_abs_diff_eq!(v.pct_base, v.pv_base / 1e6, epsilon = 1e-15);
        Ok(())
    }

    #[test]
    fn implied_vol_round_trip() -> R {
        let m = eurusd()?;
        let opt = FxVanilla::from_tenor(m.pair(), m.trade_date(), Period::months(3), Put, 1.15, 5e6)?;
        let pv = opt.value(&m, 0.0725)?.pv_quote;
        assert_abs_diff_eq!(opt.implied_vol(&m, pv)?, 0.0725, epsilon = 1e-10);
        assert!(matches!(FxVanilla::new(m.pair(), Call, 1.2, d(2026, 10, 13), d(2026, 10, 15), 1.0)?.value(&m, 0.1), Err(FxError::Expired { .. })));
        Ok(())
    }

    #[test]
    fn strike_from_delta_round_trips_in_every_convention() -> R {
        let m = eurusd()?;
        let opt = FxVanilla::from_tenor(m.pair(), m.trade_date(), Period::years(1), Call, 1.20, 1.0)?;
        let s = opt.slice(&m)?;
        let vol = 0.09;
        for conv in [DeltaConvention::Spot, DeltaConvention::Forward, DeltaConvention::SpotPremiumAdjusted, DeltaConvention::ForwardPremiumAdjusted] {
            for (kind, delta) in [(Call, 0.25), (Call, 0.10), (Put, -0.25), (Put, -0.10)] {
                let k = s.strike_from_delta(kind, delta, vol, conv)?;
                let back = s.delta(kind, k, vol, conv)?;
                assert_abs_diff_eq!(back, delta, epsilon = 1e-10);
                // The slice delta agrees with the option's own Greeks.
                let o = FxVanilla::new(m.pair(), kind, k, opt.expiry(), opt.delivery(), 1.0)?;
                assert_abs_diff_eq!(o.delta(&m, vol, conv)?, delta, epsilon = 1e-10);
            }
        }
        // Premium adjustment lowers a call's delta at every strike, so the 25d pa call is struck below the plain one.
        let plain = s.strike_from_delta(Call, 0.25, vol, DeltaConvention::Forward)?;
        let pa = s.strike_from_delta(Call, 0.25, vol, DeltaConvention::ForwardPremiumAdjusted)?;
        assert!(pa < plain, "{pa} vs {plain}");
        assert!(s.delta(Call, plain, vol, DeltaConvention::ForwardPremiumAdjusted)? < 0.25);
        // Unreachable deltas are errors, not NaNs.
        assert!(matches!(s.strike_from_delta(Call, 0.9, vol, DeltaConvention::ForwardPremiumAdjusted), Err(FxError::DeltaOutOfRange { .. })));
        assert!(matches!(s.strike_from_delta(Call, 1.5, vol, DeltaConvention::Forward), Err(FxError::DeltaOutOfRange { .. })));
        Ok(())
    }

    #[test]
    fn atm_conventions() -> R {
        let m = eurusd()?;
        let opt = FxVanilla::from_tenor(m.pair(), m.trade_date(), Period::years(1), Call, 1.20, 1.0)?;
        let s = opt.slice(&m)?;
        let vol = 0.09;
        assert_abs_diff_eq!(s.atm_strike(AtmConvention::Spot, vol), 1.175, epsilon = 1e-15);
        assert_abs_diff_eq!(s.atm_strike(AtmConvention::Forward, vol), s.forward, epsilon = 1e-15);
        // Delta-neutral straddle: call and put forward deltas cancel.
        let k = s.atm_strike(AtmConvention::DeltaNeutral, vol);
        assert_abs_diff_eq!(s.delta(Call, k, vol, DeltaConvention::Forward)? + s.delta(Put, k, vol, DeltaConvention::Forward)?, 0.0, epsilon = 1e-14);
        let kpa = s.atm_strike(AtmConvention::DeltaNeutralPremiumAdjusted, vol);
        assert_abs_diff_eq!(
            s.delta(Call, kpa, vol, DeltaConvention::ForwardPremiumAdjusted)? + s.delta(Put, kpa, vol, DeltaConvention::ForwardPremiumAdjusted)?,
            0.0,
            epsilon = 1e-14
        );
        assert!(kpa < s.forward && k > s.forward);
        Ok(())
    }

    #[test]
    fn market_defaults() -> R {
        let eurusd: CurrencyPair = "EURUSD".parse()?;
        let usdjpy: CurrencyPair = "USDJPY".parse()?;
        let eurgbp: CurrencyPair = "EURGBP".parse()?;
        assert_eq!(PremiumCurrency::market_default(eurusd), PremiumCurrency::Quote);
        assert_eq!(PremiumCurrency::market_default(usdjpy), PremiumCurrency::Base);
        assert_eq!(PremiumCurrency::market_default(eurgbp), PremiumCurrency::Base);
        assert_eq!(DeltaConvention::market_default(PremiumCurrency::Quote, Period::months(6)), DeltaConvention::Spot);
        assert_eq!(DeltaConvention::market_default(PremiumCurrency::Quote, Period::years(1)), DeltaConvention::Spot);
        assert_eq!(DeltaConvention::market_default(PremiumCurrency::Quote, Period::years(2)), DeltaConvention::Forward);
        assert_eq!(DeltaConvention::market_default(PremiumCurrency::Base, Period::months(3)), DeltaConvention::SpotPremiumAdjusted);
        assert_eq!(DeltaConvention::market_default(PremiumCurrency::Base, Period::months(18)), DeltaConvention::ForwardPremiumAdjusted);
        assert_eq!(AtmConvention::market_default(PremiumCurrency::Base), AtmConvention::DeltaNeutralPremiumAdjusted);
        Ok(())
    }
}
