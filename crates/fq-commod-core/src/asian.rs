//! Average-price (Asian) options by lognormal moment matching.
//!
//! Most physical commodity pricing is against an *average*: a monthly oil
//! swap settles on the mean of daily front-month settles, a gas index on
//! the month's bid-week average, a power block on hourly LMPs. Options on
//! those averages are priced here with the Turnbull-Wakeman / Levy
//! approach: compute the first two moments of the arithmetic average of
//! lognormal fixings exactly, match a lognormal to them, and price with
//! Black-76. Fixings already observed are handled by shrinking the strike,
//! so the same object prices a fresh option and one halfway through its
//! averaging period.
//!
//! Fixings can reference different contracts (a strip option averaging
//! across months). Pairwise log-covariance is `rho * s_i * s_j * min(t_i,
//! t_j)` with `rho = 1` for fixings on the same contract; cross-contract
//! correlation is a single parameter here, which is the usual desk
//! simplification. A full factor structure comes from [`crate::model`].

use fq_core::math::{OptionType, black76};
use fq_core::time::{Date, DayCount};
use fq_rates::curve::DiscountCurve;

use crate::contract::ContractMonth;
use crate::curve::FuturesCurve;
use crate::error::CommodError;
use crate::vol::SeasonalVol;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fixing {
    pub date: Date,
    pub contract: ContractMonth,
    /// Set once the fixing has been published.
    pub observed: Option<f64>,
}

impl Fixing {
    pub fn pending(date: Date, contract: ContractMonth) -> Self {
        Self { date, contract, observed: None }
    }

