//! Swing (take-or-pay) contracts with monthly nomination.
//!
//! A swing contract lets the buyer take between a minimum and a maximum
//! quantity each period at a fixed price, subject to a minimum and maximum
//! total over the contract. It is the standard form of a physical supply
//! deal, and the optionality is in choosing *when* to take.
//!
//! With one decision per month and a monthly index, the intrinsic problem
//! is a bounded continuous knapsack: take the minimum everywhere, then
//! allocate the remaining flexible volume to the months with the best
//! margin until the total cap binds, and if the total floor is not yet met
//! fill the least-bad months up to it. That greedy is exact for this LP.
//!
//! Three numbers come out: the intrinsic value against today's curve, an
//! upper bound (every month's flexible volume priced as an independent
//! Black-76 call, ignoring the total constraints), and a rolling-intrinsic
//! Monte Carlo value that re-nominates on simulated curves, which sits
//! between the two.

use fq_commod_core::{ClewlowStrickland, ContractMonth, FuturesCurve, McConfig, SeasonalVol};
use fq_core::math::{OptionType, black76};
use fq_core::time::DayCount;
use fq_rates::curve::DiscountCurve;

use crate::error::GasError;
use crate::hub::{GasHub, NymexCalendar, nymex_expiry};

/// `(months, days per month, discount factor per month, year fraction to each NYMEX expiry)`.
type MonthData = (Vec<ContractMonth>, Vec<i32>, Vec<f64>, Vec<f64>);

#[derive(Debug, Clone, PartialEq)]
pub struct SwingOption {
    pub hub: GasHub,
    pub start: ContractMonth,
    pub end: ContractMonth,
    /// Fixed purchase price, $/MMBtu.
    pub strike: f64,
    /// Minimum and maximum take per day, MMBtu/d.
    pub min_daily: f64,
    pub max_daily: f64,
    /// Minimum and maximum total take over the contract, MMBtu.
    pub min_total: f64,
    pub max_total: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SwingValuation {
    pub months: Vec<ContractMonth>,
    /// Intrinsic take per month under the optimal nomination.
    pub intrinsic_takes: Vec<f64>,
    pub intrinsic_pv: f64,
    /// Sum of Black-76 calls on the flexible volume plus the forward value
    /// of the minimum take, ignoring total constraints.
    pub upper_bound_pv: f64,
    pub rolling_pv: Option<f64>,
    pub rolling_std_err: Option<f64>,
}

impl SwingOption {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        hub: GasHub,
        start: ContractMonth,
        end: ContractMonth,
        strike: f64,
        min_daily: f64,
        max_daily: f64,
        min_total: f64,
        max_total: f64,
    ) -> Result<Self, GasError> {
        let months = ContractMonth::range(start, end);
        if months.is_empty() {
            return Err(GasError::EmptyPeriod { start, end });
        }
        if !(strike.is_finite() && strike > 0.0) {
            return Err(GasError::Invalid(format!("strike {strike} must be positive")));
        }
        if !(min_daily.is_finite() && max_daily.is_finite() && 0.0 <= min_daily && min_daily <= max_daily && max_daily > 0.0) {
            return Err(GasError::Invalid(format!("need 0 <= min_daily ({min_daily}) <= max_daily ({max_daily}), max positive")));
        }
        if !(min_total.is_finite() && max_total.is_finite() && 0.0 <= min_total && min_total <= max_total) {
            return Err(GasError::Invalid(format!("need 0 <= min_total ({min_total}) <= max_total ({max_total})")));
        }
        let mut lo = 0.0;
        let mut hi = 0.0;
        for m in &months {
            let d = f64::from(m.delivery()?.days());
            lo += min_daily * d;
            hi += max_daily * d;
        }
        if min_total > hi || max_total < lo {
            return Err(GasError::Invalid(format!("total constraints [{min_total}, {max_total}] are infeasible against daily limits [{lo}, {hi}]")));
        }
        Ok(Self { hub, start, end, strike, min_daily, max_daily, min_total, max_total })
    }

    pub fn months(&self) -> Vec<ContractMonth> {
        ContractMonth::range(self.start, self.end)
    }

    /// Greedy optimal nomination for `prices` over months with `days`,
    /// given totals already taken in earlier months. Returns takes and the
    /// discounted value.
    fn nominate(&self, prices: &[f64], days: &[i32], dfs: &[f64], taken_so_far: f64) -> (Vec<f64>, f64) {
        let n = prices.len();
        let mins: Vec<f64> = days.iter().map(|&d| self.min_daily * f64::from(d)).collect();
        let maxs: Vec<f64> = days.iter().map(|&d| self.max_daily * f64::from(d)).collect();
        let mut takes = mins.clone();
        let mut total = taken_so_far + mins.iter().sum::<f64>();
        // Discounted margin per unit for each month.
        let margin: Vec<f64> = (0..n).map(|i| (prices[i] - self.strike) * dfs[i]).collect();
        let mut order: Vec<usize> = (0..n).collect();
        order.sort_by(|&a, &b| margin[b].partial_cmp(&margin[a]).unwrap_or(std::cmp::Ordering::Equal));
        // Add profitable flexible volume up to the cap.
        for &i in &order {
            if margin[i] <= 0.0 {
                break;
            }
            let room = (self.max_total - total).max(0.0);
            let add = (maxs[i] - takes[i]).min(room);
            takes[i] += add;
            total += add;
        }
        // Meet the floor with the least-bad months.
        if total < self.min_total {
            for &i in &order {
                let need = self.min_total - total;
                if need <= 1e-9 {
                    break;
                }
                let add = (maxs[i] - takes[i]).min(need);
                takes[i] += add;
                total += add;
            }
        }
        let value = (0..n).map(|i| takes[i] * margin[i]).sum();
        (takes, value)
    }

