//! European options on a single futures contract (Black-76).
//!
//! Exchange options on futures are mostly American and margined, but the
//! early-exercise premium on a futures option is small and desks quote and
//! risk them as Black-76 Europeans, so that is the core pricer. OTC options
//! on a monthly swap (the typical gas/oil "option on the strip") are priced
//! the same way with the strip price as the forward and an averaging vol
//! adjustment, which lives in [`crate::asian`].

use fq_core::math::{OptionType, black76, shifted_black_implied_vol};
use fq_core::time::{Date, DayCount};
use fq_rates::curve::DiscountCurve;

use crate::contract::ContractMonth;
use crate::curve::FuturesCurve;
use crate::error::CommodError;
use crate::vol::SeasonalVol;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FuturesOption {
    pub kind: OptionType,
    pub contract: ContractMonth,
    pub strike: f64,
    pub expiry: Date,
    /// Contract size in the hub's unit (e.g. 10,000 MMBtu for one NG lot).
    pub quantity: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FuturesOptionValuation {
    pub forward: f64,
    pub vol: f64,
    pub time_to_expiry: f64,
    pub discount: f64,
    /// Undiscounted price per unit.
    pub forward_price: f64,
    /// Discounted value of the whole position.
    pub pv: f64,
    /// dPV / dF, in currency per unit price move (already scaled by quantity).
    pub delta: f64,
    pub gamma: f64,
    /// dPV per 1.00 (100 vol point) move; divide by 100 for a 1-point vega.
    pub vega: f64,
}

impl FuturesOption {
    pub fn new(kind: OptionType, contract: ContractMonth, strike: f64, expiry: Date, quantity: f64) -> Result<Self, CommodError> {
        if !(strike.is_finite() && strike > 0.0) {
            return Err(CommodError::Invalid(format!("strike {strike} must be positive")));
        }
        if !(quantity.is_finite() && quantity > 0.0) {
            return Err(CommodError::Invalid(format!("quantity {quantity} must be positive")));
        }
        let first_delivery = contract.first_day()?;
        if expiry > first_delivery {
            return Err(CommodError::ExpiryAfterContract { expiry, limit: first_delivery });
        }
        Ok(Self { kind, contract, strike, expiry, quantity })
    }

    pub fn time_to_expiry(&self, valuation: Date, dc: DayCount) -> Result<f64, CommodError> {
        if self.expiry <= valuation {
            return Err(CommodError::Expired { expiry: self.expiry, valuation });
        }
        Ok(dc.year_fraction(valuation, self.expiry, None)?)
    }

    /// Price with an explicit vol.
    pub fn value_with_vol(&self, futures: &FuturesCurve, vol: f64, discount: &DiscountCurve, dc: DayCount) -> Result<FuturesOptionValuation, CommodError> {
        let valuation = futures.reference_date();
        let t = self.time_to_expiry(valuation, dc)?;
        let f = futures.price(self.contract)?;
        let df = discount.discount(self.expiry)?;
        let r = black76(self.kind, f, self.strike, vol, t)?;
        let scale = self.quantity * df;
        Ok(FuturesOptionValuation {
            forward: f,
            vol,
            time_to_expiry: t,
            discount: df,
            forward_price: r.price,
            pv: r.price * scale,
            delta: r.delta * scale,
            gamma: r.gamma * scale,
            vega: r.vega * scale,
        })
    }

    /// Price off the contract's quoted ATM vol.
    pub fn value(&self, futures: &FuturesCurve, vols: &SeasonalVol, discount: &DiscountCurve, dc: DayCount) -> Result<FuturesOptionValuation, CommodError> {
        self.value_with_vol(futures, vols.vol(self.contract), discount, dc)
    }

    /// Black vol that reproduces a quoted premium per unit (undiscounted).
    pub fn implied_vol(&self, futures: &FuturesCurve, forward_premium: f64, dc: DayCount) -> Result<f64, CommodError> {
        let t = self.time_to_expiry(futures.reference_date(), dc)?;
        let f = futures.price(self.contract)?;
        Ok(shifted_black_implied_vol(self.kind, f, self.strike, t, 0.0, forward_premium, 10.0)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::{Commodity, Hub, Unit};
    use crate::curve::{CurveInterpolation, FuturesQuote};
    use crate::vol::VolQuote;
    use approx::assert_abs_diff_eq;
    use fq_rates::curve::Interpolation;

    fn ymd(y: i32, m: u32, d: u32) -> Date {
        Date::from_ymd(y, m, d).expect("valid date")
    }

    fn cm(y: i32, m: u32) -> ContractMonth {
        ContractMonth::new(y, m).expect("month")
    }

    fn setup() -> (FuturesCurve, DiscountCurve) {
        let val = ymd(2026, 11, 9);
        let hub = Hub::new("WTI", Commodity::Crude, Unit::Barrel);
        let q = vec![FuturesQuote::new(cm(2026, 12), 72.0), FuturesQuote::new(cm(2027, 1), 71.5), FuturesQuote::new(cm(2027, 2), 71.0)];
        let fut = FuturesCurve::new(hub, val, q, CurveInterpolation::Linear).expect("curve");
        // Flat 4% continuously compounded.
        let nodes: Vec<(Date, f64)> = (1..=24).map(|k| (val.add_months(k).expect("d"), (-0.04 * k as f64 / 12.0).exp())).collect();
        let disc = DiscountCurve::new(val, &nodes, Interpolation::LogLinearDiscount).expect("disc");
        (fut, disc)
    }

    #[test]
    fn black76_call_matches_hand_calculation() {
        let (fut, disc) = setup();
        let expiry = ymd(2027, 1, 14);
        let opt = FuturesOption::new(OptionType::Call, cm(2027, 2), 70.0, expiry, 1000.0).expect("opt");
        let v = opt.value_with_vol(&fut, 0.30, &disc, DayCount::Act365Fixed).expect("value");
        let t = 66.0 / 365.0;
        assert_abs_diff_eq!(v.time_to_expiry, t, epsilon = 1e-12);
        let sd = 0.30 * t.sqrt();
        let d1 = ((71.0f64 / 70.0).ln() + 0.5 * sd * sd) / sd;
        let d2 = d1 - sd;
        let expected = 71.0 * fq_core::math::norm_cdf(d1) - 70.0 * fq_core::math::norm_cdf(d2);
        assert_abs_diff_eq!(v.forward_price, expected, epsilon = 1e-10);
        assert_abs_diff_eq!(v.pv, expected * v.discount * 1000.0, epsilon = 1e-8);
        assert!(v.delta > 0.0 && v.delta < 1000.0 * v.discount);
        // Implied vol round trip.
        let iv = opt.implied_vol(&fut, v.forward_price, DayCount::Act365Fixed).expect("iv");
        assert_abs_diff_eq!(iv, 0.30, epsilon = 1e-9);
    }

    #[test]
    fn put_call_parity_and_seasonal_vol_lookup() {
        let (fut, disc) = setup();
        let expiry = ymd(2026, 12, 16);
        let call = FuturesOption::new(OptionType::Call, cm(2027, 1), 73.0, expiry, 1.0).expect("c");
        let put = FuturesOption::new(OptionType::Put, cm(2027, 1), 73.0, expiry, 1.0).expect("p");
        let vols = SeasonalVol::new(vec![VolQuote::new(cm(2026, 12), 0.35), VolQuote::new(cm(2027, 2), 0.31)]).expect("vols");
        let c = call.value(&fut, &vols, &disc, DayCount::Act365Fixed).expect("c");
        let p = put.value(&fut, &vols, &disc, DayCount::Act365Fixed).expect("p");
        assert_abs_diff_eq!(c.vol, 0.33, epsilon = 1e-12);
        assert_abs_diff_eq!(c.forward_price - p.forward_price, 71.5 - 73.0, epsilon = 1e-10);
    }

    #[test]
    fn validation() {
        let (fut, disc) = setup();
        // Expiry after first delivery day is refused.
        assert!(matches!(FuturesOption::new(OptionType::Call, cm(2027, 1), 70.0, ymd(2027, 1, 2), 1.0), Err(CommodError::ExpiryAfterContract { .. })));
        // Already expired relative to the curve date.
        let opt = FuturesOption::new(OptionType::Call, cm(2027, 1), 70.0, ymd(2026, 11, 1), 1.0).expect("opt");
        assert!(matches!(opt.value_with_vol(&fut, 0.3, &disc, DayCount::Act365Fixed), Err(CommodError::Expired { .. })));
    }
}
