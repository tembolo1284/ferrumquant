//! Calendar spread options and the contango carry trade.
//!
//! The front-to-back shape of the crude curve is itself a traded market:
//! calendar spread options (CSOs) pay on `F_near - F_far` at the near
//! contract's expiry, and when the curve is in steep contango a trader can
//! buy prompt barrels, store them (tank or tanker) and sell the deferred
//! contract, locking in `F_far - F_near` less storage and financing. The
//! CSO is a two-asset spread on one curve, so its key input is the
//! correlation between the two contract months, which [`ClewlowStrickland`]
//! supplies consistently with its term structure of vol.

use fq_commod_core::spread::spread_price;
use fq_commod_core::{ClewlowStrickland, ContractMonth, FuturesCurve, SeasonalVol, SpreadMethod};
use fq_core::math::OptionType;
use fq_core::time::DayCount;
use fq_rates::curve::DiscountCurve;

use crate::contract::{OilContract, OilMarket, expiry};
use crate::error::OilError;
use crate::swap::{check_curve, payment_date};

/// `max(sign (F_near - F_far - K), 0)` at the near contract's expiry.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CalendarSpreadOption {
    pub kind: OptionType,
    pub contract: OilContract,
    pub near: ContractMonth,
    pub far: ContractMonth,
    /// $/unit; may be zero.
    pub strike: f64,
    pub quantity: f64,
    pub method: SpreadMethod,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CalendarSpreadValuation {
    pub near_forward: f64,
    pub far_forward: f64,
    pub forward_spread: f64,
    pub correlation: f64,
    pub time_to_expiry: f64,
    pub discount: f64,
    pub forward_price: f64,
    pub pv: f64,
}

impl CalendarSpreadOption {
    pub fn new(kind: OptionType, contract: OilContract, near: ContractMonth, far: ContractMonth, strike: f64, quantity: f64) -> Result<Self, OilError> {
        if far <= near {
            return Err(OilError::Invalid(format!("far month {far} must be after near month {near}")));
        }
        if !strike.is_finite() || strike < 0.0 {
            return Err(OilError::Invalid(format!("strike {strike} must be non-negative")));
        }
        if !(quantity.is_finite() && quantity > 0.0) {
            return Err(OilError::Invalid(format!("quantity {quantity} must be positive")));
        }
        Ok(Self { kind, contract, near, far, strike, quantity, method: SpreadMethod::default() })
    }

    pub fn with_method(mut self, method: SpreadMethod) -> Self {
        self.method = method;
        self
    }

    /// Price with an explicit inter-month correlation.
    pub fn value_with_correlation(
        &self,
        curve: &FuturesCurve,
        vols: &SeasonalVol,
        rho: f64,
        discount: &DiscountCurve,
        market: &OilMarket,
        dc: DayCount,
    ) -> Result<CalendarSpreadValuation, OilError> {
        check_curve(self.contract, curve)?;
        let valuation = discount.reference_date();
        let exp = expiry(self.contract, self.near, market)?;
        if exp <= valuation {
            return Err(OilError::Expired { month: self.near, valuation });
        }
        let t = dc.year_fraction(valuation, exp, None)?;
        let f_near = curve.price(self.near)?;
        let f_far = curve.price(self.far)?;
        let df = discount.discount(payment_date(self.contract, self.near, market)?)?;
        let per_unit = spread_price(self.method, self.kind, f_near, f_far, vols.vol(self.near), vols.vol(self.far), rho, t, self.strike)?;
        Ok(CalendarSpreadValuation {
            near_forward: f_near,
            far_forward: f_far,
            forward_spread: f_near - f_far,
            correlation: rho,
            time_to_expiry: t,
            discount: df,
            forward_price: per_unit,
            pv: per_unit * self.quantity * df,
        })
    }

    /// Price with vols and correlation implied by a Clewlow-Strickland
    /// model: the Black vol of each leg to the option expiry and the model
    /// correlation between the two contracts over that horizon.
    pub fn value_with_model(
        &self,
        curve: &FuturesCurve,
        model: &ClewlowStrickland,
        discount: &DiscountCurve,
        market: &OilMarket,
        dc: DayCount,
    ) -> Result<CalendarSpreadValuation, OilError> {
        check_curve(self.contract, curve)?;
        let valuation = discount.reference_date();
        let exp = expiry(self.contract, self.near, market)?;
        if exp <= valuation {
            return Err(OilError::Expired { month: self.near, valuation });
        }
        let t = dc.year_fraction(valuation, exp, None)?;
        let t_far = dc.year_fraction(valuation, expiry(self.contract, self.far, market)?, None)?;
        let s_near = model.black_vol(t, t)?;
        let s_far = model.black_vol(t, t_far)?;
        let rho = model.correlation(t, t, t_far)?;
        let f_near = curve.price(self.near)?;
        let f_far = curve.price(self.far)?;
        let df = discount.discount(payment_date(self.contract, self.near, market)?)?;
        let per_unit = spread_price(self.method, self.kind, f_near, f_far, s_near, s_far, rho, t, self.strike)?;
        Ok(CalendarSpreadValuation {
            near_forward: f_near,
            far_forward: f_far,
            forward_spread: f_near - f_far,
            correlation: rho,
            time_to_expiry: t,
            discount: df,
            forward_price: per_unit,
            pv: per_unit * self.quantity * df,
        })
    }
}

