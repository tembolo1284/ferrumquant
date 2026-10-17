//! Caps and floors on compounded-in-arrears SOFR.
//!
//! Each caplet pays `N τ max(R − K, 0)` at the period's payment date, where
//! `R` is the compounded SOFR over the accrual period. It is priced as a
//! Bachelier option on the period's forward compounded rate with expiry at
//! the last fixing date (accrual end − 1 fixing-calendar business day),
//! which matches QuantLib's `BachelierCapFloorEngine` on an overnight leg:
//!
//! ```text
//! caplet = N τ DF(pay) × Bachelier(F, K, σ, T_fix)
//! ```
//!
//! Periods whose last fixing is on or before the valuation date are
//! intrinsic (the rate is known or being known). Cap − floor = payer swap
//! at the strike, which the tests check against `OisSwap`.
//!
//! Vols come from a `VolSource`: one flat normal vol, or a per-caplet strip
//! (e.g. read off a swaption cube at the caplet's own expiry/tenor).

use thiserror::Error;

use fq_core::math::{BlackError, OptionType, bachelier, bachelier_implied_vol, brent, SolverConfig, SolverError};
use fq_core::time::{BusinessDayConvention, Calendar, Date, DateError, DayCount, DayCountError, Frequency, Period, ScheduleError, ScheduleSpec};

use crate::curve::{CurveError, DiscountCurve};
use crate::index::{CompoundingConvention, IndexError, OvernightIndex};
use crate::swap::{OisSwap, SwapError, SwapPeriod, SwapSide};