    fn month_data(&self, discount: &DiscountCurve, cal: &NymexCalendar, dc: DayCount) -> Result<MonthData, GasError> {
        let valuation = discount.reference_date();
        let months = self.months();
        let (mut days, mut dfs, mut ts) = (Vec::new(), Vec::new(), Vec::new());
        for &m in &months {
            let e = nymex_expiry(m, cal)?;
            if e <= valuation {
                return Err(GasError::MonthExpired { month: m, valuation });
            }
            days.push(m.delivery()?.days());
            dfs.push(discount.discount(m.last_day()?)?);
            ts.push(dc.year_fraction(valuation, e, None)?);
        }
        Ok((months, days, dfs, ts))
    }

    /// Intrinsic nomination and the Black-76 upper bound.
    pub fn value(
        &self,
        curve: &FuturesCurve,
        vols: &SeasonalVol,
        discount: &DiscountCurve,
        cal: &NymexCalendar,
        dc: DayCount,
    ) -> Result<SwingValuation, GasError> {
        if curve.hub().code() != self.hub.code() {
            return Err(GasError::WrongHub { expected: self.hub.code().into(), got: curve.hub().to_string() });
        }
        let (months, days, dfs, ts) = self.month_data(discount, cal, dc)?;
        let prices: Vec<f64> = months.iter().map(|&m| curve.price(m)).collect::<Result<_, _>>()?;
        let (takes, intrinsic_pv) = self.nominate(&prices, &days, &dfs, 0.0);
        let mut upper = 0.0;
        for i in 0..months.len() {
            let d = f64::from(days[i]);
            let min_q = self.min_daily * d;
            let flex_q = (self.max_daily - self.min_daily) * d;
            let call = black76(OptionType::Call, prices[i], self.strike, vols.vol(months[i]), ts[i])?.price;
            upper += (min_q * (prices[i] - self.strike) + flex_q * call) * dfs[i];
        }
        Ok(SwingValuation { months, intrinsic_takes: takes, intrinsic_pv, upper_bound_pv: upper, rolling_pv: None, rolling_std_err: None })
    }