    pub fn observed(date: Date, contract: ContractMonth, price: f64) -> Self {
        Self { date, contract, observed: Some(price) }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AsianOption {
    pub kind: OptionType,
    pub strike: f64,
    pub fixings: Vec<Fixing>,
    pub settlement: Date,
    /// Total quantity over the averaging period, in the hub's unit.
    pub quantity: f64,
    /// Correlation applied between fixings on *different* contracts.
    pub cross_contract_corr: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AsianValuation {
    /// Expected average of the not-yet-fixed observations (their forward).
    pub forward_average: f64,
    /// Average of the fixings already observed, weighted by their share.
    pub fixed_contribution: f64,
    /// Strike net of the fixed contribution, as seen by the remaining fixings.
    pub effective_strike: f64,
    /// Black vol of the matched lognormal, per unit `time_to_last_fixing`.
    pub effective_vol: f64,
    pub time_to_last_fixing: f64,
    pub discount: f64,
    /// Undiscounted price per unit of quantity.
    pub forward_price: f64,
    pub pv: f64,
}

impl AsianOption {
    pub fn new(kind: OptionType, strike: f64, fixings: Vec<Fixing>, settlement: Date, quantity: f64) -> Result<Self, CommodError> {
        if fixings.is_empty() {
            return Err(CommodError::NoFixings);
        }
        if fixings.windows(2).any(|w| w[0].date >= w[1].date) {
            return Err(CommodError::UnsortedFixings);
        }
        if !(strike.is_finite() && strike > 0.0) {
            return Err(CommodError::Invalid(format!("strike {strike} must be positive")));
        }
        if !(quantity.is_finite() && quantity > 0.0) {
            return Err(CommodError::Invalid(format!("quantity {quantity} must be positive")));
        }
        let last = fixings[fixings.len() - 1].date;
        if settlement < last {
            return Err(CommodError::Invalid(format!("settlement {settlement} is before the last fixing {last}")));
        }
        Ok(Self { kind, strike, fixings, settlement, quantity, cross_contract_corr: 1.0 })
    }

    pub fn with_cross_contract_corr(mut self, rho: f64) -> Result<Self, CommodError> {
        if !(rho.is_finite() && (-1.0..=1.0).contains(&rho)) {
            return Err(CommodError::BadCorrelation(rho));
        }
        self.cross_contract_corr = rho;
        Ok(self)
    }

    /// Daily fixings on one contract over an inclusive date range using
    /// every calendar day; sector crates substitute business-day schedules.
    pub fn daily_fixings(start: Date, end: Date, contract: ContractMonth) -> Vec<Fixing> {
        (start.serial()..=end.serial()).filter_map(|s| Date::from_serial(s).ok()).map(|d| Fixing::pending(d, contract)).collect()
    }

    /// Price with vol per contract from `vols`.
    pub fn value(&self, futures: &FuturesCurve, vols: &SeasonalVol, discount: &DiscountCurve, dc: DayCount) -> Result<AsianValuation, CommodError> {
        self.value_with(futures, |m| vols.vol(m), discount, dc)
    }

    /// Price with an arbitrary per-contract vol function.
    pub fn value_with(
        &self,
        futures: &FuturesCurve,
        vol_of: impl Fn(ContractMonth) -> f64,
        discount: &DiscountCurve,
        dc: DayCount,
    ) -> Result<AsianValuation, CommodError> {
        let valuation = futures.reference_date();
        let n = self.fixings.len() as f64;
        let df = discount.discount(self.settlement)?;

        // Split fixed vs pending. A pending fixing dated on or before the
        // valuation date is treated as fixing at today's forward with no
        // remaining variance.
        let mut fixed_sum = 0.0;
        let mut pending: Vec<(f64, f64, f64, ContractMonth)> = Vec::new(); // (t, F, sigma, contract)
        for fx in &self.fixings {
            match fx.observed {
                Some(p) => fixed_sum += p,
                None => {
                    let f = futures.price(fx.contract)?;
                    let t = if fx.date <= valuation { 0.0 } else { dc.year_fraction(valuation, fx.date, None)? };
                    let s = vol_of(fx.contract);
                    if !(s.is_finite() && s >= 0.0) {
                        return Err(CommodError::BadVol(s));
                    }
                    pending.push((t, f, s, fx.contract));
                }
            }
        }
        let fixed_contribution = fixed_sum / n;
        let effective_strike = self.strike - fixed_contribution;
        let t_last = pending.iter().map(|p| p.0).fold(0.0, f64::max);

        if pending.is_empty() {
            // Fully fixed: deterministic payoff.
            let payoff = (self.kind.sign() * -effective_strike).max(0.0);
            return Ok(AsianValuation {
                forward_average: 0.0,
                fixed_contribution,
                effective_strike,
                effective_vol: 0.0,
                time_to_last_fixing: 0.0,
                discount: df,
                forward_price: payoff,
                pv: payoff * df * self.quantity,
            });
        }

        // Moments of the pending part of the average, weighted 1/n each.
        let m1: f64 = pending.iter().map(|p| p.1).sum::<f64>() / n;
        let mut m2 = 0.0;
        for (i, a) in pending.iter().enumerate() {
            for b in pending.iter().skip(i) {
                let rho = if a.3 == b.3 { 1.0 } else { self.cross_contract_corr };
                let cov = rho * a.2 * b.2 * a.0.min(b.0);
                let term = a.1 * b.1 * cov.exp();
                m2 += if std::ptr::eq(a, b) { term } else { 2.0 * term };
            }
        }
        m2 /= n * n;

        if effective_strike <= 0.0 {
            // Already guaranteed in the money for a call / worthless for a put.
            let forward_price = match self.kind {
                OptionType::Call => m1 - effective_strike,
                OptionType::Put => 0.0,
            };
            return Ok(AsianValuation {
                forward_average: m1,
                fixed_contribution,
                effective_strike,
                effective_vol: 0.0,
                time_to_last_fixing: t_last,
                discount: df,
                forward_price,
                pv: forward_price * df * self.quantity,
            });
        }

        let log_var = (m2 / (m1 * m1)).ln().max(0.0);
        let (effective_vol, t) = if t_last > 0.0 { ((log_var / t_last).sqrt(), t_last) } else { (0.0, 0.0) };
        let r = black76(self.kind, m1, effective_strike, effective_vol, t)?;
        Ok(AsianValuation {
            forward_average: m1,
            fixed_contribution,
            effective_strike,
            effective_vol,
            time_to_last_fixing: t,
            discount: df,
            forward_price: r.price,
            pv: r.price * df * self.quantity,
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

    fn setup(val: Date) -> (FuturesCurve, DiscountCurve) {
        let hub = Hub::new("BRN", Commodity::Crude, Unit::Barrel);
        let q = vec![FuturesQuote::new(cm(2027, 1), 80.0), FuturesQuote::new(cm(2027, 2), 80.0), FuturesQuote::new(cm(2027, 3), 80.0)];
        let fut = FuturesCurve::new(hub, val, q, CurveInterpolation::Linear).expect("curve");
        let nodes: Vec<(Date, f64)> = (1..=24).map(|k| (val.add_months(k).expect("d"), 1.0)).collect();
        let disc = DiscountCurve::new(val, &nodes, Interpolation::LogLinearDiscount).expect("disc");
        (fut, disc)
    }

    #[test]
    fn single_fixing_is_black76() {
        let val = ymd(2026, 11, 9);
        let (fut, disc) = setup(val);
        let exp = ymd(2026, 12, 15);
        let opt = AsianOption::new(OptionType::Call, 82.0, vec![Fixing::pending(exp, cm(2027, 1))], exp, 1.0).expect("opt");
        let vols = SeasonalVol::flat(cm(2027, 1), 0.35).expect("vols");
        let v = opt.value(&fut, &vols, &disc, DayCount::Act365Fixed).expect("v");
        let t = DayCount::Act365Fixed.year_fraction(val, exp, None).expect("t");
        let b = black76(OptionType::Call, 80.0, 82.0, 0.35, t).expect("b");
        assert_abs_diff_eq!(v.forward_price, b.price, epsilon = 1e-10);
        assert_abs_diff_eq!(v.effective_vol, 0.35, epsilon = 1e-10);
    }

    #[test]
    fn averaging_reduces_vol_and_matches_monte_carlo() {
        let val = ymd(2026, 11, 9);
        let (fut, disc) = setup(val);
        let fixings = AsianOption::daily_fixings(ymd(2027, 1, 1), ymd(2027, 1, 31), cm(2027, 2));
        let opt = AsianOption::new(OptionType::Call, 80.0, fixings.clone(), ymd(2027, 2, 5), 1.0).expect("opt");
        let vols = SeasonalVol::flat(cm(2027, 2), 0.40).expect("vols");
        let v = opt.value(&fut, &vols, &disc, DayCount::Act365Fixed).expect("v");
        assert!(v.effective_vol < 0.40 && v.effective_vol > 0.30, "effective vol {}", v.effective_vol);

        // Brute-force check: simulate the driftless lognormal path through
        // the fixing dates and average the payoff.
        let dc = DayCount::Act365Fixed;
        let ts: Vec<f64> = fixings.iter().map(|f| dc.year_fraction(val, f.date, None).expect("t")).collect();
        let mut rng = Xoshiro256StarStar::seed_from_u64(42);
        let (sigma, f0, n_paths) = (0.40f64, 80.0f64, 400_000usize);
        let mut sum = 0.0;
        for _ in 0..n_paths / 2 {
            let mut x_plus = 0.0;
            let mut x_minus = 0.0;
            let mut avg_plus = 0.0;
            let mut avg_minus = 0.0;
            let mut prev_t = 0.0;
            for &t in &ts {
                let dt = t - prev_t;
                let z = rng.next_gaussian();
                let drift = -0.5 * sigma * sigma * dt;
                x_plus += drift + sigma * dt.sqrt() * z;
                x_minus += drift - sigma * dt.sqrt() * z;
                avg_plus += f0 * x_plus.exp();
                avg_minus += f0 * x_minus.exp();
                prev_t = t;
            }
            let n = ts.len() as f64;
            sum += (avg_plus / n - 80.0).max(0.0) + (avg_minus / n - 80.0).max(0.0);
        }
        let mc = sum / n_paths as f64;
        // Moment matching is accurate to a few bp of forward for ATM.
        assert_abs_diff_eq!(v.forward_price, mc, epsilon = 0.05);
    }

    #[test]
    fn partially_fixed_option_shrinks_strike() {
        let val = ymd(2027, 1, 16);
        let (fut, disc) = setup(val);
        let mut fixings = AsianOption::daily_fixings(ymd(2027, 1, 1), ymd(2027, 1, 31), cm(2027, 2));
        for f in fixings.iter_mut().filter(|f| f.date <= ymd(2027, 1, 15)) {
            f.observed = Some(90.0);
        }
        let opt = AsianOption::new(OptionType::Call, 80.0, fixings, ymd(2027, 2, 5), 1000.0).expect("opt");
        let vols = SeasonalVol::flat(cm(2027, 2), 0.40).expect("vols");
        let v = opt.value(&fut, &vols, &disc, DayCount::Act365Fixed).expect("v");
        assert_abs_diff_eq!(v.fixed_contribution, 15.0 * 90.0 / 31.0, epsilon = 1e-12);
        assert_abs_diff_eq!(v.effective_strike, 80.0 - 15.0 * 90.0 / 31.0, epsilon = 1e-12);
        // Remaining 16 fixings at 80 contribute 16*80/31 to the average.
        assert_abs_diff_eq!(v.forward_average, 16.0 * 80.0 / 31.0, epsilon = 1e-12);
        // Intrinsic lower bound: average forward of (15*90 + 16*80)/31 vs K=80.
        let intrinsic = (15.0 * 90.0 + 16.0 * 80.0) / 31.0 - 80.0;
        assert!(v.forward_price >= intrinsic);
        assert_abs_diff_eq!(v.pv, v.forward_price * 1000.0, epsilon = 1e-9);
    }

    #[test]
    fn fully_fixed_is_deterministic() {
        let val = ymd(2027, 2, 2);
        let (fut, disc) = setup(val);
        let fixings = vec![Fixing::observed(ymd(2027, 1, 10), cm(2027, 2), 85.0), Fixing::observed(ymd(2027, 1, 20), cm(2027, 2), 87.0)];
        let call = AsianOption::new(OptionType::Call, 80.0, fixings.clone(), ymd(2027, 2, 5), 1.0).expect("c");
        let put = AsianOption::new(OptionType::Put, 80.0, fixings, ymd(2027, 2, 5), 1.0).expect("p");
        let vols = SeasonalVol::flat(cm(2027, 2), 0.40).expect("vols");
        assert_abs_diff_eq!(call.value(&fut, &vols, &disc, DayCount::Act365Fixed).expect("v").forward_price, 6.0, epsilon = 1e-12);
        assert_abs_diff_eq!(put.value(&fut, &vols, &disc, DayCount::Act365Fixed).expect("v").forward_price, 0.0, epsilon = 1e-12);
    }

    #[test]
    fn validation() {
        let f = vec![Fixing::pending(ymd(2027, 1, 2), cm(2027, 2)), Fixing::pending(ymd(2027, 1, 1), cm(2027, 2))];
        assert_eq!(AsianOption::new(OptionType::Call, 80.0, f, ymd(2027, 2, 1), 1.0), Err(CommodError::UnsortedFixings));
        assert_eq!(AsianOption::new(OptionType::Call, 80.0, vec![], ymd(2027, 2, 1), 1.0), Err(CommodError::NoFixings));
        let f = vec![Fixing::pending(ymd(2027, 1, 2), cm(2027, 2))];
        let o = AsianOption::new(OptionType::Call, 80.0, f, ymd(2027, 2, 1), 1.0).expect("o");
        assert!(matches!(o.with_cross_contract_corr(1.5), Err(CommodError::BadCorrelation(_))));
    }
}
