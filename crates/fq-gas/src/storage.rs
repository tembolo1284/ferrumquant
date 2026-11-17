//! Gas storage valuation: intrinsic by dynamic programming, extrinsic by
//! rolling-intrinsic Monte Carlo.
//!
//! A storage lease gives the right to inject up to a daily rate, hold up to
//! a working-gas capacity, and withdraw up to a daily rate, paying per-unit
//! injection and withdrawal fees and losing a fuel fraction on the way in.
//! Its value is a constrained calendar spread: buy summer, sell winter,
//! and re-trade as the curve moves.
//!
//! **Intrinsic**: the best schedule against today's forward curve, found
//! exactly (up to the inventory grid) by backward induction over months and
//! inventory levels. This is what the desk can lock in with futures today.
//!
//! **Rolling intrinsic**: simulate the forward curve month by month with a
//! [`ClewlowStrickland`] model; at each month, re-solve the intrinsic
//! problem on the simulated curve from the current inventory, execute only
//! the first month's decision at that month's realised price, and roll
//! forward. Averaging the realised P&L gives a lower bound on the true
//! (stochastic DP) value that captures most of the extrinsic premium. It is
//! the number most desks mark storage to, and the natural one to hedge.
//!
//! Decisions are monthly here. Daily granularity, ratchets (rates that
//! depend on inventory) and a full LSMC come later; the DP core is written
//! so those are additions rather than rewrites.

use fq_commod_core::{ClewlowStrickland, ContractMonth, FuturesCurve, McConfig};
use fq_core::time::DayCount;
use fq_rates::curve::DiscountCurve;

use crate::error::GasError;
use crate::hub::{GasHub, NymexCalendar, nymex_expiry};

/// `(months, days per month, discount factor per month, year fraction to each NYMEX expiry)`.
type MonthData = (Vec<ContractMonth>, Vec<i32>, Vec<f64>, Vec<f64>);

#[derive(Debug, Clone, PartialEq)]
pub struct StorageFacility {
    pub hub: GasHub,
    /// First and last delivery months of the lease.
    pub start: ContractMonth,
    pub end: ContractMonth,
    /// Working gas capacity, MMBtu.
    pub capacity: f64,
    /// Maximum injection, MMBtu per day (into inventory, after fuel).
    pub max_injection: f64,
    /// Maximum withdrawal, MMBtu per day.
    pub max_withdrawal: f64,
    /// $/MMBtu injected.
    pub injection_cost: f64,
    /// $/MMBtu withdrawn.
    pub withdrawal_cost: f64,
    /// Fraction of purchased gas burned as fuel on injection: to add 1
    /// MMBtu to inventory you buy `1 / (1 - fuel_in)`.
    pub fuel_in: f64,
    pub initial_inventory: f64,
    /// Inventory that must remain at the end of the lease (leftover gas
    /// above this is valued at zero, conservatively).
    pub final_min_inventory: f64,
    /// Inventory grid resolution for the DP.
    pub grid_levels: usize,
}