    /// Adds a rolling-intrinsic Monte Carlo value: on each simulated path
    /// the buyer re-nominates each month off the then-current curve with
    /// the totals already taken, and executes that month's take at its
    /// realised index.
    #[allow(clippy::too_many_arguments)]
    pub fn value_with_model(
        &self,
        curve: &FuturesCurve,
        vols: &SeasonalVol,
        model: &ClewlowStrickland,
        discount: &DiscountCurve,
        cal: &NymexCalendar,
        dc: DayCount,
        cfg: McConfig,
    ) -> Result<SwingValuation, GasError> {
        let mut out = self.value(curve, vols, discount, cal, dc)?;
        let (months, days, dfs, ts) = self.month_data(discount, cal, dc)?;
        let m = months.len();
        let initial: Vec<(f64, f64)> = months.iter().zip(ts.iter()).map(|(&cm, &t)| curve.price(cm).map(|f| (t, f))).collect::<Result<_, _>>()?;
        let paths = model.simulate(&initial, &ts, cfg)?;
        let mut prices = vec![0.0; m];
        let (mean, se) = paths.estimate(|p| {
            let mut taken = 0.0;
            let mut pnl = 0.0;
            for k in 0..m {
                for (j, price) in prices.iter_mut().enumerate().skip(k) {
                    *price = paths.forward(p, k, j);
                }
                let (takes, _) = self.nominate(&prices[k..], &days[k..], &dfs[k..], taken);
                pnl += takes[0] * (prices[k] - self.strike) * dfs[k];
                taken += takes[0];
            }
            pnl
        });
        out.rolling_pv = Some(mean);
        out.rolling_std_err = Some(se);
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_abs_diff_eq;
    use fq_commod_core::{CurveInterpolation, FuturesQuote};
    use fq_core::time::Date;
    use fq_rates::curve::Interpolation;

    fn ymd(y: i32, m: u32, d: u32) -> Date {
        Date::from_ymd(y, m, d).expect("valid date")
    }

    fn cm(y: i32, m: u32) -> ContractMonth {
        ContractMonth::new(y, m).expect("month")
    }

    fn market() -> (FuturesCurve, SeasonalVol, DiscountCurve, NymexCalendar) {
        let val = ymd(2026, 11, 9);
        let c = FuturesCurve::new(
            GasHub::HenryHub.hub(),
            val,
            vec![
                FuturesQuote::new(cm(2027, 1), 4.40),
                FuturesQuote::new(cm(2027, 2), 4.20),
                FuturesQuote::new(cm(2027, 3), 3.60),
                FuturesQuote::new(cm(2027, 4), 3.00),
            ],
            CurveInterpolation::Linear,
        )
        .expect("curve");
        let vols = SeasonalVol::flat(cm(2027, 1), 0.70).expect("vols");
        let nodes: Vec<(Date, f64)> = (1..=24).map(|k| (val.add_months(k).expect("d"), (-0.04 * k as f64 / 12.0).exp())).collect();
        let disc = DiscountCurve::new(val, &nodes, Interpolation::LogLinearDiscount).expect("disc");
        (c, vols, disc, NymexCalendar::new())
    }

    #[test]
    fn intrinsic_nomination_respects_caps_and_prefers_best_margin() {
        let (c, vols, disc, cal) = market();
        // Strike 3.80: Jan/Feb in the money, Mar/Apr out. Max 10k/d, min 2k/d,
        // total cap 500k < Jan+Feb max (31+28)*10k = 590k, so the cap binds.
        let sw = SwingOption::new(GasHub::HenryHub, cm(2027, 1), cm(2027, 4), 3.80, 2_000.0, 10_000.0, 300_000.0, 500_000.0).expect("swing");
        let v = sw.value(&c, &vols, &disc, &cal, DayCount::Act365Fixed).expect("v");
        let t = &v.intrinsic_takes;
        // Minimums everywhere.
        for (i, m) in v.months.iter().enumerate() {
            assert!(t[i] >= 2_000.0 * f64::from(m.delivery().expect("d").days()) - 1e-6);
        }
        // Jan (best margin) at max; Mar/Apr at min.
        assert_abs_diff_eq!(t[0], 310_000.0, epsilon = 1e-6);
        assert_abs_diff_eq!(t[2], 62_000.0, epsilon = 1e-6);
        assert_abs_diff_eq!(t[3], 60_000.0, epsilon = 1e-6);
        let total: f64 = t.iter().sum();
        assert_abs_diff_eq!(total, 500_000.0, epsilon = 1e-6);
        assert!(v.intrinsic_pv > 0.0);
        assert!(v.upper_bound_pv >= v.intrinsic_pv);
    }

    #[test]
    fn floor_forces_takes_in_least_bad_months() {
        let (c, vols, disc, cal) = market();
        // Strike 5.00: nothing is in the money, but 400k must be taken.
        let sw = SwingOption::new(GasHub::HenryHub, cm(2027, 1), cm(2027, 4), 5.00, 0.0, 10_000.0, 400_000.0, 1_000_000.0).expect("swing");
        let v = sw.value(&c, &vols, &disc, &cal, DayCount::Act365Fixed).expect("v");
        let total: f64 = v.intrinsic_takes.iter().sum();
        assert_abs_diff_eq!(total, 400_000.0, epsilon = 1e-6);
        // Jan (4.40) is least bad: filled to max 310k, then Feb 90k.
        assert_abs_diff_eq!(v.intrinsic_takes[0], 310_000.0, epsilon = 1e-6);
        assert_abs_diff_eq!(v.intrinsic_takes[1], 90_000.0, epsilon = 1e-6);
        assert!(v.intrinsic_pv < 0.0);
    }

    #[test]
    fn rolling_sits_between_intrinsic_and_upper_bound() {
        let (c, vols, disc, cal) = market();
        let sw = SwingOption::new(GasHub::HenryHub, cm(2027, 1), cm(2027, 4), 3.80, 2_000.0, 10_000.0, 300_000.0, 500_000.0).expect("swing");
        let model = ClewlowStrickland::two_factor(0.70, 2.0, 0.2, 0.3).expect("model");
        let cfg = McConfig { paths: 4_000, seed: 3, antithetic: true };
        let v = sw.value_with_model(&c, &vols, &model, &disc, &cal, DayCount::Act365Fixed, cfg).expect("v");
        let (roll, se) = (v.rolling_pv.expect("r"), v.rolling_std_err.expect("se"));
        assert!(roll >= v.intrinsic_pv - 3.0 * se, "rolling {roll} < intrinsic {}", v.intrinsic_pv);
        assert!(roll <= v.upper_bound_pv + 3.0 * se, "rolling {roll} > upper {}", v.upper_bound_pv);
    }

    #[test]
    fn validation() {
        assert!(SwingOption::new(GasHub::HenryHub, cm(2027, 1), cm(2027, 2), 4.0, 5_000.0, 1_000.0, 0.0, 1e9).is_err());
        // Total floor above what daily maxima allow.
        assert!(SwingOption::new(GasHub::HenryHub, cm(2027, 1), cm(2027, 2), 4.0, 0.0, 1_000.0, 100_000.0, 200_000.0).is_err());
        assert!(SwingOption::new(GasHub::HenryHub, cm(2027, 1), cm(2027, 2), 4.0, 0.0, 1_000.0, 0.0, 10_000.0).is_ok());
    }
}
