//! Calendar spreads: storable versus non-storable.
//!
//! For a storable commodity the spread between two delivery months is
//! bounded above by the cost of carrying inventory from one to the other
//! (storage plus interest, less any convenience yield), so a spread is
//! read as a *percentage of full carry*: near full carry means ample
//! supply, a steep inversion means scarcity. Livestock cannot be stored,
//! so its spreads are forecasts of seasonal supply with no arbitrage
//! bound; this module refuses to compute a carry benchmark for them.
//!
//! A calendar spread option on either kind is a two-asset spread on one
//! curve, priced with the core's spread pricers. For storables the
//! correlation between adjacent months is high; for livestock it is
//! lower and must be supplied, which the API makes explicit.

use fq_commod_core::spread::spread_price;
use fq_commod_core::{ContractMonth, FuturesCurve, SeasonalVol, SpreadMethod};
use fq_core::math::OptionType;
use fq_core::time::DayCount;
use fq_rates::curve::DiscountCurve;

use crate::contract::{AgCalendar, AgContract, expiry, option_expiry};
use crate::error::AgsError;
use crate::option::check_curve;

/// Full-carry benchmark between two months of a storable, in quotation
/// units per physical unit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CarryBenchmark {
    pub near: f64,
    pub far: f64,
    /// `far - near` in quotation units.
    pub spread: f64,
    pub days: i32,
    /// Storage over the period, quotation units.
    pub storage: f64,
    /// Interest on the near price over the period, quotation units.
    pub interest: f64,
    /// `storage + interest`.
    pub full_carry: f64,
    /// `spread / full_carry`; above 1 is over full carry (rare), negative
    /// is an inverted market.
    pub percent_of_full_carry: f64,
}

/// `storage_per_unit_day` is in quotation units per physical unit per
/// calendar day (e.g. cents/bu/day; CBOT delivery storage is around
/// 0.165 c/bu/day for corn). Interest comes from the discount curve
/// between the two expiries.
pub fn full_carry(
    contract: AgContract,
    curve: &FuturesCurve,
    near: ContractMonth,
    far: ContractMonth,
    storage_per_unit_day: f64,
    discount: &DiscountCurve,
    cal: &AgCalendar,
) -> Result<CarryBenchmark, AgsError> {
    check_curve(contract, curve)?;
    if !contract.is_storable() {
        return Err(AgsError::Invalid(format!("{} is not storable; carry has no meaning", contract.code())));
    }
    if far <= near {
        return Err(AgsError::Invalid(format!("far month {far} must be after near month {near}")));
    }
    if !(storage_per_unit_day.is_finite() && storage_per_unit_day >= 0.0) {
        return Err(AgsError::Invalid(format!("storage {storage_per_unit_day} must be non-negative")));
    }
    let f_near = curve.price(near)?;
    let f_far = curve.price(far)?;
    let e_near = expiry(contract, near, cal)?;
    let e_far = expiry(contract, far, cal)?;
    let days = e_far - e_near;
    let storage = storage_per_unit_day * f64::from(days);
    let interest = f_near * (discount.discount(e_near)? / discount.discount(e_far)? - 1.0);
    let fc = storage + interest;
    let spread = f_far - f_near;
    Ok(CarryBenchmark {
        near: f_near,
        far: f_far,
        spread,
        days,
        storage,
        interest,
        full_carry: fc,
        percent_of_full_carry: if fc > 0.0 { spread / fc } else { f64::NAN },
    })
}

/// `max(sign (F_near - F_far - K), 0)` at the near month's option expiry.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AgCalendarSpread {
    pub kind: OptionType,
    pub contract: AgContract,
    pub near: ContractMonth,
    pub far: ContractMonth,
    /// Quotation units; non-negative.
    pub strike: f64,
    pub lots: f64,
    pub method: SpreadMethod,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AgCalendarSpreadValuation {
    pub near_forward: f64,
    pub far_forward: f64,
    pub forward_spread: f64,
    pub time_to_expiry: f64,
    pub discount: f64,
    /// Quotation units, undiscounted.
    pub forward_price: f64,
    pub pv: f64,
}

impl AgCalendarSpread {
    pub fn new(kind: OptionType, contract: AgContract, near: ContractMonth, far: ContractMonth, strike: f64, lots: f64) -> Result<Self, AgsError> {
        for m in [near, far] {
            if !contract.is_listed(m) {
                return Err(AgsError::NotListed { contract: contract.code(), month: m });
            }
        }
        if far <= near {
            return Err(AgsError::Invalid(format!("far month {far} must be after near month {near}")));
        }
        if !strike.is_finite() || strike < 0.0 {
            return Err(AgsError::Invalid(format!("strike {strike} must be non-negative")));
        }
        if !(lots.is_finite() && lots > 0.0) {
            return Err(AgsError::Invalid(format!("lots {lots} must be positive")));
        }
        Ok(Self { kind, contract, near, far, strike, lots, method: SpreadMethod::default() })
    }

    pub fn with_method(mut self, method: SpreadMethod) -> Self {
        self.method = method;
        self
    }