impl StorageFacility {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        hub: GasHub,
        start: ContractMonth,
        end: ContractMonth,
        capacity: f64,
        max_injection: f64,
        max_withdrawal: f64,
        injection_cost: f64,
        withdrawal_cost: f64,
        fuel_in: f64,
    ) -> Result<Self, GasError> {
        if ContractMonth::range(start, end).is_empty() {
            return Err(GasError::EmptyPeriod { start, end });
        }
        for (n, x) in [("capacity", capacity), ("max injection", max_injection), ("max withdrawal", max_withdrawal)] {
            if !(x.is_finite() && x > 0.0) {
                return Err(GasError::Invalid(format!("{n} {x} must be positive")));
            }
        }
        for (n, x) in [("injection cost", injection_cost), ("withdrawal cost", withdrawal_cost)] {
            if !(x.is_finite() && x >= 0.0) {
                return Err(GasError::Invalid(format!("{n} {x} must be non-negative")));
            }
        }
        if !(fuel_in.is_finite() && (0.0..1.0).contains(&fuel_in)) {
            return Err(GasError::Invalid(format!("fuel_in {fuel_in} must be in [0, 1)")));
        }
        Ok(Self {
            hub,
            start,
            end,
            capacity,
            max_injection,
            max_withdrawal,
            injection_cost,
            withdrawal_cost,
            fuel_in,
            initial_inventory: 0.0,
            final_min_inventory: 0.0,
            grid_levels: 50,
        })
    }

    pub fn with_inventory(mut self, initial: f64, final_min: f64) -> Result<Self, GasError> {
        for (n, x) in [("initial inventory", initial), ("final minimum inventory", final_min)] {
            if !(x.is_finite() && (0.0..=self.capacity).contains(&x)) {
                return Err(GasError::Invalid(format!("{n} {x} must be within [0, capacity]")));
            }
        }
        self.initial_inventory = initial;
        self.final_min_inventory = final_min;
        Ok(self)
    }

    pub fn with_grid_levels(mut self, levels: usize) -> Result<Self, GasError> {
        if levels < 2 {
            return Err(GasError::Invalid("need at least 2 grid levels".into()));
        }
        self.grid_levels = levels;
        Ok(self)
    }

    pub fn months(&self) -> Vec<ContractMonth> {
        ContractMonth::range(self.start, self.end)
    }

    fn step(&self) -> f64 {
        self.capacity / self.grid_levels as f64
    }

    fn level_of(&self, inventory: f64) -> usize {
        ((inventory / self.step()).round() as usize).min(self.grid_levels)
    }

    /// Cash flow (undiscounted) of moving inventory by `delta` MMBtu at
    /// price `price`. Negative delta is a withdrawal (sale).
    fn cash(&self, delta: f64, price: f64) -> f64 {
        if delta > 0.0 { -(delta / (1.0 - self.fuel_in)) * price - self.injection_cost * delta } else { -delta * price - self.withdrawal_cost * (-delta) }
    }

    /// Backward induction over `prices[k]` for the remaining months, with
    /// `days[k]` and `dfs[k]` aligned. Returns the optimal value from
    /// `start_level` and the first-month inventory change (in levels).
    fn dp(&self, prices: &[f64], days: &[i32], dfs: &[f64], start_level: usize) -> Result<(f64, Vec<i64>), GasError> {
        let n = self.grid_levels;
        let m = prices.len();
        let step = self.step();
        let final_level = self.level_of(self.final_min_inventory);
        // value[l] for the current month boundary, initialised at terminal.
        let mut next: Vec<f64> = (0..=n).map(|l| if l >= final_level { 0.0 } else { f64::NEG_INFINITY }).collect();
        let mut policy: Vec<Vec<i64>> = vec![vec![0; n + 1]; m];
        for k in (0..m).rev() {
            let max_up = ((self.max_injection * f64::from(days[k])) / step).floor() as i64;
            let max_dn = ((self.max_withdrawal * f64::from(days[k])) / step).floor() as i64;
            if max_up == 0 || max_dn == 0 {
                return Err(GasError::Storage(format!("grid too coarse: a month's rate limit is below one level ({step:.0} MMBtu); raise grid_levels")));
            }
            let mut cur = vec![f64::NEG_INFINITY; n + 1];
            for i in 0..=n {
                let lo = (i as i64 - max_dn).max(0);
                let hi = (i as i64 + max_up).min(n as i64);
                let mut best = f64::NEG_INFINITY;
                let mut best_d = 0;
                for j in lo..=hi {
                    let cont = next[j as usize];
                    if cont == f64::NEG_INFINITY {
                        continue;
                    }
                    let delta = (j - i as i64) as f64 * step;
                    let v = self.cash(delta, prices[k]) * dfs[k] + cont;
                    if v > best {
                        best = v;
                        best_d = j - i as i64;
                    }
                }
                cur[i] = best;
                policy[k][i] = best_d;
            }
            next = cur;
        }
        let value = next[start_level];
        if value == f64::NEG_INFINITY {
            return Err(GasError::Storage("no feasible schedule reaches the final inventory constraint".into()));
        }
        // Forward pass for the full schedule in levels.
        let mut sched = Vec::with_capacity(m);
        let mut l = start_level as i64;
        for pol in &policy {
            let d = pol[l as usize];
            sched.push(d);
            l += d;
        }
        Ok((value, sched))
    }

    fn month_data(&self, discount: &DiscountCurve, cal: &NymexCalendar, dc: DayCount) -> Result<MonthData, GasError> {
        let valuation = discount.reference_date();
        let months = self.months();
        let mut days = Vec::with_capacity(months.len());
        let mut dfs = Vec::with_capacity(months.len());
        let mut expiries = Vec::with_capacity(months.len());
        for &m in &months {
            let e = nymex_expiry(m, cal)?;
            if e <= valuation {
                return Err(GasError::MonthExpired { month: m, valuation });
            }
            days.push(m.delivery()?.days());
            dfs.push(discount.discount(m.last_day()?)?);
            expiries.push(dc.year_fraction(valuation, e, None)?);
        }
        Ok((months, days, dfs, expiries))
    }
}

