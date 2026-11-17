//! The soybean crush.
//!
//! Crushing a 60 lb bushel of soybeans yields about 44 lb of meal and 11
//! lb of oil (the remainder is waste and moisture). With meal quoted in
//! dollars per short ton and oil in cents per pound, the *board crush*
//! per bushel is
//!
//! ```text
//! crush = 0.022 * ZM + 0.11 * ZL - ZS / 100        ($/bu)
//! ```
//!
//! (44 lb = 0.022 short tons; 11 lb at cents/lb is `0.11 * ZL` dollars;
//! beans in cents/bu divided by 100). The crush is the processor's gross
//! margin and the spread the market trades as a package. A crush option is
//! priced here by matching the product basket (meal plus oil) to one
//! lognormal and treating it as a two-asset spread against beans with the
//! core's Bjerksund-Stensland, the same shortcut as the oil 3-2-1.

use fq_commod_core::spread::spread_price;
use fq_commod_core::{ContractMonth, FuturesCurve, SeasonalVol, SpreadMethod};
use fq_core::math::OptionType;
use fq_core::time::DayCount;
use fq_rates::curve::DiscountCurve;

use crate::contract::{AgCalendar, AgContract, option_expiry};
use crate::error::AgsError;
use crate::option::check_curve;

/// Meal yield per bushel in short tons (44 lb).
pub const MEAL_TONS_PER_BUSHEL: f64 = 0.022;
/// Oil yield per bushel in pounds.
pub const OIL_LBS_PER_BUSHEL: f64 = 11.0;

/// Match `w1 F1 + w2 F2` of two lognormals to one: `(forward, vol)`.
#[allow(clippy::too_many_arguments)]
pub fn basket_lognormal(w1: f64, f1: f64, s1: f64, w2: f64, f2: f64, s2: f64, rho: f64, t: f64) -> Result<(f64, f64), AgsError> {
    for (n, x) in [("w1", w1), ("F1", f1), ("w2", w2), ("F2", f2)] {
        if !(x.is_finite() && x > 0.0) {
            return Err(AgsError::Invalid(format!("{n} {x} must be positive")));
        }
    }
    if !(rho.is_finite() && (-1.0..=1.0).contains(&rho)) {
        return Err(AgsError::Invalid(format!("correlation {rho} must be in [-1, 1]")));
    }
    if !(t.is_finite() && t >= 0.0) {
        return Err(AgsError::Invalid(format!("horizon {t} must be non-negative")));
    }
    let a = w1 * f1;
    let b = w2 * f2;
    let m1 = a + b;
    let m2 = a * a * (s1 * s1 * t).exp() + b * b * (s2 * s2 * t).exp() + 2.0 * a * b * (rho * s1 * s2 * t).exp();
    let var = (m2 / (m1 * m1)).ln().max(0.0);
    Ok((m1, if t > 0.0 { (var / t).sqrt() } else { 0.0 }))
}

