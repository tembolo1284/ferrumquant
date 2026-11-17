//! Unit commitment for a single thermal unit by dynamic programming.
//!
//! A heat-rate option strip prices a plant as if it could switch on and
//! off every hour for free. Real units cannot: starting costs money and
//! takes time, once on they must stay on for a minimum number of hours,
//! once off they must stay off, and below minimum load they cannot run at
//! all. Against an hourly price path the optimal commitment is a small
//! dynamic program over the state (on/off, hours in that state), and the
//! difference between its value and the frictionless strip is the cost of
//! the plant's inflexibility, which is what a tolling counterparty is
//! really negotiating over.
//!
//! The dispatch here is deterministic (perfect foresight of the hourly
//! path), so on a shaped forward curve it is the intrinsic dispatch value.
//! Run it over simulated hourly paths for an expected value.

use crate::error::PowerError;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ThermalUnit {
    /// Maximum output, MW.
    pub capacity_mw: f64,
    /// Minimum stable output when on, MW.
    pub min_load_mw: f64,
    /// MMBtu per MWh at full load (a single heat rate; no part-load curve).
    pub heat_rate: f64,
    /// Variable O&M, $/MWh.
    pub vom: f64,
    /// $ per start.
    pub start_cost: f64,
    pub min_up_hours: u32,
    pub min_down_hours: u32,
    /// State at the start of the horizon.
    pub initially_on: bool,
    /// Hours already spent in the initial state (so constraints can be
    /// already satisfied at hour 0).
    pub initial_hours_in_state: u32,
}