#[derive(Debug, Error, Clone, PartialEq)]
pub enum CapFloorError {
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
    #[error(transparent)]
    Swap(#[from] SwapError),
    #[error(transparent)]
    Black(#[from] BlackError),
    #[error(transparent)]
    Solver(#[from] SolverError),
    #[error("vol strip has {got} entries for {periods} periods")]
    StripLength { got: usize, periods: usize },
    #[error("no live caplets as of {0}")]
    Expired(Date),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CapFloorType {
    Cap,
    Floor,
}

impl CapFloorType {
    const fn option_type(self) -> OptionType {
        match self {
            Self::Cap => OptionType::Call,
            Self::Floor => OptionType::Put,
        }
    }
}

/// Normal vols for the caplets.
#[derive(Debug, Clone, PartialEq)]
pub enum VolSource {
    Flat(f64),
    /// One vol per period, in schedule order.
    Strip(Vec<f64>),
}

impl VolSource {
    fn vol(&self, i: usize, periods: usize) -> Result<f64, CapFloorError> {
        match self {
            Self::Flat(v) => Ok(*v),
            Self::Strip(vs) => {
                if vs.len() != periods {
                    return Err(CapFloorError::StripLength { got: vs.len(), periods });
                }
                Ok(vs[i])
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CapletPricing {
    pub accrual_start: Date,
    pub accrual_end: Date,
    pub payment_date: Date,
    pub fixing_date: Date,
    pub expiry_time: f64,
    pub forward: f64,
    pub vol: f64,
    pub pv: f64,
    /// dPV/dvol in currency per unit vol.
    pub vega: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CapFloorPricing {
    pub pv: f64,
    pub vega: f64,
    pub caplets: Vec<CapletPricing>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CapFloor {
    kind: CapFloorType,
    notional: f64,
    strike: f64,
    /// Reuses the swap's period rows: accrual, payment and ACT/360 fractions.
    periods: Vec<SwapPeriod>,
    fixing_calendar: Calendar,
    compounding: CompoundingConvention,
}

impl CapFloor {
    /// Caplets on the given floating-leg accrual periods.
    pub fn from_swap_periods(kind: CapFloorType, notional: f64, strike: f64, periods: &[SwapPeriod], fixing_calendar: Calendar, compounding: CompoundingConvention) -> Self {
        Self { kind, notional, strike, periods: periods.to_vec(), fixing_calendar, compounding }
    }

    /// USD SOFR cap/floor: spot start T+2, annual compounded periods, ACT/360
    /// ModifiedFollowing on USGS+USNY, 2-day payment lag (the OIS floating leg).
    pub fn usd_sofr(trade_date: Date, tenor: Period, kind: CapFloorType, notional: f64, strike: f64) -> Result<Self, CapFloorError> {
        let cal = OisSwap::usd_sofr_calendar();
        let effective = cal.advance(trade_date, 2)?;
        let schedule = ScheduleSpec::new(effective, tenor.add_to(effective)?, Frequency::Annual, cal.clone())
            .convention(BusinessDayConvention::ModifiedFollowing)
            .build()?;
        let swap = OisSwap::new(schedule, SwapSide::PayFixed, notional, strike, DayCount::Act360, 2, cal, CompoundingConvention::default())?;
        Ok(Self::from_swap_periods(kind, notional, strike, swap.periods(), OvernightIndex::sofr().fixing_calendar().clone(), CompoundingConvention::default()))
    }

    pub fn kind(&self) -> CapFloorType {
        self.kind
    }

    pub fn notional(&self) -> f64 {
        self.notional
    }

    pub fn strike(&self) -> f64 {
        self.strike
    }

    pub fn periods(&self) -> &[SwapPeriod] {
        &self.periods
    }

    /// Last fixing date of a period: one fixing-calendar business day before accrual end.
    pub fn fixing_date(&self, p: &SwapPeriod) -> Result<Date, CapFloorError> {
        Ok(self.fixing_calendar.advance(p.accrual_end, -1)?)
    }

    pub fn price(&self, index: &OvernightIndex, projection: &DiscountCurve, discount: &DiscountCurve, vols: &VolSource) -> Result<CapFloorPricing, CapFloorError> {
        let today = discount.reference_date();
        let kind = self.kind.option_type();
        let mut caplets = Vec::with_capacity(self.periods.len());
        let (mut pv, mut vega) = (0.0, 0.0);
        for (i, p) in self.periods.iter().enumerate() {
            if p.payment_date <= today {
                continue;
            }
            let fixing_date = self.fixing_date(p)?;
            let t = (f64::from(fixing_date - today) / 365.0).max(0.0);
            let forward = index.compounded_rate(p.accrual_start, p.accrual_end, Some(projection), self.compounding)?;
            let vol = vols.vol(i, self.periods.len())?;
            let df = discount.discount(p.payment_date)?;
            let scale = self.notional * p.fixed_year_fraction * df;
            let model = bachelier(kind, forward, self.strike, vol, t)?;
            let c = CapletPricing { accrual_start: p.accrual_start, accrual_end: p.accrual_end, payment_date: p.payment_date, fixing_date, expiry_time: t, forward, vol, pv: scale * model.price, vega: scale * model.vega };
            pv += c.pv;
            vega += c.vega;
            caplets.push(c);
        }
        if caplets.is_empty() {
            return Err(CapFloorError::Expired(today));
        }
        Ok(CapFloorPricing { pv, vega, caplets })
    }

    pub fn npv(&self, index: &OvernightIndex, curve: &DiscountCurve, vols: &VolSource) -> Result<f64, CapFloorError> {
        Ok(self.price(index, curve, curve, vols)?.pv)
    }

    /// Flat normal vol that reproduces `pv`.
    pub fn implied_flat_vol(&self, index: &OvernightIndex, projection: &DiscountCurve, discount: &DiscountCurve, pv: f64) -> Result<f64, CapFloorError> {
        // A single caplet has a closed-form inversion; otherwise solve.
        let live: Vec<_> = self.price(index, projection, discount, &VolSource::Flat(0.01))?.caplets;
        if live.len() == 1 {
            let c = &live[0];
            let p = self.periods.iter().find(|p| p.accrual_start == c.accrual_start).expect("period");
            let scale = self.notional * p.fixed_year_fraction * discount.discount(p.payment_date)?;
            return Ok(bachelier_implied_vol(self.kind.option_type(), c.forward, self.strike, c.expiry_time, pv / scale, 1.0)?);
        }
        let mut err = None;
        let objective = |v: f64| match self.price(index, projection, discount, &VolSource::Flat(v)) {
            Ok(r) => r.pv - pv,
            Err(e) => {
                err = Some(e);
                f64::NAN
            }
        };
        let v = brent(objective, 0.0, 1.0, SolverConfig { tolerance: 1e-12, max_iterations: 200 });
        match (v, err) {
            (_, Some(e)) => Err(e),
            (Ok(v), None) => Ok(v),
            (Err(e), None) => Err(e.into()),
        }
    }
}

#[cfg(test)]
#[allow(clippy::excessive_precision)]
mod tests {
    use super::*;
    use approx::assert_abs_diff_eq;

    type R = Result<(), Box<dyn std::error::Error>>;

    fn d(y: i32, m: u32, dd: u32) -> Date {
        Date::from_ymd(y, m, dd).expect("valid test date")
    }

    fn setup() -> (Date, OvernightIndex, DiscountCurve) {
        let today = d(2026, 10, 13);
        (today, OvernightIndex::sofr(), DiscountCurve::flat(today, 0.04, d(2040, 1, 1)).expect("curve"))
    }

    #[test]
    fn periods_and_fixing_dates_follow_the_ois_leg() -> R {
        let (today, _, _) = setup();
        let cap = CapFloor::usd_sofr(today, Period::years(2), CapFloorType::Cap, 1e6, 0.04)?;
        let p = cap.periods();
        assert_eq!(p.len(), 2);
        assert_eq!((p[0].accrual_start, p[0].accrual_end, p[0].payment_date), (d(2026, 10, 15), d(2027, 10, 15), d(2027, 10, 19)));
        assert_eq!((p[1].accrual_start, p[1].accrual_end, p[1].payment_date), (d(2027, 10, 15), d(2028, 10, 16), d(2028, 10, 18)));
        assert_eq!(cap.fixing_date(&p[0])?, d(2027, 10, 14));
        assert_eq!(cap.fixing_date(&p[1])?, d(2028, 10, 13)); // Mon 16 -> Fri 13
        Ok(())
    }

    #[test]
    fn cap_minus_floor_is_the_payer_swap() -> R {
        let (today, index, curve) = setup();
        let strike = 0.037;
        let cap = CapFloor::usd_sofr(today, Period::years(5), CapFloorType::Cap, 1e6, strike)?;
        let floor = CapFloor::usd_sofr(today, Period::years(5), CapFloorType::Floor, 1e6, strike)?;
        let vols = VolSource::Flat(0.0075);
        let diff = cap.npv(&index, &curve, &vols)? - floor.npv(&index, &curve, &vols)?;
        let swap = OisSwap::usd_sofr_spot(today, Period::years(5), SwapSide::PayFixed, 1e6, strike)?;
        assert_abs_diff_eq!(diff, swap.npv(&index, &curve)?, epsilon = 1e-6);
        Ok(())
    }

    #[test]
    fn zero_vol_is_intrinsic_and_vega_is_positive() -> R {
        let (today, index, curve) = setup();
        let cap = CapFloor::usd_sofr(today, Period::years(3), CapFloorType::Cap, 1e6, 0.03)?;
        let r0 = cap.price(&index, &curve, &curve, &VolSource::Flat(0.0))?;
        for c in &r0.caplets {
            assert!(c.forward > 0.03);
            assert_abs_diff_eq!(c.vega, 0.0, epsilon = 1e-15);
        }
        let r = cap.price(&index, &curve, &curve, &VolSource::Flat(0.008))?;
        assert!(r.pv > r0.pv);
        assert!(r.vega > 0.0);
        assert_abs_diff_eq!(r.vega, r.caplets.iter().map(|c| c.vega).sum::<f64>(), epsilon = 1e-9);
        Ok(())
    }

    #[test]
    fn strip_matches_flat_when_constant_and_implied_vol_round_trips() -> R {
        let (today, index, curve) = setup();
        let cap = CapFloor::usd_sofr(today, Period::years(4), CapFloorType::Cap, 1e6, 0.042)?;
        let flat = cap.npv(&index, &curve, &VolSource::Flat(0.0085))?;
        let strip = cap.npv(&index, &curve, &VolSource::Strip(vec![0.0085; 4]))?;
        assert_abs_diff_eq!(flat, strip, epsilon = 1e-9);
        assert_abs_diff_eq!(cap.implied_flat_vol(&index, &curve, &curve, flat)?, 0.0085, epsilon = 1e-10);
        assert!(matches!(cap.npv(&index, &curve, &VolSource::Strip(vec![0.01; 3])), Err(CapFloorError::StripLength { .. })));
        // Single-period: closed-form inversion path.
        let one = CapFloor::usd_sofr(today, Period::years(1), CapFloorType::Floor, 1e6, 0.045)?;
        let pv1 = one.npv(&index, &curve, &VolSource::Flat(0.007))?;
        assert_abs_diff_eq!(one.implied_flat_vol(&index, &curve, &curve, pv1)?, 0.007, epsilon = 1e-10);
        Ok(())
    }

    #[test]
    fn golden_quantlib_bachelier_cap_floor() -> R {
        // QuantLib 1.43: OvernightLeg(annual, ACT/360, MF, USGS, lag 2) on a
        // flat 4% cc curve, ConstantOptionletVolatility 80bp Normal,
        // BachelierCapFloorEngine, evaluation 2026-10-13.
        let (today, index, curve) = setup();
        let vols = VolSource::Flat(0.008);
        let cap = CapFloor::usd_sofr(today, Period::years(2), CapFloorType::Cap, 1e6, 0.04)?;
        let floor = CapFloor::usd_sofr(today, Period::years(2), CapFloorType::Floor, 1e6, 0.04)?;
        let r = cap.price(&index, &curve, &curve, &vols)?;
        assert_abs_diff_eq!(r.pv, 7_604.427_570_057_423, epsilon = 1e-6);
        assert_abs_diff_eq!(r.caplets[0].pv, 3_235.255_560_113_149, epsilon = 1e-6);
        assert_abs_diff_eq!(r.caplets[1].pv, 4_369.172_009_944_274, epsilon = 1e-6);
        assert_abs_diff_eq!(r.caplets[0].forward, 0.040_251_722_491, epsilon = 1e-11);
        assert_abs_diff_eq!(floor.npv(&index, &curve, &vols)?, 7_118.469_242_685_865, epsilon = 1e-6);
        let cap5 = CapFloor::usd_sofr(today, Period::years(5), CapFloorType::Cap, 1e6, 0.035)?;
        let floor5 = CapFloor::usd_sofr(today, Period::years(5), CapFloorType::Floor, 1e6, 0.035)?;
        assert_abs_diff_eq!(cap5.npv(&index, &curve, &vols)?, 37_567.951_359_340_783, epsilon = 1e-6);
        assert_abs_diff_eq!(floor5.npv(&index, &curve, &vols)?, 13_915.275_153_798_524, epsilon = 1e-6);
        Ok(())
    }
}