/// The crush for one contract month (beans, meal and oil of the same
/// month; meal and oil list every bean month so this always exists).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Crush {
    pub month: ContractMonth,
    /// Bushels of beans crushed.
    pub bushels: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CrushValuation {
    /// $/bu.
    pub beans: f64,
    /// $/bu of meal value.
    pub meal_value: f64,
    /// $/bu of oil value.
    pub oil_value: f64,
    /// `meal_value + oil_value - beans`, $/bu.
    pub crush_margin: f64,
    /// Total dollars on `bushels`.
    pub gross_margin: f64,
    /// Share of product value from oil (the "oil share"), a watched ratio.
    pub oil_share: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CrushOptionValuation {
    pub spot: CrushValuation,
    pub basket_forward: f64,
    pub basket_vol: f64,
    pub time_to_expiry: f64,
    pub discount: f64,
    /// $/bu, undiscounted.
    pub forward_price: f64,
    pub pv: f64,
}

impl Crush {
    pub fn new(month: ContractMonth, bushels: f64) -> Result<Self, AgsError> {
        if !AgContract::Soybeans.is_listed(month) {
            return Err(AgsError::NotListed { contract: AgContract::Soybeans.code(), month });
        }
        if !(bushels.is_finite() && bushels > 0.0) {
            return Err(AgsError::Invalid(format!("bushels {bushels} must be positive")));
        }
        Ok(Self { month, bushels })
    }

    /// Board crush off the three curves (beans cents/bu, meal $/st, oil cents/lb).
    pub fn value(&self, beans: &FuturesCurve, meal: &FuturesCurve, oil: &FuturesCurve) -> Result<CrushValuation, AgsError> {
        check_curve(AgContract::Soybeans, beans)?;
        check_curve(AgContract::SoybeanMeal, meal)?;
        check_curve(AgContract::SoybeanOil, oil)?;
        let b = beans.price(self.month)? / 100.0;
        let meal_value = MEAL_TONS_PER_BUSHEL * meal.price(self.month)?;
        let oil_value = OIL_LBS_PER_BUSHEL * oil.price(self.month)? / 100.0;
        let crush_margin = meal_value + oil_value - b;
        Ok(CrushValuation {
            beans: b,
            meal_value,
            oil_value,
            crush_margin,
            gross_margin: crush_margin * self.bushels,
            oil_share: oil_value / (meal_value + oil_value),
        })
    }

    /// Option on the crush margin struck at `strike` $/bu (a call is the
    /// processor's natural long). Vols are per contract; `rho_products` is
    /// meal/oil, `rho_beans` the basket-to-beans correlation. Expires with
    /// the bean option.
    #[allow(clippy::too_many_arguments)]
    pub fn option(
        &self,
        kind: OptionType,
        strike: f64,
        beans: &FuturesCurve,
        meal: &FuturesCurve,
        oil: &FuturesCurve,
        vols: (&SeasonalVol, &SeasonalVol, &SeasonalVol),
        rho_products: f64,
        rho_beans: f64,
        discount: &DiscountCurve,
        cal: &AgCalendar,
        dc: DayCount,
    ) -> Result<CrushOptionValuation, AgsError> {
        if !strike.is_finite() || strike < 0.0 {
            return Err(AgsError::Invalid(format!("strike {strike} must be non-negative")));
        }
        let spot = self.value(beans, meal, oil)?;
        let valuation = beans.reference_date();
        let expiry = option_expiry(AgContract::Soybeans, self.month, cal)?;
        if expiry <= valuation {
            return Err(AgsError::Expired { expiry, valuation });
        }
        let t = dc.year_fraction(valuation, expiry, None)?;
        let m = self.month;
        let (basket_forward, basket_vol) =
            basket_lognormal(MEAL_TONS_PER_BUSHEL, meal.price(m)?, vols.1.vol(m), OIL_LBS_PER_BUSHEL / 100.0, oil.price(m)?, vols.2.vol(m), rho_products, t)?;
        let per_bu = spread_price(SpreadMethod::default(), kind, basket_forward, spot.beans, basket_vol, vols.0.vol(m), rho_beans, t, strike)?;
        let df = discount.discount(expiry)?;
        Ok(CrushOptionValuation { spot, basket_forward, basket_vol, time_to_expiry: t, discount: df, forward_price: per_bu, pv: per_bu * self.bushels * df })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::option::tests::{cm, curve, disc, ymd};
    use approx::assert_abs_diff_eq;

    #[test]
    fn board_crush_arithmetic() {
        let val = ymd(2026, 11, 9);
        // Jan-27: beans 1050 c/bu, meal $310/st, oil 45 c/lb.
        let beans = curve(AgContract::Soybeans, val, &[(cm(2027, 1), 1050.0), (cm(2027, 3), 1060.0)]);
        let meal = curve(AgContract::SoybeanMeal, val, &[(cm(2027, 1), 310.0), (cm(2027, 3), 312.0)]);
        let oil = curve(AgContract::SoybeanOil, val, &[(cm(2027, 1), 45.0), (cm(2027, 3), 45.5)]);
        let crush = Crush::new(cm(2027, 1), 100_000.0).expect("crush");
        let v = crush.value(&beans, &meal, &oil).expect("v");
        // 0.022*310 = 6.82; 11*0.45 = 4.95; beans 10.50 -> crush 1.27.
        assert_abs_diff_eq!(v.meal_value, 6.82, epsilon = 1e-12);
        assert_abs_diff_eq!(v.oil_value, 4.95, epsilon = 1e-12);
        assert_abs_diff_eq!(v.crush_margin, 1.27, epsilon = 1e-12);
        assert_abs_diff_eq!(v.gross_margin, 127_000.0, epsilon = 1e-6);
        assert_abs_diff_eq!(v.oil_share, 4.95 / 11.77, epsilon = 1e-12);

        let vols = SeasonalVol::flat(cm(2027, 1), 0.25).expect("v");
        let o = crush
            .option(OptionType::Call, 1.00, &beans, &meal, &oil, (&vols, &vols, &vols), 0.4, 0.85, &disc(val), &AgCalendar::new(), DayCount::Act365Fixed)
            .expect("o");
        assert!(o.forward_price > 0.27);
        assert!(o.basket_vol < 0.25);
        assert_abs_diff_eq!(o.basket_forward, 11.77, epsilon = 1e-12);
        assert_abs_diff_eq!(o.pv, o.forward_price * 100_000.0 * o.discount, epsilon = 1e-6);
        // Bean option expiry for Jan-27: last business day of Dec 2026 is Thu
        // Dec 31, two business days before is Tue Dec 29, and the last Friday
        // on or before that which is a business day is Dec 18 (Dec 25 is a holiday).
        let t = DayCount::Act365Fixed.year_fraction(val, ymd(2026, 12, 18), None).expect("t");
        assert_abs_diff_eq!(o.time_to_expiry, t, epsilon = 1e-12);
    }

    #[test]
    fn validation() {
        assert!(matches!(Crush::new(cm(2027, 2), 1.0), Err(AgsError::NotListed { .. })));
        assert!(Crush::new(cm(2027, 1), 0.0).is_err());
        assert!(basket_lognormal(1.0, 1.0, 0.2, 1.0, 1.0, 0.2, 1.5, 1.0).is_err());
    }
}
