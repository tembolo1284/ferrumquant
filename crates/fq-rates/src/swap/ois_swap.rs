//! Overnight index swap, with USD SOFR as the built-in convention set:
//! spot start T+2, annual fixed and annual compounded-SOFR legs, both
//! ACT/360 ModifiedFollowing on the USGS+USNY calendar, 2-business-day
//! payment lag. Accrual runs on adjusted dates (unlike bonds).
//!
//! Pricing takes separate projection and discount curves so the same code
//! works when discounting moves off the projection curve; for USD SOFR they
//! are the same curve.

use std::fmt;
use std::str::FromStr;

use thiserror::Error;

use fq_core::time::{
    BusinessDayConvention, Calendar, Date, DateError, DayCount, DayCountError, Frequency, JoinRule,
    Market, Schedule, ScheduleError, ScheduleSpec,
};

use crate::curve::{CurveError, DiscountCurve};
use crate::index::{CompoundingConvention, IndexError, OvernightIndex};

#[derive(Debug, Error, Clone, PartialEq)]
pub enum SwapError {
    #[error(transparent)]
    Date(#[from] DateError),
    #[error(transparent)]
    DayCount(#[from] DayCountError),
    #[error(transparent)]
    Schedule(#[from] ScheduleError),
    #[error(transparent)]
    Curve(#[from] CurveError),
    #[error(transparent)]
    Index(#[from] IndexError),
    #[error("invalid tenor '{0}' (expected e.g. 6M, 18M, 5Y)")]
    InvalidTenor(String),
    #[error("swap has no unpaid cashflows as of {0}")]
    Expired(Date),
}

/// A month-based tenor (weeks/days come later with the deposit instruments).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Tenor {
    months: i32,
}

impl Tenor {
    pub const fn months(months: i32) -> Self {
        Self { months }
    }

    pub const fn years(years: i32) -> Self {
        Self { months: years * 12 }
    }

    pub const fn in_months(self) -> i32 {
        self.months
    }

    pub fn add_to(self, d: Date) -> Result<Date, DateError> {
        d.add_months(self.months)
    }
}

impl fmt::Display for Tenor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.months % 12 == 0 { write!(f, "{}Y", self.months / 12) } else { write!(f, "{}M", self.months) }
    }
}

impl FromStr for Tenor {
    type Err = SwapError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let t = s.trim().to_ascii_uppercase();
        let bad = || SwapError::InvalidTenor(s.to_owned());
        let (num, unit) = t.split_at(t.len().checked_sub(1).ok_or_else(bad)?);
        let n: i32 = num.parse().map_err(|_| bad())?;
        match unit {
            "Y" => Ok(Self::years(n)),
            "M" => Ok(Self::months(n)),
            _ => Err(bad()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SwapSide {
    PayFixed,
    ReceiveFixed,
}

impl SwapSide {
    /// +1 when the holder receives the floating leg.
    fn sign(self) -> f64 {
        match self {
            Self::PayFixed => 1.0,
            Self::ReceiveFixed => -1.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SwapCashflow {
    pub accrual_start: Date,
    pub accrual_end: Date,
    pub payment_date: Date,
    /// Fixed rate, or the compounded floating rate for the period.
    pub rate: f64,
    pub amount: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SwapValuation {
    /// From the holder's side: receive-float minus pay-fixed for a payer.
    pub npv: f64,
    pub fixed_leg_pv: f64,
    pub float_leg_pv: f64,
    /// PV of 1 unit of fixed rate on the notional (the swap's PV01 base).
    pub annuity: f64,
    pub par_rate: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OisSwap {
    side: SwapSide,
    notional: f64,
    fixed_rate: f64,
    schedule: Schedule,
    fixed_day_count: DayCount,
    payment_lag: i32,
    payment_calendar: Calendar,
    compounding: CompoundingConvention,
}

impl OisSwap {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        schedule: Schedule,
        side: SwapSide,
        notional: f64,
        fixed_rate: f64,
        fixed_day_count: DayCount,
        payment_lag: i32,
        payment_calendar: Calendar,
        compounding: CompoundingConvention,
    ) -> Self {
        Self { side, notional, fixed_rate, schedule, fixed_day_count, payment_lag, payment_calendar, compounding }
    }

    pub fn usd_sofr_calendar() -> Calendar {
        Calendar::joint([Market::UsGovernmentBond, Market::UsSettlement], JoinRule::Holidays)
    }

    /// USD SOFR OIS between explicit (unadjusted) effective and maturity dates.
    pub fn usd_sofr(effective: Date, maturity: Date, side: SwapSide, notional: f64, fixed_rate: f64) -> Result<Self, SwapError> {
        let cal = Self::usd_sofr_calendar();
        let schedule = ScheduleSpec::new(effective, maturity, Frequency::Annual, cal.clone())
            .convention(BusinessDayConvention::ModifiedFollowing)
            .build()?;
        Ok(Self::new(schedule, side, notional, fixed_rate, DayCount::Act360, 2, cal, CompoundingConvention::default()))
    }

    /// Spot-starting (T+2) USD SOFR OIS for a tenor, as quoted on screens.
    pub fn usd_sofr_spot(trade_date: Date, tenor: Tenor, side: SwapSide, notional: f64, fixed_rate: f64) -> Result<Self, SwapError> {
        let cal = Self::usd_sofr_calendar();
        let effective = cal.advance(trade_date, 2)?;
        Self::usd_sofr(effective, tenor.add_to(effective)?, side, notional, fixed_rate)
    }

    pub fn side(&self) -> SwapSide {
        self.side
    }

    pub fn notional(&self) -> f64 {
        self.notional
    }

    pub fn fixed_rate(&self) -> f64 {
        self.fixed_rate
    }

    pub fn schedule(&self) -> &Schedule {
        &self.schedule
    }

    pub fn effective_date(&self) -> Date {
        self.schedule.adjusted_dates()[0]
    }

    pub fn maturity_date(&self) -> Date {
        *self.schedule.adjusted_dates().last().expect("schedule has at least two dates")
    }

    /// Adjusted (start, end, payment) triples for both legs.
    pub fn periods(&self) -> Result<Vec<(Date, Date, Date)>, SwapError> {
        self.schedule
            .adjusted_dates()
            .windows(2)
            .map(|w| Ok((w[0], w[1], self.payment_calendar.advance(w[1], self.payment_lag)?)))
            .collect()
    }

    pub fn fixed_cashflows(&self) -> Result<Vec<SwapCashflow>, SwapError> {
        self.periods()?
            .into_iter()
            .map(|(s, e, p)| {
                let yf = self.fixed_day_count.year_fraction(s, e, None)?;
                Ok(SwapCashflow { accrual_start: s, accrual_end: e, payment_date: p, rate: self.fixed_rate, amount: self.notional * self.fixed_rate * yf })
            })
            .collect()
    }

    /// Floating cashflows using stored fixings where available and the
    /// projection curve for the rest.
    pub fn float_cashflows(&self, index: &OvernightIndex, projection: &DiscountCurve) -> Result<Vec<SwapCashflow>, SwapError> {
        self.periods()?
            .into_iter()
            .map(|(s, e, p)| {
                let factor = index.compounded_factor(s, e, Some(projection), self.compounding)?;
                let rate = index.compounded_rate(s, e, Some(projection), self.compounding)?;
                Ok(SwapCashflow { accrual_start: s, accrual_end: e, payment_date: p, rate, amount: self.notional * (factor - 1.0) })
            })
            .collect()
    }

    /// Full valuation; cashflows paying on or before the discount curve's
    /// reference date are treated as settled.
    pub fn value(&self, index: &OvernightIndex, projection: &DiscountCurve, discount: &DiscountCurve) -> Result<SwapValuation, SwapError> {
        let today = discount.reference_date();
        let live = |cf: &SwapCashflow| cf.payment_date > today;

        let mut fixed_leg_pv = 0.0;
        let mut annuity = 0.0;
        for cf in self.fixed_cashflows()?.iter().filter(|cf| live(cf)) {
            let df = discount.discount(cf.payment_date)?;
            let yf = self.fixed_day_count.year_fraction(cf.accrual_start, cf.accrual_end, None)?;
            fixed_leg_pv += cf.amount * df;
            annuity += self.notional * yf * df;
        }
        if annuity == 0.0 {
            return Err(SwapError::Expired(today));
        }

        let mut float_leg_pv = 0.0;
        for cf in self.float_cashflows(index, projection)?.iter().filter(|cf| live(cf)) {
            float_leg_pv += cf.amount * discount.discount(cf.payment_date)?;
        }

        Ok(SwapValuation {
            npv: self.side.sign() * (float_leg_pv - fixed_leg_pv),
            fixed_leg_pv,
            float_leg_pv,
            annuity,
            par_rate: float_leg_pv / annuity,
        })
    }

    /// Single-curve NPV (projection = discount).
    pub fn npv(&self, index: &OvernightIndex, curve: &DiscountCurve) -> Result<f64, SwapError> {
        Ok(self.value(index, curve, curve)?.npv)
    }

    /// Single-curve parallel DV01: NPV change for a +1bp zero-curve shift
    /// (central difference).
    pub fn dv01(&self, index: &OvernightIndex, curve: &DiscountCurve) -> Result<f64, SwapError> {
        let up = self.npv(index, &curve.shifted(1e-4))?;
        let down = self.npv(index, &curve.shifted(-1e-4))?;
        Ok(0.5 * (up - down))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_abs_diff_eq;

    type R = Result<(), SwapError>;

    fn d(y: i32, m: u32, dd: u32) -> Date {
        Date::from_ymd(y, m, dd).expect("valid test date")
    }

    const TODAY: (i32, u32, u32) = (2026, 9, 23);

    fn setup() -> (Date, OvernightIndex, DiscountCurve) {
        let today = d(TODAY.0, TODAY.1, TODAY.2);
        let curve = DiscountCurve::flat(today, 0.04, d(2056, 9, 23)).expect("flat curve");
        (today, OvernightIndex::sofr(), curve)
    }

    #[test]
    fn usd_sofr_dates() -> R {
        let (today, _, _) = setup();
        let s = OisSwap::usd_sofr_spot(today, Tenor::years(5), SwapSide::PayFixed, 1e6, 0.04)?;
        assert_eq!(s.effective_date(), d(2026, 9, 25));
        assert_eq!(s.maturity_date(), d(2031, 9, 25));
        let periods = s.periods()?;
        assert_eq!(periods.len(), 5);
        // 2027-09-25 is a Saturday: accrual end rolls to Mon 27, payment T+2 = Wed 29.
        assert_eq!(periods[0], (d(2026, 9, 25), d(2027, 9, 27), d(2027, 9, 29)));
        assert_eq!(periods[1].0, d(2027, 9, 27));
        Ok(())
    }

    #[test]
    fn float_leg_telescopes_without_payment_lag() -> R {
        let (_, index, curve) = setup();
        let cal = OisSwap::usd_sofr_calendar();
        let schedule = ScheduleSpec::new(d(2026, 9, 25), d(2029, 9, 25), Frequency::Annual, cal.clone()).build()?;
        let s = OisSwap::new(schedule, SwapSide::PayFixed, 100.0, 0.04, DayCount::Act360, 0, cal, CompoundingConvention::default());
        let v = s.value(&index, &curve, &curve)?;
        let expected = 100.0 * (curve.discount(s.effective_date())? - curve.discount(s.maturity_date())?);
        assert_abs_diff_eq!(v.float_leg_pv, expected, epsilon = 1e-9);
        Ok(())
    }

    #[test]
    fn par_swap_has_zero_npv_and_sides_mirror() -> R {
        let (today, index, curve) = setup();
        let probe = OisSwap::usd_sofr_spot(today, Tenor::years(10), SwapSide::PayFixed, 1e6, 0.0)?;
        let par = probe.value(&index, &curve, &curve)?.par_rate;
        assert!(par > 0.04 && par < 0.042, "ACT/360 par on a 4% cc curve, got {par}");

        let payer = OisSwap::usd_sofr_spot(today, Tenor::years(10), SwapSide::PayFixed, 1e6, par)?;
        assert_abs_diff_eq!(payer.npv(&index, &curve)?, 0.0, epsilon = 1e-6);
        let off = OisSwap::usd_sofr_spot(today, Tenor::years(10), SwapSide::PayFixed, 1e6, par + 0.001)?;
        let off_rx = OisSwap::usd_sofr_spot(today, Tenor::years(10), SwapSide::ReceiveFixed, 1e6, par + 0.001)?;
        assert_abs_diff_eq!(off.npv(&index, &curve)?, -off_rx.npv(&index, &curve)?, epsilon = 1e-9);
        assert!(off.npv(&index, &curve)? < 0.0);
        Ok(())
    }

    #[test]
    fn payer_dv01_is_positive_and_near_annuity() -> R {
        let (today, index, curve) = setup();
        let s = OisSwap::usd_sofr_spot(today, Tenor::years(5), SwapSide::PayFixed, 1e6, 0.04)?;
        let dv01 = s.dv01(&index, &curve)?;
        let annuity_bp = s.value(&index, &curve, &curve)?.annuity * 1e-4;
        assert!(dv01 > 0.0);
        assert_abs_diff_eq!(dv01, annuity_bp, epsilon = 0.05 * annuity_bp);
        Ok(())
    }

    #[test]
    fn seasoned_swap_blends_fixings_and_forwards() -> R {
        let (today, mut index, curve) = setup();
        // Started a year ago: the first period is almost fully fixed at 4.5%
        // and pays 2026-09-29, still after today, so it is live.
        let s = OisSwap::usd_sofr(d(2025, 9, 25), d(2027, 9, 25), SwapSide::PayFixed, 1e6, 0.04)?;
        for day in d(2025, 9, 25).serial()..today.serial() {
            index.add_fixing(Date::from_serial(day)?, 0.045);
        }
        let flows = s.float_cashflows(&index, &curve)?;
        assert!(s.periods()?[0].2 > today);
        assert!(flows[0].rate > 0.0455 && flows[0].rate < 0.0465, "got {}", flows[0].rate); // daily-compounded 4.5%
        assert!(flows[1].rate > 0.04 && flows[1].rate < 0.0405, "got {}", flows[1].rate);
        // Once the first payment has settled it drops out of the valuation.
        let later = DiscountCurve::flat(d(2026, 10, 1), 0.04, d(2056, 9, 23))?;
        for day in today.serial()..d(2026, 10, 1).serial() {
            index.add_fixing(Date::from_serial(day)?, 0.045);
        }
        let v = s.value(&index, &later, &later)?;
        let live_fixed: f64 = s.fixed_cashflows()?.iter().filter(|c| c.payment_date > later.reference_date()).map(|c| c.amount).sum();
        assert!(v.fixed_leg_pv < live_fixed && v.fixed_leg_pv > 0.95 * live_fixed);
        Ok(())
    }

    #[test]
    fn tenor_parsing() -> R {
        assert_eq!("5y".parse::<Tenor>()?, Tenor::years(5));
        assert_eq!("18M".parse::<Tenor>()?, Tenor::months(18));
        assert_eq!(Tenor::months(24).to_string(), "2Y");
        assert_eq!(Tenor::months(6).to_string(), "6M");
        assert!("1W".parse::<Tenor>().is_err());
        assert!("".parse::<Tenor>().is_err());
        Ok(())
    }
}
