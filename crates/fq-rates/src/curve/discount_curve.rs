//! Discount curve on dated nodes.
//!
//! Time is ACT/365F from the reference date. The reference node (t = 0,
//! DF = 1) is implicit. `LogLinearDiscount` (piecewise-constant instantaneous
//! forwards) is the default and the one the bootstrap uses; `LinearZero`
//! interpolates continuously-compounded zero rates. Beyond the last node both
//! extrapolate flat in the interpolated quantity's slope (flat forward /
//! flat zero respectively).

use thiserror::Error;

use fq_core::time::{Date, DateError, DayCount, DayCountError};

#[derive(Debug, Error, Clone, PartialEq)]
pub enum CurveError {
    #[error(transparent)]
    Date(#[from] DateError),
    #[error(transparent)]
    DayCount(#[from] DayCountError),
    #[error("curve needs at least one node")]
    EmptyNodes,
    #[error("node dates must be strictly increasing and after the reference date {reference}; offending node {node}")]
    BadNodeOrder { reference: Date, node: Date },
    #[error("discount factor {df} at {node} is not positive")]
    NonPositiveDiscount { node: Date, df: f64 },
    #[error("date {0} is before the curve reference date")]
    BeforeReference(Date),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Interpolation {
    #[default]
    LogLinearDiscount,
    LinearZero,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DiscountCurve {
    reference: Date,
    dates: Vec<Date>,
    times: Vec<f64>,
    /// ln DF at each node (t = 0 node included at index 0).
    log_dfs: Vec<f64>,
    interpolation: Interpolation,
}

impl DiscountCurve {
    /// `nodes` are (date, discount factor), strictly increasing, all after `reference`.
    pub fn new(reference: Date, nodes: &[(Date, f64)], interpolation: Interpolation) -> Result<Self, CurveError> {
        if nodes.is_empty() {
            return Err(CurveError::EmptyNodes);
        }
        let mut dates = Vec::with_capacity(nodes.len() + 1);
        let mut times = Vec::with_capacity(nodes.len() + 1);
        let mut log_dfs = Vec::with_capacity(nodes.len() + 1);
        dates.push(reference);
        times.push(0.0);
        log_dfs.push(0.0);
        let mut prev = reference;
        for &(d, df) in nodes {
            if d <= prev {
                return Err(CurveError::BadNodeOrder { reference, node: d });
            }
            if df <= 0.0 || !df.is_finite() {
                return Err(CurveError::NonPositiveDiscount { node: d, df });
            }
            dates.push(d);
            times.push(year_fraction(reference, d));
            log_dfs.push(df.ln());
            prev = d;
        }
        Ok(Self { reference, dates, times, log_dfs, interpolation })
    }

    /// Flat continuously-compounded curve, handy for tests and as a bootstrap seed.
    pub fn flat(reference: Date, rate: f64, horizon: Date) -> Result<Self, CurveError> {
        let t = year_fraction(reference, horizon);
        Self::new(reference, &[(horizon, (-rate * t).exp())], Interpolation::LogLinearDiscount)
    }

    pub fn reference_date(&self) -> Date {
        self.reference
    }

    pub fn interpolation(&self) -> Interpolation {
        self.interpolation
    }

    /// Node (date, DF) pairs, excluding the implicit reference node.
    pub fn nodes(&self) -> Vec<(Date, f64)> {
        self.dates.iter().zip(&self.log_dfs).skip(1).map(|(&d, &l)| (d, l.exp())).collect()
    }

    /// ACT/365F time from the reference date.
    pub fn time(&self, d: Date) -> f64 {
        year_fraction(self.reference, d)
    }

    pub fn discount(&self, d: Date) -> Result<f64, CurveError> {
        if d < self.reference {
            return Err(CurveError::BeforeReference(d));
        }
        Ok(self.discount_t(self.time(d)))
    }

    /// Discount factor at time `t >= 0` (years).
    pub fn discount_t(&self, t: f64) -> f64 {
        self.log_discount_t(t).exp()
    }

    /// Continuously-compounded zero rate to `d`.
    pub fn zero_rate(&self, d: Date) -> Result<f64, CurveError> {
        let t = self.time(d);
        if d < self.reference {
            return Err(CurveError::BeforeReference(d));
        }
        if t <= 0.0 {
            return Ok(self.zero_rate_t(f64::EPSILON));
        }
        Ok(-self.log_discount_t(t) / t)
    }

    /// Simple forward rate between two dates under `day_count`.
    pub fn forward_rate(&self, d1: Date, d2: Date, day_count: DayCount) -> Result<f64, CurveError> {
        let yf = day_count.year_fraction(d1, d2, None)?;
        Ok((self.discount(d1)? / self.discount(d2)? - 1.0) / yf)
    }

    /// Parallel shift of the continuously-compounded zero curve by `shift`
    /// (e.g. 1e-4 for +1bp): DF(t) -> DF(t) * exp(-shift * t).
    pub fn shifted(&self, shift: f64) -> Self {
        let log_dfs = self.log_dfs.iter().zip(&self.times).map(|(&l, &t)| l - shift * t).collect();
        Self { log_dfs, ..self.clone() }
    }

    /// Same curve with node `i` (0-based, excluding the reference node)
    /// shifted in zero-rate terms; for bucketed risk.
    pub fn with_node_shift(&self, i: usize, shift: f64) -> Result<Self, CurveError> {
        let mut c = self.clone();
        let Some(l) = c.log_dfs.get_mut(i + 1) else {
            return Err(CurveError::EmptyNodes);
        };
        *l -= shift * self.times[i + 1];
        Ok(c)
    }

    fn zero_rate_t(&self, t: f64) -> f64 {
        -self.log_discount_t(t) / t
    }

    fn log_discount_t(&self, t: f64) -> f64 {
        let n = self.times.len();
        // Locate the segment [times[i], times[i+1]] containing t; extrapolate on the last one.
        let i = match self.times.partition_point(|&x| x <= t) {
            0 => 0,
            k if k >= n => n - 2,
            k => k - 1,
        };
        let (t0, t1) = (self.times[i], self.times[i + 1]);
        let (l0, l1) = (self.log_dfs[i], self.log_dfs[i + 1]);
        match self.interpolation {
            Interpolation::LogLinearDiscount => {
                let w = (t - t0) / (t1 - t0);
                l0 + w * (l1 - l0)
            }
            Interpolation::LinearZero => {
                // Zero at the reference node is the first segment's zero (avoids 0/0).
                let z0 = if t0 > 0.0 { -l0 / t0 } else { -l1 / t1 };
                let z1 = -l1 / t1;
                let w = (t - t0) / (t1 - t0);
                -(z0 + w * (z1 - z0)) * t
            }
        }
    }
}

fn year_fraction(from: Date, to: Date) -> f64 {
    f64::from(to - from) / 365.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_abs_diff_eq;

    type R = Result<(), CurveError>;

    fn d(y: i32, m: u32, dd: u32) -> Date {
        Date::from_ymd(y, m, dd).expect("valid test date")
    }

    const REF: (i32, u32, u32) = (2026, 9, 23);

    #[test]
    fn flat_curve_reproduces_rate_everywhere() -> R {
        let r = d(REF.0, REF.1, REF.2);
        let c = DiscountCurve::flat(r, 0.04, d(2028, 9, 23))?;
        for (date, t) in [(d(2027, 3, 23), 181.0 / 365.0), (d(2027, 9, 23), 1.0), (d(2031, 9, 23), 1826.0 / 365.0)] {
            assert_abs_diff_eq!(c.discount(date)?, (-0.04_f64 * t).exp(), epsilon = 1e-14);
            assert_abs_diff_eq!(c.zero_rate(date)?, 0.04, epsilon = 1e-12);
        }
        assert_abs_diff_eq!(c.discount(r)?, 1.0, epsilon = 1e-15);
        Ok(())
    }

    #[test]
    fn log_linear_interpolates_in_log_space() -> R {
        let r = d(REF.0, REF.1, REF.2);
        let c = DiscountCurve::new(r, &[(d(2027, 9, 23), 0.96), (d(2028, 9, 22), 0.90)], Interpolation::LogLinearDiscount)?;
        // 2028-03-23 is exactly halfway (182 of 365 days) between the two nodes.
        let mid = c.discount(d(2028, 3, 23))?;
        let expected = (0.5 * (0.96_f64.ln() + 0.90_f64.ln()) + (182.0 / 365.0 - 0.5) * (0.90_f64.ln() - 0.96_f64.ln())).exp();
        assert_abs_diff_eq!(mid, expected, epsilon = 1e-14);
        // Flat-forward extrapolation continues the last segment's slope.
        let fwd_last = c.forward_rate(d(2027, 9, 23), d(2028, 9, 22), DayCount::Act365Fixed)?;
        let fwd_ext = c.forward_rate(d(2028, 9, 22), d(2029, 9, 22), DayCount::Act365Fixed)?;
        assert_abs_diff_eq!(fwd_last, fwd_ext, epsilon = 1e-12);
        Ok(())
    }

    #[test]
    fn linear_zero_interpolates_zero_rates() -> R {
        let r = d(REF.0, REF.1, REF.2);
        let (d1, d2) = (d(2027, 9, 23), d(2028, 9, 22));
        let c = DiscountCurve::new(r, &[(d1, (-0.03_f64).exp()), (d2, (-0.05_f64 * 2.0).exp())], Interpolation::LinearZero)?;
        assert_abs_diff_eq!(c.zero_rate(d1)?, 0.03, epsilon = 1e-12);
        assert_abs_diff_eq!(c.zero_rate(d2)?, 0.05, epsilon = 1e-12);
        let mid = d(2028, 3, 23);
        let w = c.time(mid) - 1.0;
        assert_abs_diff_eq!(c.zero_rate(mid)?, 0.03 + w * 0.02, epsilon = 1e-12);
        Ok(())
    }

    #[test]
    fn parallel_and_bucket_shifts() -> R {
        let r = d(REF.0, REF.1, REF.2);
        let c = DiscountCurve::new(r, &[(d(2027, 9, 23), 0.96), (d(2028, 9, 22), 0.90)], Interpolation::LogLinearDiscount)?;
        let up = c.shifted(1e-4);
        let date = d(2028, 3, 23);
        assert_abs_diff_eq!(up.zero_rate(date)?, c.zero_rate(date)? + 1e-4, epsilon = 1e-12);
        let bucket = c.with_node_shift(1, 1e-4)?;
        assert_abs_diff_eq!(bucket.discount(d(2027, 9, 23))?, 0.96, epsilon = 1e-15);
        assert_abs_diff_eq!(bucket.zero_rate(d(2028, 9, 22))?, c.zero_rate(d(2028, 9, 22))? + 1e-4, epsilon = 1e-12);
        Ok(())
    }

    #[test]
    fn rejects_bad_inputs() {
        let r = d(REF.0, REF.1, REF.2);
        assert!(matches!(DiscountCurve::new(r, &[], Interpolation::LogLinearDiscount), Err(CurveError::EmptyNodes)));
        let unsorted = [(d(2028, 9, 22), 0.9), (d(2027, 9, 23), 0.96)];
        assert!(matches!(DiscountCurve::new(r, &unsorted, Interpolation::LogLinearDiscount), Err(CurveError::BadNodeOrder { .. })));
        let bad_df = [(d(2027, 9, 23), 0.0)];
        assert!(matches!(DiscountCurve::new(r, &bad_df, Interpolation::LogLinearDiscount), Err(CurveError::NonPositiveDiscount { .. })));
        let c = DiscountCurve::flat(r, 0.04, d(2027, 9, 23)).expect("flat curve");
        assert!(matches!(c.discount(d(2026, 1, 1)), Err(CurveError::BeforeReference(_))));
    }
}