/// One month of an optimal (intrinsic) schedule.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StorageSchedule {
    pub month: ContractMonth,
    pub price: f64,
    pub inventory_start: f64,
    /// Positive injection or negative withdrawal, MMBtu into inventory.
    pub change: f64,
    pub cash_pv: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StorageValuation {
    pub intrinsic_pv: f64,
    pub schedule: Vec<StorageSchedule>,
    /// Rolling-intrinsic Monte Carlo value, if computed.
    pub rolling_pv: Option<f64>,
    pub rolling_std_err: Option<f64>,
    /// `rolling_pv - intrinsic_pv`, if computed.
    pub extrinsic_pv: Option<f64>,
}

/// Optimal schedule and value against today's curve.
pub fn intrinsic_storage(fac: &StorageFacility, curve: &FuturesCurve, discount: &DiscountCurve, cal: &NymexCalendar) -> Result<StorageValuation, GasError> {
    if curve.hub().code() != fac.hub.code() {
        return Err(GasError::WrongHub { expected: fac.hub.code().into(), got: curve.hub().to_string() });
    }
    let (months, days, dfs, _) = fac.month_data(discount, cal, DayCount::Act365Fixed)?;
    let prices: Vec<f64> = months.iter().map(|&m| curve.price(m)).collect::<Result<_, _>>()?;
    let start_level = fac.level_of(fac.initial_inventory);
    let (value, sched) = fac.dp(&prices, &days, &dfs, start_level)?;
    let step = fac.step();
    let mut inv = start_level as f64 * step;
    let schedule = months
        .iter()
        .zip(sched.iter())
        .zip(prices.iter().zip(dfs.iter()))
        .map(|((&m, &d), (&p, &df))| {
            let change = d as f64 * step;
            let row = StorageSchedule { month: m, price: p, inventory_start: inv, change, cash_pv: fac.cash(change, p) * df };
            inv += change;
            row
        })
        .collect();
    Ok(StorageValuation { intrinsic_pv: value, schedule, rolling_pv: None, rolling_std_err: None, extrinsic_pv: None })
}

