//! Crack spreads: refined product minus crude, per barrel.
//!
//! A refinery's gross margin is the crack: what its products sell for less
//! what the crude cost, per barrel of crude. Single-product cracks (RBOB
//! vs WTI, gasoil vs Brent) are plain two-asset spreads once the product
//! is converted to $/bbl, and a crack *option* is a spread option priced
//! with the core's Bjerksund-Stensland. The 3-2-1 (three barrels of crude
//! into two of gasoline and one of distillate) is a basket spread; it is
//! priced here by matching the product basket to one lognormal and then
//! treating it as a two-asset spread, which is the standard desk shortcut
//! and accurate to a few cents for realistic correlations.

use fq_commod_core::spread::spread_price;
use fq_commod_core::{ContractMonth, FuturesCurve, SeasonalVol, SpreadMethod};
use fq_core::math::OptionType;
use fq_core::time::DayCount;
use fq_rates::curve::DiscountCurve;

use crate::contract::{OilContract, OilMarket, expiry};
use crate::error::OilError;
use crate::swap::{check_curve, payment_date};

/// Match `w1 F1 + w2 F2` (lognormal legs with vols `s1`, `s2`, correlation
/// `rho`, horizon `t`) to a single lognormal: returns `(forward, vol)`.
#[allow(clippy::too_many_arguments)]
pub fn basket_lognormal(w1: f64, f1: f64, s1: f64, w2: f64, f2: f64, s2: f64, rho: f64, t: f64) -> Result<(f64, f64), OilError> {
    for (n, x) in [("w1", w1), ("F1", f1), ("w2", w2), ("F2", f2)] {
        if !(x.is_finite() && x > 0.0) {
            return Err(OilError::Invalid(format!("{n} {x} must be positive")));
        }
    }
    if !(rho.is_finite() && (-1.0..=1.0).contains(&rho)) {
        return Err(OilError::Invalid(format!("correlation {rho} must be in [-1, 1]")));
    }
    if t < 0.0 || !t.is_finite() {
        return Err(OilError::Invalid(format!("horizon {t} must be non-negative")));
    }
    let a = w1 * f1;
    let b = w2 * f2;
    let m1 = a + b;
    let m2 = a * a * (s1 * s1 * t).exp() + b * b * (s2 * s2 * t).exp() + 2.0 * a * b * (rho * s1 * s2 * t).exp();
    let var = (m2 / (m1 * m1)).ln().max(0.0);
    let vol = if t > 0.0 { (var / t).sqrt() } else { 0.0 };
    Ok((m1, vol))
}

/// A single-product crack for one contract month, per barrel of crude.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CrackSpread {
    pub product: OilContract,
    pub crude: OilContract,
    pub month: ContractMonth,
}

impl CrackSpread {
    pub fn new(product: OilContract, crude: OilContract, month: ContractMonth) -> Result<Self, OilError> {
        if product.is_crude() || !crude.is_crude() {
            return Err(OilError::Invalid(format!("crack needs a product and a crude, got {} vs {}", product.code(), crude.code())));
        }
        Ok(Self { product, crude, month })
    }

    /// Forward crack in $/bbl: product converted to $/bbl less crude.
    pub fn forward(&self, product: &FuturesCurve, crude: &FuturesCurve) -> Result<f64, OilError> {
        check_curve(self.product, product)?;
        check_curve(self.crude, crude)?;
        Ok(self.product.to_per_barrel() * product.price(self.month)? - crude.price(self.month)?)
    }
}

/// Option on a single-product crack: `max(sign (product_bbl - crude - K), 0)`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CrackOption {
    pub kind: OptionType,
    pub spread: CrackSpread,
    /// $/bbl.
    pub strike: f64,
    /// Barrels of crude.
    pub barrels: f64,
    pub method: SpreadMethod,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CrackOptionValuation {
    pub product_per_bbl: f64,
    pub crude: f64,
    pub forward_crack: f64,
    pub time_to_expiry: f64,
    pub discount: f64,
    pub forward_price: f64,
    pub pv: f64,
}

impl CrackOption {
    pub fn new(kind: OptionType, spread: CrackSpread, strike: f64, barrels: f64) -> Result<Self, OilError> {
        if !strike.is_finite() || strike < 0.0 {
            return Err(OilError::Invalid(format!("strike {strike} must be non-negative (Kirk/BS need K >= 0)")));
        }
        if !(barrels.is_finite() && barrels > 0.0) {
            return Err(OilError::Invalid(format!("barrels {barrels} must be positive")));
        }
        Ok(Self { kind, spread, strike, barrels, method: SpreadMethod::default() })
    }

    pub fn with_method(mut self, method: SpreadMethod) -> Self {
        self.method = method;
        self
    }

