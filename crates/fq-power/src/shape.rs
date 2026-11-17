//! Hourly shaping of block forwards.
//!
//! Dispatch and load-following products need an hourly price path, but the
//! market only quotes two blocks a month. Shaping takes an intraday profile
//! (relative price by hour-ending for peak days and for non-peak days,
//! typically fitted to history) and rescales it so that the hours-weighted
//! average over the month's peak hours reproduces the peak forward and over
//! the off-peak hours the off-peak forward. The shape is the desk's view;
//! the levels are the market's.
//!
//! Daylight-saving days are handled explicitly: the 23-hour day in March
//! has no hour-ending 3, the 25-hour day in November has hour-ending 2
//! twice.

use fq_commod_core::ContractMonth;
use fq_core::time::Date;

use crate::curve::PowerCurve;
use crate::error::PowerError;
use crate::hub::PowerHub;
use crate::iso::{Block, hours_in_day, is_peak_day, is_peak_hour};

/// Relative intraday price multipliers indexed by hour-ending (index 0 is
/// HE1). Only ratios matter; the shaping normalises levels.
#[derive(Debug, Clone, PartialEq)]
pub struct HourlyProfile {
    pub peak_day: [f64; 24],
    pub nonpeak_day: [f64; 24],
}

impl HourlyProfile {
    pub fn new(peak_day: [f64; 24], nonpeak_day: [f64; 24]) -> Result<Self, PowerError> {
        for (n, arr) in [("peak day", &peak_day), ("non-peak day", &nonpeak_day)] {
            if arr.iter().any(|x| !(x.is_finite() && *x > 0.0)) {
                return Err(PowerError::Invalid(format!("{n} profile must be positive and finite")));
            }
        }
        Ok(Self { peak_day, nonpeak_day })
    }