impl ThermalUnit {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        capacity_mw: f64,
        min_load_mw: f64,
        heat_rate: f64,
        vom: f64,
        start_cost: f64,
        min_up_hours: u32,
        min_down_hours: u32,
    ) -> Result<Self, PowerError> {
        if !(capacity_mw.is_finite() && capacity_mw > 0.0) {
            return Err(PowerError::Invalid(format!("capacity {capacity_mw} must be positive")));
        }
        if !(min_load_mw.is_finite() && (0.0..=capacity_mw).contains(&min_load_mw)) {
            return Err(PowerError::Invalid(format!("min load {min_load_mw} must be within [0, capacity]")));
        }
        if !(heat_rate.is_finite() && heat_rate > 0.0) {
            return Err(PowerError::Invalid(format!("heat rate {heat_rate} must be positive")));
        }
        for (n, x) in [("VOM", vom), ("start cost", start_cost)] {
            if !(x.is_finite() && x >= 0.0) {
                return Err(PowerError::Invalid(format!("{n} {x} must be non-negative")));
            }
        }
        Ok(Self { capacity_mw, min_load_mw, heat_rate, vom, start_cost, min_up_hours, min_down_hours, initially_on: false, initial_hours_in_state: u32::MAX })
    }

    pub fn starting(mut self, on: bool, hours_in_state: u32) -> Self {
        self.initially_on = on;
        self.initial_hours_in_state = hours_in_state;
        self
    }

    /// Margin per MWh at a given power and gas price.
    pub fn margin(&self, power: f64, gas: f64) -> f64 {
        power - self.heat_rate * gas - self.vom
    }

    /// Best output and hourly profit when on: full load if the margin is
    /// positive, minimum load otherwise.
    fn on_profit(&self, power: f64, gas: f64) -> (f64, f64) {
        let m = self.margin(power, gas);
        let mw = if m >= 0.0 { self.capacity_mw } else { self.min_load_mw };
        (mw, m * mw)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DispatchHour {
    pub on: bool,
    pub mw: f64,
    pub started: bool,
    /// Energy margin less any start cost this hour.
    pub profit: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DispatchResult {
    pub value: f64,
    pub schedule: Vec<DispatchHour>,
    pub starts: u32,
    pub hours_on: u32,
    /// Value of the same hours with no start cost or min up/down
    /// constraints (each hour independently), for comparison.
    pub frictionless_value: f64,
}

/// Optimal commitment of `unit` against `power` prices ($/MWh) and `gas`
/// prices ($/MMBtu), one entry per hour. `gas` may have one entry (flat)
/// or one per hour.
pub fn dispatch(unit: &ThermalUnit, power: &[f64], gas: &[f64]) -> Result<DispatchResult, PowerError> {
    let n = power.len();
    if n == 0 {
        return Err(PowerError::Dispatch("no hours".into()));
    }
    if gas.len() != 1 && gas.len() != n {
        return Err(PowerError::Dispatch(format!("gas has {} entries, need 1 or {n}", gas.len())));
    }
    if power.iter().chain(gas.iter()).any(|x| !x.is_finite()) {
        return Err(PowerError::Dispatch("non-finite price".into()));
    }
    let gas_at = |h: usize| if gas.len() == 1 { gas[0] } else { gas[h] };

    // States: on with k hours (1..=up), off with k hours (1..=dn), k capped.
    let up = unit.min_up_hours.max(1) as usize;
    let dn = unit.min_down_hours.max(1) as usize;
    let n_states = up + dn;
    let on_idx = |k: usize| k - 1; // k in 1..=up
    let off_idx = |k: usize| up + k - 1; // k in 1..=dn
    let neg = f64::NEG_INFINITY;

    // value[h][s] = best future profit from hour h onward in state s *before* deciding hour h.
    let mut value = vec![vec![0.0; n_states]; n + 1];
    // choice[h][s] = true if the unit is on during hour h from state s.
    let mut choice = vec![vec![false; n_states]; n];
    for h in (0..n).rev() {
        let (_, p_on) = unit.on_profit(power[h], gas_at(h));
        for k in 1..=up {
            // On for k hours already (k capped at up). Stay on -> on(min(k+1, up)); turn off (if k >= up) -> off(1).
            let stay = p_on + value[h + 1][on_idx((k + 1).min(up))];
            let off = if k >= up { value[h + 1][off_idx(1)] } else { neg };
            let s = on_idx(k);
            if stay >= off {
                value[h][s] = stay;
                choice[h][s] = true;
            } else {
                value[h][s] = off;
                choice[h][s] = false;
            }
        }
        for k in 1..=dn {
            let stay_off = value[h + 1][off_idx((k + 1).min(dn))];
            let start = if k >= dn { p_on - unit.start_cost + value[h + 1][on_idx(1)] } else { neg };
            let s = off_idx(k);
            if start > stay_off {
                value[h][s] = start;
                choice[h][s] = true;
            } else {
                value[h][s] = stay_off;
                choice[h][s] = false;
            }
        }
    }

    // Initial state.
    let init_k = unit.initial_hours_in_state.max(1) as usize;
    let mut state = if unit.initially_on { on_idx(init_k.min(up)) } else { off_idx(init_k.min(dn)) };
    let total = value[0][state];

    // Forward pass.
    let mut schedule = Vec::with_capacity(n);
    let (mut starts, mut hours_on, mut frictionless) = (0u32, 0u32, 0.0);
    for h in 0..n {
        let (mw_on, p_on) = unit.on_profit(power[h], gas_at(h));
        frictionless += p_on.max(0.0);
        let was_on = state < up;
        let on = choice[h][state];
        let started = on && !was_on;
        let profit = if on { p_on - if started { unit.start_cost } else { 0.0 } } else { 0.0 };
        if started {
            starts += 1;
        }
        if on {
            hours_on += 1;
        }
        schedule.push(DispatchHour { on, mw: if on { mw_on } else { 0.0 }, started, profit });
        state = if on {
            let k = if was_on { state + 1 + 1 } else { 1 }; // on_idx(k) = k-1 -> k = state+1; next k+1
            on_idx(k.min(up))
        } else {
            let k = if was_on { 1 } else { state - up + 1 + 1 };
            off_idx(k.min(dn))
        };
    }
    Ok(DispatchResult { value: total, schedule, starts, hours_on, frictionless_value: frictionless })
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_abs_diff_eq;

    fn unit() -> ThermalUnit {
        // 200 MW CCGT-ish: 7.0 HR, $2 VOM, $15,000 start, 4h min up, 4h min down.
        ThermalUnit::new(200.0, 100.0, 7.0, 2.0, 15_000.0, 4, 4).expect("unit")
    }

    #[test]
    fn always_profitable_runs_flat_out_with_one_start() {
        let u = unit();
        let power = vec![80.0; 48];
        let r = dispatch(&u, &power, &[4.0]).expect("dispatch");
        // Margin 80 - 28 - 2 = 50 $/MWh * 200 MW * 48 h - one start.
        assert_abs_diff_eq!(r.value, 50.0 * 200.0 * 48.0 - 15_000.0, epsilon = 1e-6);
        assert_eq!(r.starts, 1);
        assert_eq!(r.hours_on, 48);
        assert!(r.schedule[0].started);
        assert_abs_diff_eq!(r.frictionless_value, 50.0 * 200.0 * 48.0, epsilon = 1e-6);
        // Already on: no start cost.
        let r2 = dispatch(&u.starting(true, 10), &power, &[4.0]).expect("dispatch");
        assert_abs_diff_eq!(r2.value, 50.0 * 200.0 * 48.0, epsilon = 1e-6);
        assert_eq!(r2.starts, 0);
    }

    #[test]
    fn never_profitable_stays_off() {
        let u = unit();
        let r = dispatch(&u, &[20.0; 24], &[4.0]).expect("dispatch");
        assert_abs_diff_eq!(r.value, 0.0);
        assert_eq!(r.hours_on, 0);
        assert!(r.schedule.iter().all(|h| !h.on));
    }

    #[test]
    fn rides_through_a_short_dip_rather_than_cycling() {
        let u = unit();
        // 8 good hours, 2 bad hours (margin -10 at min load = -1,000/h), 8 good hours.
        // Cycling would cost a $15,000 start; riding through costs $2,000.
        let mut power = vec![80.0; 18];
        power[8] = 20.0;
        power[9] = 20.0;
        let r = dispatch(&u, &power, &[4.0]).expect("dispatch");
        assert_eq!(r.starts, 1);
        assert!(r.schedule[8].on && r.schedule[9].on);
        assert_abs_diff_eq!(r.schedule[8].mw, 100.0);
        assert_abs_diff_eq!(r.value, 16.0 * 50.0 * 200.0 - 2.0 * 10.0 * 100.0 - 15_000.0, epsilon = 1e-6);
        // Frictionless would skip the dip.
        assert_abs_diff_eq!(r.frictionless_value, 16.0 * 50.0 * 200.0, epsilon = 1e-6);
    }

    #[test]
    fn min_up_time_is_respected() {
        let u = unit();
        // One hugely profitable hour then unprofitable: once started the
        // unit must stay on 4 hours, so it only starts if the spike pays
        // for the 3 hours at min load plus the start.
        let mut power = vec![20.0; 10];
        power[2] = 500.0;
        let r = dispatch(&u, &power, &[4.0]).expect("dispatch");
        // Spike margin 470 * 200 = 94,000; three bad hours -10*100 = -3,000; start 15,000.
        assert_abs_diff_eq!(r.value, 94_000.0 - 3_000.0 - 15_000.0, epsilon = 1e-6);
        assert_eq!(r.hours_on, 4);
        let on: Vec<bool> = r.schedule.iter().map(|h| h.on).collect();
        assert_eq!(on, vec![false, false, true, true, true, true, false, false, false, false]);
        // A smaller spike that cannot pay for the obligation is skipped.
        power[2] = 100.0;
        let r2 = dispatch(&u, &power, &[4.0]).expect("dispatch");
        assert_abs_diff_eq!(r2.value, 0.0);
    }

    #[test]
    fn validation() {
        assert!(ThermalUnit::new(0.0, 0.0, 7.0, 2.0, 0.0, 1, 1).is_err());
        assert!(ThermalUnit::new(100.0, 150.0, 7.0, 2.0, 0.0, 1, 1).is_err());
        let u = unit();
        assert!(dispatch(&u, &[], &[4.0]).is_err());
        assert!(dispatch(&u, &[50.0, 50.0], &[4.0, 4.0, 4.0]).is_err());
    }
}