/// Economics of buying `near`, storing, and selling `far`, per unit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CarryResult {
    pub near_forward: f64,
    pub far_forward: f64,
    /// `far - near`; positive is contango.
    pub spread: f64,
    pub months: i32,
    pub storage_cost: f64,
    /// Interest on the purchase price over the holding period.
    pub financing_cost: f64,
    /// `spread - storage - financing`.
    pub net_carry: f64,
    /// Spread at which the trade breaks even.
    pub breakeven_spread: f64,
}

/// `storage_per_unit_month` is the all-in storage fee per unit per month;
/// financing uses the discount curve between the two contracts' expiries
/// as a simple interest cost on the near price.
pub fn contango_carry(
    contract: OilContract,
    curve: &FuturesCurve,
    near: ContractMonth,
    far: ContractMonth,
    storage_per_unit_month: f64,
    discount: &DiscountCurve,
    market: &OilMarket,
) -> Result<CarryResult, OilError> {
    check_curve(contract, curve)?;
    if far <= near {
        return Err(OilError::Invalid(format!("far month {far} must be after near month {near}")));
    }
    if !(storage_per_unit_month.is_finite() && storage_per_unit_month >= 0.0) {
        return Err(OilError::Invalid(format!("storage cost {storage_per_unit_month} must be non-negative")));
    }
    let f_near = curve.price(near)?;
    let f_far = curve.price(far)?;
    let months = near.months_until(far);
    let storage = storage_per_unit_month * f64::from(months);
    let d_near = discount.discount(expiry(contract, near, market)?)?;
    let d_far = discount.discount(expiry(contract, far, market)?)?;
    let financing = f_near * (d_near / d_far - 1.0);
    let spread = f_far - f_near;
    Ok(CarryResult {
        near_forward: f_near,
        far_forward: f_far,
        spread,
        months,
        storage_cost: storage,
        financing_cost: financing,
        net_carry: spread - storage - financing,
        breakeven_spread: storage + financing,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::swap::tests::{cm, disc, wti_curve, ymd};
    use approx::assert_abs_diff_eq;
    use fq_commod_core::{CurveInterpolation, FuturesQuote};

    #[test]
    fn cso_with_explicit_and_model_correlation() {
        let val = ymd(2026, 11, 9);
        let mkt = OilMarket::new();
        let c = wti_curve(val);
        let d = disc(val);
        let vols = SeasonalVol::flat(cm(2027, 1), 0.35).expect("v");
        let cso = CalendarSpreadOption::new(OptionType::Call, OilContract::Wti, cm(2027, 2), cm(2027, 6), 0.0, 1_000.0).expect("cso");
        let v = cso.value_with_correlation(&c, &vols, 0.95, &d, &mkt, DayCount::Act365Fixed).expect("v");
        // Backwardated curve: 70 - 67.6 = 2.40.
        assert_abs_diff_eq!(v.forward_spread, 2.40, epsilon = 1e-12);
        assert!(v.forward_price > 2.40);
        let t = DayCount::Act365Fixed.year_fraction(val, ymd(2027, 1, 20), None).expect("t");
        assert_abs_diff_eq!(v.time_to_expiry, t, epsilon = 1e-12);
        // Lower correlation -> more valuable spread option.
        let lo = cso.value_with_correlation(&c, &vols, 0.70, &d, &mkt, DayCount::Act365Fixed).expect("v");
        assert!(lo.forward_price > v.forward_price);
        // Model route: two-factor gives rho < 1 and a near vol above the far vol.
        let model = ClewlowStrickland::two_factor(0.45, 1.5, 0.20, 0.3).expect("m");
        let vm = cso.value_with_model(&c, &model, &d, &mkt, DayCount::Act365Fixed).expect("vm");
        assert!(vm.correlation > 0.5 && vm.correlation < 1.0, "rho {}", vm.correlation);
        assert!(vm.forward_price > 2.40);
        assert!(CalendarSpreadOption::new(OptionType::Call, OilContract::Wti, cm(2027, 6), cm(2027, 2), 0.0, 1.0).is_err());
    }

    #[test]
    fn contango_carry_economics() {
        let val = ymd(2026, 11, 9);
        let mkt = OilMarket::new();
        let d = disc(val);
        // A contango curve for the carry trade.
        let q = vec![FuturesQuote::new(cm(2027, 1), 60.0), FuturesQuote::new(cm(2027, 4), 62.5), FuturesQuote::new(cm(2027, 7), 64.0)];
        let c = FuturesCurve::new(OilContract::Wti.hub(), val, q, CurveInterpolation::Linear).expect("curve");
        let r = contango_carry(OilContract::Wti, &c, cm(2027, 1), cm(2027, 7), 0.30, &d, &mkt).expect("carry");
        assert_eq!(r.months, 6);
        assert_abs_diff_eq!(r.spread, 4.0, epsilon = 1e-12);
        assert_abs_diff_eq!(r.storage_cost, 1.80, epsilon = 1e-12);
        // ~4% for ~6 months on $60 is about $1.2.
        assert!(r.financing_cost > 1.0 && r.financing_cost < 1.4, "financing {}", r.financing_cost);
        assert_abs_diff_eq!(r.net_carry, 4.0 - 1.80 - r.financing_cost, epsilon = 1e-12);
        assert_abs_diff_eq!(r.breakeven_spread, 1.80 + r.financing_cost, epsilon = 1e-12);
        // Backwardation makes the carry negative.
        let b = contango_carry(OilContract::Wti, &wti_curve(val), cm(2027, 1), cm(2027, 6), 0.30, &d, &mkt).expect("carry");
        assert!(b.net_carry < 0.0);
    }
}
