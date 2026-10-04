//! Fixed-rate bullet bond, with US Treasury conventions as the first target.
//!
//! Yield uses the Street convention: compounding at the coupon frequency with
//! ACT/ACT ICMA fractional periods on unadjusted coupon dates, switching to
//! simple interest once only the final coupon period remains (QuantLib's
//! `SimpleThenCompounded` for bonds in their last period). Prices are per
//! `face` (100 for Treasuries).

use thiserror::Error;

use fq_core::math::{SolverConfig, SolverError, newton_safe};
use fq_core::time::{
    BusinessDayConvention, Calendar, Date, DateError, DayCount, DayCountError, Frequency, Market,
    RefPeriod, Schedule, ScheduleError, ScheduleSpec,
};

#[derive(Debug, Error, Clone, PartialEq)]
pub enum BondError {
    #[error(transparent)]
    Schedule(#[from] ScheduleError),
    #[error(transparent)]
    DayCount(#[from] DayCountError),
    #[error(transparent)]
    Date(#[from] DateError),
    #[error(transparent)]
    Solver(#[from] SolverError),
    #[error("settlement {settle} outside the bond's life ({start}..{maturity})")]
    SettlementOutOfRange { settle: Date, start: Date, maturity: Date },
    #[error("invalid price quote '{0}'")]
    InvalidPrice(String),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Coupon {
    pub accrual_start: Date,
    pub accrual_end: Date,
    pub payment_date: Date,
    pub ref_period: RefPeriod,
    pub amount: f64,
}

/// Yield-based price and risk measures, all per `face`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct YieldAnalytics {
    pub yield_rate: f64,
    pub clean_price: f64,
    pub dirty_price: f64,
    pub accrued: f64,
    pub macaulay_duration: f64,
    pub modified_duration: f64,
    pub convexity: f64,
    /// Dirty-price change for a 1bp fall in yield.
    pub dv01: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FixedRateBond {
    coupons: Vec<Coupon>,
    coupon_rate: f64,
    face: f64,
    frequency: Frequency,
    day_count: DayCount,
    calendar: Calendar,
    settlement_days: i32,
    redemption_date: Date,
}

impl FixedRateBond {
    pub fn new(
        schedule: &Schedule,
        coupon_rate: f64,
        face: f64,
        day_count: DayCount,
        calendar: Calendar,
        payment_convention: BusinessDayConvention,
        settlement_days: i32,
    ) -> Result<Self, BondError> {
        let coupons = (0..schedule.periods())
            .map(|i| {
                let (start, end) = schedule.period(i);
                let ref_period = schedule.ref_period(i)?;
                let yf = day_count.year_fraction(start, end, Some(ref_period))?;
                Ok(Coupon {
                    accrual_start: start,
                    accrual_end: end,
                    payment_date: calendar.adjust(end, payment_convention)?,
                    ref_period,
                    amount: face * coupon_rate * yf,
                })
            })
            .collect::<Result<Vec<_>, BondError>>()?;
        let redemption_date = coupons.last().map_or(schedule.dates()[0], |c| c.payment_date);
        Ok(Self {
            coupons,
            coupon_rate,
            face,
            frequency: schedule.frequency(),
            day_count,
            calendar,
            settlement_days,
            redemption_date,
        })
    }

    /// US Treasury note/bond: semiannual ACT/ACT ICMA on unadjusted coupon
    /// dates, payments Following on the SIFMA calendar, T+1 settlement, EOM
    /// rule when the maturity is a month end. `first_coupon` handles odd
    /// (long) first coupons; short first coupons need no extra input.
    pub fn us_treasury(
        dated_date: Date,
        maturity: Date,
        coupon_rate: f64,
        first_coupon: Option<Date>,
    ) -> Result<Self, BondError> {
        let calendar = Calendar::new(Market::UsGovernmentBond);
        let mut spec = ScheduleSpec::new(dated_date, maturity, Frequency::Semiannual, calendar.clone())
            .convention(BusinessDayConvention::Unadjusted)
            .end_of_month(maturity.is_end_of_month());
        if let Some(fc) = first_coupon {
            spec = spec.first_date(fc);
        }
        Self::new(
            &spec.build()?,
            coupon_rate,
            100.0,
            DayCount::ActActIcma,
            calendar,
            BusinessDayConvention::Following,
            1,
        )
    }

    pub fn coupons(&self) -> &[Coupon] {
        &self.coupons
    }

    pub fn coupon_rate(&self) -> f64 {
        self.coupon_rate
    }

    pub fn face(&self) -> f64 {
        self.face
    }

    pub fn maturity(&self) -> Date {
        self.coupons.last().map_or(self.redemption_date, |c| c.accrual_end)
    }

    pub fn redemption_date(&self) -> Date {
        self.redemption_date
    }

    pub fn settlement_date(&self, trade_date: Date) -> Result<Date, BondError> {
        Ok(self.calendar.advance(trade_date, self.settlement_days)?)
    }

    pub fn accrued(&self, settle: Date) -> Result<f64, BondError> {
        self.check_settle(settle)?;
        match self.coupons.iter().find(|c| c.accrual_start <= settle && settle < c.accrual_end) {
            Some(c) => {
                let yf = self.day_count.year_fraction(c.accrual_start, settle, Some(c.ref_period))?;
                Ok(self.face * self.coupon_rate * yf)
            }
            None => Ok(0.0),
        }
    }

    pub fn dirty_price(&self, yield_rate: f64, settle: Date) -> Result<f64, BondError> {
        Ok(self.price_and_derivatives(yield_rate, settle)?.0)
    }

    pub fn clean_price(&self, yield_rate: f64, settle: Date) -> Result<f64, BondError> {
        Ok(self.dirty_price(yield_rate, settle)? - self.accrued(settle)?)
    }

    pub fn yield_from_clean(&self, clean_price: f64, settle: Date) -> Result<f64, BondError> {
        let target = clean_price + self.accrued(settle)?;
        let flows = self.remaining_flows(settle)?;
        let f = f64::from(self.frequency.per_year());
        let guess = self.coupon_rate.max(1e-4);
        let y = newton_safe(
            |y| {
                let (p, dp, _, _) = street_price(&flows, y, f);
                (p - target, dp)
            },
            guess,
            -0.25 * f,
            1.0,
            SolverConfig::default(),
        )?;
        Ok(y)
    }

    pub fn analytics(&self, yield_rate: f64, settle: Date) -> Result<YieldAnalytics, BondError> {
        let (dirty, dp, d2p, time_weighted) = self.price_and_derivatives(yield_rate, settle)?;
        let accrued = self.accrued(settle)?;
        Ok(YieldAnalytics {
            yield_rate,
            clean_price: dirty - accrued,
            dirty_price: dirty,
            accrued,
            macaulay_duration: time_weighted / dirty,
            modified_duration: -dp / dirty,
            convexity: d2p / dirty,
            dv01: -dp * 1e-4,
        })
    }

    fn price_and_derivatives(&self, yield_rate: f64, settle: Date) -> Result<(f64, f64, f64, f64), BondError> {
        let flows = self.remaining_flows(settle)?;
        let f = f64::from(self.frequency.per_year());
        Ok(street_price(&flows, yield_rate, f))
    }

    fn check_settle(&self, settle: Date) -> Result<(), BondError> {
        let start = self.coupons.first().map_or(settle, |c| c.accrual_start);
        let maturity = self.maturity();
        if settle < start || settle >= maturity {
            return Err(BondError::SettlementOutOfRange { settle, start, maturity });
        }
        Ok(())
    }

    /// (time in years from settle, amount) for every flow after settlement,
    /// with redemption folded into the last coupon.
    fn remaining_flows(&self, settle: Date) -> Result<Vec<(f64, f64)>, BondError> {
        self.check_settle(settle)?;
        let mut flows = Vec::with_capacity(self.coupons.len());
        let mut t = 0.0;
        for c in self.coupons.iter().filter(|c| c.accrual_end > settle) {
            let from = if c.accrual_start > settle { c.accrual_start } else { settle };
            t += self.day_count.year_fraction(from, c.accrual_end, Some(c.ref_period))?;
            flows.push((t, c.amount));
        }
        if let Some(last) = flows.last_mut() {
            last.1 += self.face;
        }
        Ok(flows)
    }
}

/// Street-convention price with first and second yield derivatives and the
/// time-weighted PV (for Macaulay duration): (P, dP/dy, d2P/dy2, sum t*PV).
fn street_price(flows: &[(f64, f64)], y: f64, f: f64) -> (f64, f64, f64, f64) {
    if let &[(t, cf)] = flows {
        // Final coupon period: simple interest.
        let g = 1.0 + y * t;
        let pv = cf / g;
        return (pv, -cf * t / (g * g), 2.0 * cf * t * t / (g * g * g), t * pv);
    }
    let base = 1.0 + y / f;
    flows.iter().fold((0.0, 0.0, 0.0, 0.0), |(p, dp, d2p, tw), &(t, cf)| {
        let df = base.powf(-f * t);
        let pv = cf * df;
        (p + pv, dp - t * pv / base, d2p + t * (t + 1.0 / f) * pv / (base * base), tw + t * pv)
    })
}

/// Parse a Treasury price in 32nds: "99-16" = 99 16/32, "99-16+" = 99 16.5/32,
/// "99-162" = 99 16.25/32 (third digit in eighths of a 32nd). Plain decimals
/// ("99.5") are accepted too.
pub fn parse_32nds(s: &str) -> Result<f64, BondError> {
    let bad = || BondError::InvalidPrice(s.to_owned());
    let s = s.trim();
    let Some((whole, frac)) = s.split_once('-') else {
        return s.parse::<f64>().map_err(|_| bad());
    };
    let whole: u32 = whole.parse().map_err(|_| bad())?;
    let (digits, plus) = match frac.strip_suffix('+') {
        Some(d) => (d, true),
        None => (frac, false),
    };
    if !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(bad());
    }
    let (thirty_seconds, eighths) = match (digits.len(), plus) {
        (2, false) => (digits.parse::<u32>().map_err(|_| bad())?, 0),
        (2, true) => (digits.parse::<u32>().map_err(|_| bad())?, 4),
        (3, false) => (digits[..2].parse::<u32>().map_err(|_| bad())?, digits[2..].parse::<u32>().map_err(|_| bad())?),
        _ => return Err(bad()),
    };
    if thirty_seconds >= 32 || eighths >= 8 {
        return Err(bad());
    }
    Ok(f64::from(whole) + (f64::from(thirty_seconds) + f64::from(eighths) / 8.0) / 32.0)
}

/// Format a price to the nearest 1/256 in 32nds notation ("99-16+", "99-162").
pub fn format_32nds(price: f64) -> String {
    let total = (price * 256.0).round() as i64;
    let (whole, rem) = (total.div_euclid(256), total.rem_euclid(256));
    let (thirty_seconds, eighths) = (rem / 8, rem % 8);
    match eighths {
        0 => format!("{whole}-{thirty_seconds:02}"),
        4 => format!("{whole}-{thirty_seconds:02}+"),
        e => format!("{whole}-{thirty_seconds:02}{e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_abs_diff_eq;

    type R = Result<(), BondError>;

    fn d(y: i32, m: u32, dd: u32) -> Date {
        Date::from_ymd(y, m, dd).expect("valid test date")
    }

    fn ten_year(coupon: f64) -> Result<FixedRateBond, BondError> {
        FixedRateBond::us_treasury(d(2026, 8, 15), d(2036, 8, 15), coupon, None)
    }

    #[test]
    fn regular_coupons_are_half_the_rate() -> R {
        let b = ten_year(0.0425)?;
        assert_eq!(b.coupons().len(), 20);
        for c in b.coupons() {
            assert_abs_diff_eq!(c.amount, 2.125, epsilon = 1e-12);
        }
        // 2027-08-15 is a Sunday: paid Monday, accrues to the 15th.
        assert_eq!(b.coupons()[1].payment_date, d(2027, 8, 16));
        Ok(())
    }

    #[test]
    fn accrued_interest() -> R {
        let b = ten_year(0.0425)?;
        assert_abs_diff_eq!(b.accrued(d(2026, 9, 22))?, 2.125 * 38.0 / 184.0, epsilon = 1e-12);
        assert_abs_diff_eq!(b.accrued(d(2027, 2, 15))?, 0.0, epsilon = 1e-15);
        Ok(())
    }

    #[test]
    fn par_on_coupon_date() -> R {
        let b = ten_year(0.04)?;
        let settle = d(2026, 8, 15);
        assert_abs_diff_eq!(b.clean_price(0.04, settle)?, 100.0, epsilon = 1e-10);
        assert_abs_diff_eq!(b.yield_from_clean(100.0, settle)?, 0.04, epsilon = 1e-12);
        Ok(())
    }

    #[test]
    fn annuity_closed_form() -> R {
        let b = ten_year(0.04)?;
        let (y, n) = (0.05, 20);
        let v = (1.0 + y / 2.0_f64).powi(-n);
        let expected = 2.0 * (1.0 - v) / (y / 2.0) + 100.0 * v;
        assert_abs_diff_eq!(b.dirty_price(y, d(2026, 8, 15))?, expected, epsilon = 1e-10);
        Ok(())
    }

    #[test]
    fn yield_round_trip_off_coupon() -> R {
        let b = ten_year(0.0425)?;
        let settle = d(2026, 9, 22);
        let clean = b.clean_price(0.04375, settle)?;
        assert_abs_diff_eq!(b.yield_from_clean(clean, settle)?, 0.04375, epsilon = 1e-12);
        Ok(())
    }

    #[test]
    fn final_period_uses_simple_yield() -> R {
        let b = FixedRateBond::us_treasury(d(2025, 2, 15), d(2027, 2, 15), 0.04, None)?;
        let (settle, y) = (d(2026, 11, 16), 0.039);
        let t = 0.5 * 91.0 / 184.0;
        assert_abs_diff_eq!(b.dirty_price(y, settle)?, 102.0 / (1.0 + y * t), epsilon = 1e-12);
        Ok(())
    }

    #[test]
    fn short_first_coupon_amount() -> R {
        let b = FixedRateBond::us_treasury(d(2026, 10, 15), d(2031, 8, 15), 0.04, None)?;
        let first = b.coupons()[0];
        assert_eq!(first.accrual_end, d(2027, 2, 15));
        assert_abs_diff_eq!(first.amount, 100.0 * 0.04 * 0.5 * 123.0 / 184.0, epsilon = 1e-12);
        Ok(())
    }

    #[test]
    fn risk_matches_finite_differences() -> R {
        let b = ten_year(0.0425)?;
        let (settle, y, h) = (d(2026, 9, 22), 0.043, 1e-6);
        let a = b.analytics(y, settle)?;
        let (up, dn) = (b.dirty_price(y + h, settle)?, b.dirty_price(y - h, settle)?);
        assert_abs_diff_eq!(a.modified_duration, (dn - up) / (2.0 * h * a.dirty_price), epsilon = 1e-6);
        assert_abs_diff_eq!(a.convexity, (up + dn - 2.0 * a.dirty_price) / (h * h * a.dirty_price), epsilon = 1e-2);
        assert_abs_diff_eq!(a.dv01, a.modified_duration * a.dirty_price * 1e-4, epsilon = 1e-12);
        assert_abs_diff_eq!(a.macaulay_duration, a.modified_duration * (1.0 + y / 2.0), epsilon = 1e-10);
        Ok(())
    }

    #[test]
    fn settlement_bounds() -> R {
        let b = ten_year(0.04)?;
        assert!(matches!(b.accrued(d(2026, 8, 14)), Err(BondError::SettlementOutOfRange { .. })));
        assert_eq!(b.settlement_date(d(2026, 4, 2))?, d(2026, 4, 6)); // T+1 over Good Friday
        Ok(())
    }

    #[test]
    fn thirty_seconds() -> R {
        assert_abs_diff_eq!(parse_32nds("99-16")?, 99.5, epsilon = 1e-15);
        assert_abs_diff_eq!(parse_32nds("99-16+")?, 99.515_625, epsilon = 1e-15);
        assert_abs_diff_eq!(parse_32nds("99-162")?, 99.507_812_5, epsilon = 1e-15);
        assert_abs_diff_eq!(parse_32nds("101.25")?, 101.25, epsilon = 1e-15);
        assert!(parse_32nds("99-32").is_err());
        assert!(parse_32nds("99-1").is_err());
        assert_eq!(format_32nds(99.515_625), "99-16+");
        assert_eq!(format_32nds(99.507_812_5), "99-162");
        assert_eq!(format_32nds(100.0), "100-00");
        Ok(())
    }
}