    /// `rho` is the correlation between the two months' returns.
    pub fn value(
        &self,
        curve: &FuturesCurve,
        vols: &SeasonalVol,
        rho: f64,
        discount: &DiscountCurve,
        cal: &AgCalendar,
        dc: DayCount,
    ) -> Result<AgCalendarSpreadValuation, AgsError> {
        check_curve(self.contract, curve)?;
        let valuation = curve.reference_date();
        let exp = option_expiry(self.contract, self.near, cal)?;
        if exp <= valuation {
            return Err(AgsError::Expired { expiry: exp, valuation });
        }
        let t = dc.year_fraction(valuation, exp, None)?;
        let f_near = curve.price(self.near)?;
        let f_far = curve.price(self.far)?;
        let df = discount.discount(exp)?;
        let per_unit = spread_price(self.method, self.kind, f_near, f_far, vols.vol(self.near), vols.vol(self.far), rho, t, self.strike)?;
        let point_value = self.contract.lot_value(1.0) * self.lots;
        Ok(AgCalendarSpreadValuation {
            near_forward: f_near,
            far_forward: f_far,
            forward_spread: f_near - f_far,
            time_to_expiry: t,
            discount: df,
            forward_price: per_unit,
            pv: per_unit * point_value * df,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::option::tests::{cm, curve, disc, ymd};
    use approx::assert_abs_diff_eq;

    #[test]
    fn corn_carry_benchmark() {
        let val = ymd(2026, 11, 9);
        let cal = AgCalendar::new();
        let d = disc(val);
        // Dec-26 440, Mar-27 452: a 12 cent spread.
        let corn = curve(AgContract::Corn, val, &[(cm(2026, 12), 440.0), (cm(2027, 3), 452.0), (cm(2027, 5), 458.0)]);
        let b = full_carry(AgContract::Corn, &corn, cm(2026, 12), cm(2027, 3), 0.165, &d, &cal).expect("carry");
        // Dec 14 2026 to Mar 12 2027 = 88 days.
        assert_eq!(b.days, 88);
        assert_abs_diff_eq!(b.spread, 12.0, epsilon = 1e-12);
        assert_abs_diff_eq!(b.storage, 0.165 * 88.0, epsilon = 1e-12);
        // ~4% for 88 days on 440c is about 4.2c.
        assert!(b.interest > 3.5 && b.interest < 5.0, "interest {}", b.interest);
        assert_abs_diff_eq!(b.full_carry, b.storage + b.interest, epsilon = 1e-12);
        assert!(b.percent_of_full_carry > 0.5 && b.percent_of_full_carry < 0.75, "pct {}", b.percent_of_full_carry);
        // Livestock has no carry.
        let hogs = curve(AgContract::LeanHogs, val, &[(cm(2027, 2), 85.0), (cm(2027, 4), 90.0)]);
        assert!(full_carry(AgContract::LeanHogs, &hogs, cm(2027, 2), cm(2027, 4), 0.0, &d, &cal).is_err());
    }

    #[test]
    fn spread_options_storable_and_not() {
        let val = ymd(2026, 11, 9);
        let cal = AgCalendar::new();
        let d = disc(val);
        let corn = curve(AgContract::Corn, val, &[(cm(2026, 12), 440.0), (cm(2027, 3), 452.0), (cm(2027, 5), 458.0)]);
        let vols = SeasonalVol::flat(cm(2027, 3), 0.22).expect("v");
        // Put on Mar/May (near - far = -6): pays if the spread widens further.
        let cso = AgCalendarSpread::new(OptionType::Put, AgContract::Corn, cm(2027, 3), cm(2027, 5), 0.0, 10.0).expect("cso");
        let v = cso.value(&corn, &vols, 0.97, &d, &cal, DayCount::Act365Fixed).expect("v");
        assert_abs_diff_eq!(v.forward_spread, -6.0, epsilon = 1e-12);
        assert!(v.forward_price > 6.0);
        assert_abs_diff_eq!(v.time_to_expiry, DayCount::Act365Fixed.year_fraction(val, ymd(2027, 2, 19), None).expect("t"), epsilon = 1e-12);
        assert_abs_diff_eq!(v.pv, v.forward_price * 500.0 * v.discount, epsilon = 1e-9);
        // Lower correlation (livestock-like) makes the spread option dearer.
        let hogs = curve(AgContract::LeanHogs, val, &[(cm(2027, 2), 85.0), (cm(2027, 4), 90.0), (cm(2027, 6), 98.0)]);
        let hv = SeasonalVol::flat(cm(2027, 2), 0.20).expect("v");
        let hcso = AgCalendarSpread::new(OptionType::Call, AgContract::LeanHogs, cm(2027, 4), cm(2027, 6), 0.0, 1.0).expect("cso");
        let hi = hcso.value(&hogs, &hv, 0.95, &d, &cal, DayCount::Act365Fixed).expect("v");
        let lo = hcso.value(&hogs, &hv, 0.60, &d, &cal, DayCount::Act365Fixed).expect("v");
        assert!(lo.forward_price > hi.forward_price);
        assert!(AgCalendarSpread::new(OptionType::Call, AgContract::Corn, cm(2027, 5), cm(2027, 3), 0.0, 1.0).is_err());
        assert!(matches!(AgCalendarSpread::new(OptionType::Call, AgContract::Corn, cm(2027, 4), cm(2027, 5), 0.0, 1.0), Err(AgsError::NotListed { .. })));
    }
}