    /// A generic two-hump weekday shape (morning ramp, evening peak) and a
    /// flatter weekend shape. Good enough for tests and as a starting point.
    pub fn typical() -> Self {
        let peak_day = [
            0.72, 0.68, 0.66, 0.66, 0.70, 0.80, 0.95, 1.10, 1.15, 1.12, 1.10, 1.08, 1.06, 1.05, 1.07, 1.12, 1.20, 1.35, 1.45, 1.40, 1.25, 1.10, 0.95, 0.82,
        ];
        let nonpeak_day = [
            0.78, 0.74, 0.72, 0.72, 0.74, 0.78, 0.85, 0.92, 0.98, 1.02, 1.04, 1.05, 1.05, 1.04, 1.05, 1.08, 1.15, 1.25, 1.32, 1.28, 1.18, 1.05, 0.95, 0.85,
        ];
        Self { peak_day, nonpeak_day }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HourPrice {
    pub date: Date,
    /// 1..=24; the November fall-back hour repeats hour-ending 2.
    pub hour_ending: u32,
    pub peak: bool,
    pub price: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HourlyPrices {
    pub hub: PowerHub,
    pub month: ContractMonth,
    pub hours: Vec<HourPrice>,
}

impl HourlyPrices {
    pub fn prices(&self) -> Vec<f64> {
        self.hours.iter().map(|h| h.price).collect()
    }

    /// Simple average over the hours of a block (for checking the shaping
    /// reproduces the forwards).
    pub fn block_average(&self, block: Block) -> Option<f64> {
        let sel: Vec<f64> = self
            .hours
            .iter()
            .filter(|h| match block {
                Block::Peak => h.peak,
                Block::OffPeak => !h.peak,
                Block::Atc => true,
                Block::Wrap2x16 => !h.peak && is_peak_hour(h.hour_ending),
                Block::Nights7x8 => !is_peak_hour(h.hour_ending),
            })
            .map(|h| h.price)
            .collect();
        if sel.is_empty() { None } else { Some(sel.iter().sum::<f64>() / sel.len() as f64) }
    }
}

/// The hour-endings of a day in order, accounting for DST.
fn hour_endings(d: Date) -> Vec<u32> {
    match hours_in_day(d) {
        23 => (1..=24).filter(|&h| h != 3).collect(),
        25 => {
            let mut v = vec![1, 2, 2];
            v.extend(3..=24);
            v
        }
        _ => (1..=24).collect(),
    }
}

/// Hourly prices for one delivery month, matching the curve's peak and
/// off-peak forwards on average.
pub fn shape_month(curve: &PowerCurve, month: ContractMonth, profile: &HourlyProfile) -> Result<HourlyPrices, PowerError> {
    let iso = curve.hub().iso();
    let f_peak = curve.block_price(month, Block::Peak)?.price;
    let f_off = curve.block_price(month, Block::OffPeak)?.price;
    let period = month.delivery()?;

    // First pass: raw weights and their means per block.
    let mut raw: Vec<(Date, u32, bool, f64)> = Vec::new();
    let (mut sum_p, mut n_p, mut sum_o, mut n_o) = (0.0, 0usize, 0.0, 0usize);
    for s in period.start().serial()..=period.end().serial() {
        let d = Date::from_serial(s)?;
        let pd = is_peak_day(iso, d);
        let prof = if pd { &profile.peak_day } else { &profile.nonpeak_day };
        for he in hour_endings(d) {
            let w = prof[(he - 1) as usize];
            let peak = pd && is_peak_hour(he);
            if peak {
                sum_p += w;
                n_p += 1;
            } else {
                sum_o += w;
                n_o += 1;
            }
            raw.push((d, he, peak, w));
        }
    }
    let scale_p = if n_p > 0 { f_peak / (sum_p / n_p as f64) } else { 0.0 };
    let scale_o = if n_o > 0 { f_off / (sum_o / n_o as f64) } else { 0.0 };
    let hours =
        raw.into_iter().map(|(date, hour_ending, peak, w)| HourPrice { date, hour_ending, peak, price: w * if peak { scale_p } else { scale_o } }).collect();
    Ok(HourlyPrices { hub: curve.hub(), month, hours })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::curve::tests::pjm_curve;
    use approx::assert_abs_diff_eq;

    fn cm(y: i32, m: u32) -> ContractMonth {
        ContractMonth::new(y, m).expect("month")
    }

    #[test]
    fn shaped_hours_reproduce_block_forwards() {
        let c = pjm_curve();
        let h = shape_month(&c, cm(2027, 1), &HourlyProfile::typical()).expect("shape");
        assert_eq!(h.hours.len(), 744);
        assert_eq!(h.hours.iter().filter(|x| x.peak).count(), 320);
        assert_abs_diff_eq!(h.block_average(Block::Peak).expect("p"), 95.0, epsilon = 1e-9);
        assert_abs_diff_eq!(h.block_average(Block::OffPeak).expect("o"), 60.0, epsilon = 1e-9);
        // Evening peak hour is above the morning on a weekday.
        let he19 = h.hours.iter().find(|x| x.date.day() == 4 && x.hour_ending == 19).expect("he19").price;
        let he8 = h.hours.iter().find(|x| x.date.day() == 4 && x.hour_ending == 8).expect("he8").price;
        assert!(he19 > he8);
    }

    #[test]
    fn dst_months_have_the_right_hour_count() {
        let c = pjm_curve();
        let mar = shape_month(&c, cm(2027, 3), &HourlyProfile::typical()).expect("shape");
        assert_eq!(mar.hours.len(), 31 * 24 - 1);
        assert!(!mar.hours.iter().any(|x| x.date.day() == 14 && x.hour_ending == 3));
        // Extend the curve to November for the 25-hour day.
        let he2_count = hour_endings(Date::from_ymd(2027, 11, 7).expect("d")).iter().filter(|&&h| h == 2).count();
        assert_eq!(he2_count, 2);
    }

    #[test]
    fn profile_validation() {
        let mut bad = HourlyProfile::typical();
        bad.peak_day[5] = 0.0;
        assert!(HourlyProfile::new(bad.peak_day, bad.nonpeak_day).is_err());
    }
}