/// Intrinsic plus rolling-intrinsic Monte Carlo under a Clewlow-Strickland
/// model of the hub's forward curve. Each month's contract is simulated to
/// its NYMEX expiry, where its price is taken as the realised monthly
/// index for that month's injections or withdrawals.
pub fn rolling_intrinsic_storage(
    fac: &StorageFacility,
    curve: &FuturesCurve,
    model: &ClewlowStrickland,
    discount: &DiscountCurve,
    cal: &NymexCalendar,
    dc: DayCount,
    cfg: McConfig,
) -> Result<StorageValuation, GasError> {
    let mut out = intrinsic_storage(fac, curve, discount, cal)?;
    let (months, days, dfs, expiries) = fac.month_data(discount, cal, dc)?;
    let m = months.len();
    let initial: Vec<(f64, f64)> = months.iter().zip(expiries.iter()).map(|(&cm, &t)| curve.price(cm).map(|f| (t, f))).collect::<Result<_, _>>()?;
    let paths = model.simulate(&initial, &expiries, cfg)?;
    let step = fac.step();
    let start_level = fac.level_of(fac.initial_inventory);

    let mut prices = vec![0.0; m];
    let mut err: Option<GasError> = None;
    let (mean, se) = paths.estimate(|p| {
        if err.is_some() {
            return 0.0;
        }
        let mut level = start_level;
        let mut pnl = 0.0;
        for k in 0..m {
            // Current view of the remaining curve: month k is at its expiry
            // (fixed), later months are live forwards.
            for (j, price) in prices.iter_mut().enumerate().skip(k) {
                *price = paths.forward(p, k, j);
            }
            match fac.dp(&prices[k..], &days[k..], &dfs[k..], level) {
                Ok((_, sched)) => {
                    let d = sched[0];
                    pnl += fac.cash(d as f64 * step, prices[k]) * dfs[k];
                    level = (level as i64 + d) as usize;
                }
                Err(e) => {
                    err = Some(e);
                    return 0.0;
                }
            }
        }
        pnl
    });
    if let Some(e) = err {
        return Err(e);
    }
    out.rolling_pv = Some(mean);
    out.rolling_std_err = Some(se);
    out.extrinsic_pv = Some(mean - out.intrinsic_pv);
    Ok(out)
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

    fn disc(val: Date) -> DiscountCurve {
        let nodes: Vec<(Date, f64)> = (1..=36).map(|k| (val.add_months(k).expect("d"), (-0.04 * k as f64 / 12.0).exp())).collect();
        DiscountCurve::new(val, &nodes, Interpolation::LogLinearDiscount).expect("disc")
    }

    fn curve(val: Date, quotes: &[(ContractMonth, f64)]) -> FuturesCurve {
        FuturesCurve::new(GasHub::HenryHub.hub(), val, quotes.iter().map(|&(m, p)| FuturesQuote::new(m, p)).collect(), CurveInterpolation::Linear)
            .expect("curve")
    }

    #[test]
    fn two_month_intrinsic_matches_hand_calculation() {
        let val = ymd(2026, 11, 9);
        // Inject Dec at 3.00, withdraw Jan at 4.50. Rates generous enough to
        // fill and empty in a month.
        let c = curve(val, &[(cm(2026, 12), 3.00), (cm(2027, 1), 4.50)]);
        let fac = StorageFacility::new(GasHub::HenryHub, cm(2026, 12), cm(2027, 1), 1_000_000.0, 100_000.0, 100_000.0, 0.02, 0.02, 0.01).expect("fac");
        let d = disc(val);
        let v = intrinsic_storage(&fac, &c, &d, &NymexCalendar::new()).expect("v");
        let df_dec = d.discount(ymd(2026, 12, 31)).expect("df");
        let df_jan = d.discount(ymd(2027, 1, 31)).expect("df");
        let buy = -(1_000_000.0 / 0.99) * 3.00 - 0.02 * 1_000_000.0;
        let sell = 1_000_000.0 * 4.50 - 0.02 * 1_000_000.0;
        assert_abs_diff_eq!(v.intrinsic_pv, buy * df_dec + sell * df_jan, epsilon = 1e-3);
        assert_eq!(v.schedule.len(), 2);
        assert_abs_diff_eq!(v.schedule[0].change, 1_000_000.0, epsilon = 1e-6);
        assert_abs_diff_eq!(v.schedule[1].change, -1_000_000.0, epsilon = 1e-6);
        assert_abs_diff_eq!(v.schedule[1].inventory_start, 1_000_000.0, epsilon = 1e-6);
        assert!(v.rolling_pv.is_none());
    }

    #[test]
    fn backwardation_with_empty_tank_is_worthless_and_rates_bind() {
        let val = ymd(2026, 11, 9);
        let c = curve(val, &[(cm(2026, 12), 5.00), (cm(2027, 1), 4.00), (cm(2027, 2), 3.50)]);
        let fac = StorageFacility::new(GasHub::HenryHub, cm(2026, 12), cm(2027, 2), 1_000_000.0, 100_000.0, 100_000.0, 0.02, 0.02, 0.0).expect("fac");
        let v = intrinsic_storage(&fac, &c, &disc(val), &NymexCalendar::new()).expect("v");
        assert_abs_diff_eq!(v.intrinsic_pv, 0.0, epsilon = 1e-9);
        assert!(v.schedule.iter().all(|s| s.change == 0.0));

        // Contango but a slow tank: 10,000/d over 31 days caps the cycle.
        let c2 = curve(val, &[(cm(2026, 12), 3.00), (cm(2027, 1), 4.50)]);
        let slow = StorageFacility::new(GasHub::HenryHub, cm(2026, 12), cm(2027, 1), 1_000_000.0, 10_000.0, 10_000.0, 0.0, 0.0, 0.0)
            .expect("fac")
            .with_grid_levels(100)
            .expect("grid");
        let v2 = intrinsic_storage(&slow, &c2, &disc(val), &NymexCalendar::new()).expect("v");
        assert!(v2.schedule[0].change <= 310_000.0 + 1e-6);
        assert!(v2.schedule[0].change >= 300_000.0);
        assert_abs_diff_eq!(v2.schedule[1].change, -v2.schedule[0].change, epsilon = 1e-6);
    }

    #[test]
    fn seasonal_cycle_injects_summer_withdraws_winter() {
        let val = ymd(2026, 11, 9);
        let months = ContractMonth::range(cm(2027, 4), cm(2028, 3));
        // Apr-Oct 3.0ish, Nov-Mar 4.5ish.
        let quotes: Vec<(ContractMonth, f64)> = months.iter().map(|&m| (m, if m.is_winter() { 4.50 } else { 3.00 })).collect();
        let c = curve(val, &quotes);
        let fac = StorageFacility::new(GasHub::HenryHub, cm(2027, 4), cm(2028, 3), 3_000_000.0, 20_000.0, 40_000.0, 0.02, 0.02, 0.01).expect("fac");
        let v = intrinsic_storage(&fac, &c, &disc(val), &NymexCalendar::new()).expect("v");
        assert!(v.intrinsic_pv > 0.0);
        for s in &v.schedule {
            if s.month.is_winter() {
                assert!(s.change <= 0.0, "{}: {}", s.month, s.change);
            } else {
                assert!(s.change >= 0.0, "{}: {}", s.month, s.change);
            }
        }
        let peak = v.schedule.iter().map(|s| s.inventory_start + s.change).fold(0.0, f64::max);
        assert!(peak <= 3_000_000.0 + 1e-6);
        assert!(peak > 2_500_000.0);
    }

    #[test]
    fn rolling_intrinsic_reduces_to_intrinsic_at_zero_vol_and_exceeds_it_otherwise() {
        let val = ymd(2026, 11, 9);
        let months = ContractMonth::range(cm(2027, 4), cm(2027, 11));
        let quotes: Vec<(ContractMonth, f64)> = months.iter().map(|&m| (m, if m.is_winter() { 4.20 } else { 3.20 })).collect();
        let c = curve(val, &quotes);
        let fac = StorageFacility::new(GasHub::HenryHub, cm(2027, 4), cm(2027, 11), 1_000_000.0, 15_000.0, 40_000.0, 0.02, 0.02, 0.01)
            .expect("fac")
            .with_grid_levels(25)
            .expect("grid");
        let d = disc(val);
        let cal = NymexCalendar::new();
        let still = ClewlowStrickland::one_factor(0.0, 1.0).expect("model");
        let cfg = McConfig { paths: 4, seed: 1, antithetic: true };
        let v0 = rolling_intrinsic_storage(&fac, &c, &still, &d, &cal, DayCount::Act365Fixed, cfg).expect("v");
        assert_abs_diff_eq!(v0.rolling_pv.expect("rolling"), v0.intrinsic_pv, epsilon = 1e-6);

        let model = ClewlowStrickland::two_factor(0.70, 1.5, 0.25, 0.3).expect("model");
        let cfg = McConfig { paths: 300, seed: 7, antithetic: true };
        let v = rolling_intrinsic_storage(&fac, &c, &model, &d, &cal, DayCount::Act365Fixed, cfg).expect("v");
        let (roll, se) = (v.rolling_pv.expect("rolling"), v.rolling_std_err.expect("se"));
        assert!(roll > v.intrinsic_pv, "rolling {roll} intrinsic {}", v.intrinsic_pv);
        assert!(se > 0.0 && se < roll);
        assert_abs_diff_eq!(v.extrinsic_pv.expect("ex"), roll - v.intrinsic_pv, epsilon = 1e-9);
    }

    #[test]
    fn validation() {
        let val = ymd(2026, 11, 9);
        let c = curve(val, &[(cm(2026, 12), 3.0), (cm(2027, 1), 4.0)]);
        assert!(StorageFacility::new(GasHub::HenryHub, cm(2027, 1), cm(2026, 12), 1.0, 1.0, 1.0, 0.0, 0.0, 0.0).is_err());
        assert!(StorageFacility::new(GasHub::HenryHub, cm(2026, 12), cm(2027, 1), 1.0, 1.0, 1.0, 0.0, 0.0, 1.0).is_err());
        let fac = StorageFacility::new(GasHub::HenryHub, cm(2026, 12), cm(2027, 1), 1_000_000.0, 1_000.0, 1_000.0, 0.0, 0.0, 0.0).expect("fac");
        // 1,000/d * 31 d = 31,000 < one level (20,000) at 50 levels? 1e6/50 = 20,000, fine.
        // At 10 levels a level is 100,000 and the rate can't move one level.
        let coarse = fac.clone().with_grid_levels(10).expect("grid");
        assert!(matches!(intrinsic_storage(&coarse, &c, &disc(val), &NymexCalendar::new()), Err(GasError::Storage(_))));
        // Final inventory unreachable.
        let impossible = fac.with_inventory(0.0, 1_000_000.0).expect("inv");
        assert!(matches!(intrinsic_storage(&impossible, &c, &disc(val), &NymexCalendar::new()), Err(GasError::Storage(_))));
    }
}