    /// Expires with the earlier of the two contracts' last trading days.
    #[allow(clippy::too_many_arguments)]
    pub fn value(
        &self,
        product: &FuturesCurve,
        crude: &FuturesCurve,
        product_vol: &SeasonalVol,
        crude_vol: &SeasonalVol,
        rho: f64,
        discount: &DiscountCurve,
        market: &OilMarket,
        dc: DayCount,
    ) -> Result<CrackOptionValuation, OilError> {
        let s = self.spread;
        let f_prod = s.product.to_per_barrel() * product.price(s.month)?;
        let f_crude = crude.price(s.month)?;
        let fwd = s.forward(product, crude)?;
        let valuation = discount.reference_date();
        let exp = expiry(s.product, s.month, market)?.min(expiry(s.crude, s.month, market)?);
        if exp <= valuation {
            return Err(OilError::Expired { month: s.month, valuation });
        }
        let t = dc.year_fraction(valuation, exp, None)?;
        let df = discount.discount(payment_date(s.crude, s.month, market)?)?;
        let per_bbl = spread_price(self.method, self.kind, f_prod, f_crude, product_vol.vol(s.month), crude_vol.vol(s.month), rho, t, self.strike)?;
        Ok(CrackOptionValuation {
            product_per_bbl: f_prod,
            crude: f_crude,
            forward_crack: fwd,
            time_to_expiry: t,
            discount: df,
            forward_price: per_bbl,
            pv: per_bbl * self.barrels * df,
        })
    }
}

/// The 3-2-1 crack: 2 bbl gasoline + 1 bbl distillate per 3 bbl crude.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Crack321 {
    pub gasoline: OilContract,
    pub distillate: OilContract,
    pub crude: OilContract,
    pub month: ContractMonth,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Crack321Valuation {
    /// Forward 3-2-1 crack, $/bbl of crude.
    pub forward_crack: f64,
    /// Matched product basket per bbl of crude: `(forward, vol)`.
    pub basket_forward: f64,
    pub basket_vol: f64,
    pub time_to_expiry: f64,
    pub discount: f64,
    /// Option on the crack per bbl of crude (if a strike was given).
    pub option_forward_price: Option<f64>,
    pub option_pv: Option<f64>,
}

impl Crack321 {
    pub fn new(gasoline: OilContract, distillate: OilContract, crude: OilContract, month: ContractMonth) -> Result<Self, OilError> {
        if gasoline.is_crude() || distillate.is_crude() || !crude.is_crude() {
            return Err(OilError::Invalid("3-2-1 needs two products and a crude".into()));
        }
        Ok(Self { gasoline, distillate, crude, month })
    }

    /// Forward crack and, if `option` is given as `(kind, strike $/bbl,
    /// barrels of crude)`, a spread option on it. Vols are per contract;
    /// `rho_products` is gasoline/distillate correlation, `rho_crude` the
    /// (matched) basket-to-crude correlation.
    #[allow(clippy::too_many_arguments)]
    pub fn value(
        &self,
        gasoline: &FuturesCurve,
        distillate: &FuturesCurve,
        crude: &FuturesCurve,
        vols: (&SeasonalVol, &SeasonalVol, &SeasonalVol),
        rho_products: f64,
        rho_crude: f64,
        option: Option<(OptionType, f64, f64)>,
        discount: &DiscountCurve,
        market: &OilMarket,
        dc: DayCount,
    ) -> Result<Crack321Valuation, OilError> {
        check_curve(self.gasoline, gasoline)?;
        check_curve(self.distillate, distillate)?;
        check_curve(self.crude, crude)?;
        let m = self.month;
        let valuation = discount.reference_date();
        let exp = expiry(self.gasoline, m, market)?.min(expiry(self.distillate, m, market)?).min(expiry(self.crude, m, market)?);
        if exp <= valuation {
            return Err(OilError::Expired { month: m, valuation });
        }
        let t = dc.year_fraction(valuation, exp, None)?;
        let g = self.gasoline.to_per_barrel() * gasoline.price(m)?;
        let d = self.distillate.to_per_barrel() * distillate.price(m)?;
        let c = crude.price(m)?;
        let (basket_forward, basket_vol) = basket_lognormal(2.0 / 3.0, g, vols.0.vol(m), 1.0 / 3.0, d, vols.1.vol(m), rho_products, t)?;
        let forward_crack = basket_forward - c;
        let df = discount.discount(payment_date(self.crude, m, market)?)?;
        let (option_forward_price, option_pv) = match option {
            Some((kind, strike, barrels)) => {
                if !strike.is_finite() || strike < 0.0 || !(barrels.is_finite() && barrels > 0.0) {
                    return Err(OilError::Invalid(format!("strike {strike} must be >= 0 and barrels {barrels} > 0")));
                }
                let p = spread_price(SpreadMethod::default(), kind, basket_forward, c, basket_vol, vols.2.vol(m), rho_crude, t, strike)?;
                (Some(p), Some(p * barrels * df))
            }
            None => (None, None),
        };
        Ok(Crack321Valuation { forward_crack, basket_forward, basket_vol, time_to_expiry: t, discount: df, option_forward_price, option_pv })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::swap::tests::{cm, disc, wti_curve, ymd};
    use approx::assert_abs_diff_eq;
    use fq_commod_core::{CurveInterpolation, FuturesQuote};
    use fq_core::time::Date;

    fn product_curve(c: OilContract, val: Date, prices: &[f64]) -> FuturesCurve {
        let q = prices.iter().enumerate().map(|(i, p)| FuturesQuote::new(cm(2026, 12).add_months(i as i32), *p)).collect();
        FuturesCurve::new(c.hub(), val, q, CurveInterpolation::Linear).expect("curve")
    }

    #[test]
    fn basket_matching_limits() {
        // Perfect correlation and equal vols collapse to the same vol.
        let (f, v) = basket_lognormal(1.0, 50.0, 0.3, 1.0, 30.0, 0.3, 1.0, 1.0).expect("b");
        assert_abs_diff_eq!(f, 80.0);
        assert_abs_diff_eq!(v, 0.3, epsilon = 1e-10);
        // Imperfect correlation lowers basket vol.
        let (_, v2) = basket_lognormal(1.0, 50.0, 0.3, 1.0, 30.0, 0.3, 0.5, 1.0).expect("b");
        assert!(v2 < 0.3);
        assert!(basket_lognormal(1.0, 50.0, 0.3, 1.0, 30.0, 0.3, 1.5, 1.0).is_err());
    }

    #[test]
    fn rbob_crack_forward_and_option() {
        let val = ymd(2026, 11, 9);
        let mkt = OilMarket::new();
        let wti = wti_curve(val);
        // RBOB in $/gal: 2.10 * 42 = 88.20 $/bbl vs Feb WTI 70 -> crack 18.20.
        let rbob = product_curve(OilContract::Rbob, val, &[2.05, 2.08, 2.10, 2.15]);
        let spread = CrackSpread::new(OilContract::Rbob, OilContract::Wti, cm(2027, 2)).expect("crack");
        assert_abs_diff_eq!(spread.forward(&rbob, &wti).expect("fwd"), 2.10 * 42.0 - 70.0, epsilon = 1e-12);
        let opt = CrackOption::new(OptionType::Call, spread, 15.0, 100_000.0).expect("opt");
        let pv_ = SeasonalVol::flat(cm(2027, 1), 0.40).expect("v");
        let cv = SeasonalVol::flat(cm(2027, 1), 0.35).expect("v");
        let v = opt.value(&rbob, &wti, &pv_, &cv, 0.80, &disc(val), &mkt, DayCount::Act365Fixed).expect("v");
        assert_abs_diff_eq!(v.forward_crack, 18.20, epsilon = 1e-12);
        // Expiry is the earlier of RB Feb (Jan 29) and CL Feb (Jan 20): Jan 20.
        let t = DayCount::Act365Fixed.year_fraction(val, ymd(2027, 1, 20), None).expect("t");
        assert_abs_diff_eq!(v.time_to_expiry, t, epsilon = 1e-12);
        assert!(v.forward_price > 18.20 - 15.0);
        assert_abs_diff_eq!(v.pv, v.forward_price * 100_000.0 * v.discount, epsilon = 1e-6);
        assert!(CrackSpread::new(OilContract::Wti, OilContract::Brent, cm(2027, 2)).is_err());
    }

    #[test]
    fn three_two_one() {
        let val = ymd(2026, 11, 9);
        let mkt = OilMarket::new();
        let wti = wti_curve(val);
        let rbob = product_curve(OilContract::Rbob, val, &[2.05, 2.08, 2.10, 2.15]);
        let ho = product_curve(OilContract::HeatingOil, val, &[2.30, 2.32, 2.28, 2.20]);
        let crack = Crack321::new(OilContract::Rbob, OilContract::HeatingOil, OilContract::Wti, cm(2027, 2)).expect("321");
        let v = SeasonalVol::flat(cm(2027, 1), 0.38).expect("v");
        let r = crack
            .value(&rbob, &ho, &wti, (&v, &v, &v), 0.85, 0.80, Some((OptionType::Call, 20.0, 300_000.0)), &disc(val), &mkt, DayCount::Act365Fixed)
            .expect("r");
        let expected = (2.0 * 2.10 * 42.0 + 2.28 * 42.0) / 3.0 - 70.0;
        assert_abs_diff_eq!(r.forward_crack, expected, epsilon = 1e-12);
        assert!(r.basket_vol < 0.38 && r.basket_vol > 0.30);
        let op = r.option_forward_price.expect("option");
        assert!(op > (expected - 20.0).max(0.0));
        assert_abs_diff_eq!(r.option_pv.expect("pv"), op * 300_000.0 * r.discount, epsilon = 1e-6);
        let no_opt = crack.value(&rbob, &ho, &wti, (&v, &v, &v), 0.85, 0.80, None, &disc(val), &mkt, DayCount::Act365Fixed).expect("r");
        assert!(no_opt.option_pv.is_none());
        assert!(Crack321::new(OilContract::Rbob, OilContract::Brent, OilContract::Wti, cm(2027, 2)).is_err());
    }
}
