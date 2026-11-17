//! Firm transport capacity as a strip of basis spread options.
//!
//! A shipper holding firm capacity from a receipt point to a delivery
//! point may, each day, nominate up to the maximum daily quantity (MDQ).
//! Flowing gas earns `(1 - fuel) * P_delivery - P_receipt - commodity
//! charge` per MMBtu received, and the shipper only flows when that is
//! positive, so each month of capacity is a call on the locational spread
//! with strike equal to the variable cost. The reservation (demand) charge
//! is paid regardless and is netted off.
//!
//! Intrinsic value uses the two forward curves directly; extrinsic value
//! prices each month's spread option (Bjerksund-Stensland by default) with
//! the two hubs' vols and a correlation, which for gas basis is high and
//! the single most important input.

use fq_commod_core::spread::spread_price;
use fq_commod_core::{ContractMonth, FuturesCurve, SeasonalVol, SpreadMethod};
use fq_core::math::OptionType;
use fq_core::time::DayCount;
use fq_rates::curve::DiscountCurve;

use crate::error::GasError;
use crate::hub::{GasHub, NymexCalendar, nymex_expiry};

#[derive(Debug, Clone, PartialEq)]
pub struct TransportCapacity {
    pub receipt: GasHub,
    pub delivery: GasHub,
    pub start: ContractMonth,
    pub end: ContractMonth,
    /// Maximum daily quantity received, MMBtu/d.
    pub mdq: f64,
    /// Fraction of received gas retained by the pipeline as fuel.
    pub fuel_retention: f64,
    /// Variable (commodity) charge per MMBtu received.
    pub commodity_charge: f64,
    /// Reservation charge per MMBtu/d of MDQ per day (so monthly cost is
    /// `demand_charge * mdq * days`).
    pub demand_charge: f64,
    pub method: SpreadMethod,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TransportMonth {
    pub month: ContractMonth,
    pub days: i32,
    pub receipt_forward: f64,
    pub delivery_forward: f64,
    /// `(1 - fuel) * delivery - receipt - commodity`, per MMBtu received.
    pub forward_margin: f64,
    pub discount: f64,
    /// Discounted, flow at MDQ only when the forward margin is positive.
    pub intrinsic_pv: f64,
    /// Discounted spread option value at MDQ.
    pub option_pv: f64,
    pub demand_pv: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TransportValuation {
    pub months: Vec<TransportMonth>,
    pub intrinsic_pv: f64,
    pub option_pv: f64,
    pub extrinsic_pv: f64,
    pub demand_pv: f64,
    /// `option_pv - demand_pv`: what the capacity is worth net of what you
    /// pay to hold it.
    pub net_pv: f64,
}

impl TransportCapacity {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        receipt: GasHub,
        delivery: GasHub,
        start: ContractMonth,
        end: ContractMonth,
        mdq: f64,
        fuel_retention: f64,
        commodity_charge: f64,
        demand_charge: f64,
    ) -> Result<Self, GasError> {
        if receipt == delivery {
            return Err(GasError::Invalid("receipt and delivery hubs must differ".into()));
        }
        if ContractMonth::range(start, end).is_empty() {
            return Err(GasError::EmptyPeriod { start, end });
        }
        if !(mdq.is_finite() && mdq > 0.0) {
            return Err(GasError::Invalid(format!("MDQ {mdq} must be positive")));
        }
        if !(fuel_retention.is_finite() && (0.0..1.0).contains(&fuel_retention)) {
            return Err(GasError::Invalid(format!("fuel retention {fuel_retention} must be in [0, 1)")));
        }
        for (n, x) in [("commodity charge", commodity_charge), ("demand charge", demand_charge)] {
            if !(x.is_finite() && x >= 0.0) {
                return Err(GasError::Invalid(format!("{n} {x} must be non-negative")));
            }
        }
        Ok(Self { receipt, delivery, start, end, mdq, fuel_retention, commodity_charge, demand_charge, method: SpreadMethod::default() })
    }

    pub fn with_method(mut self, method: SpreadMethod) -> Self {
        self.method = method;
        self
    }

    pub fn months(&self) -> Vec<ContractMonth> {
        ContractMonth::range(self.start, self.end)
    }

    /// `receipt_curve` / `delivery_curve` are outright curves at the two
    /// hubs; `rho` is the correlation of their log returns. Cash flows are
    /// discounted to the delivery month's last day.
    #[allow(clippy::too_many_arguments)]
    pub fn value(
        &self,
        receipt_curve: &FuturesCurve,
        delivery_curve: &FuturesCurve,
        receipt_vol: &SeasonalVol,
        delivery_vol: &SeasonalVol,
        rho: f64,
        discount: &DiscountCurve,
        cal: &NymexCalendar,
        dc: DayCount,
    ) -> Result<TransportValuation, GasError> {
        if receipt_curve.hub().code() != self.receipt.code() {
            return Err(GasError::WrongHub { expected: self.receipt.code().into(), got: receipt_curve.hub().to_string() });
        }
        if delivery_curve.hub().code() != self.delivery.code() {
            return Err(GasError::WrongHub { expected: self.delivery.code().into(), got: delivery_curve.hub().to_string() });
        }
        let valuation = discount.reference_date();
        let w_del = 1.0 - self.fuel_retention;
        let mut months = Vec::new();
        let (mut intr, mut opt, mut dem) = (0.0, 0.0, 0.0);
        for m in self.months() {
            let expiry = nymex_expiry(m, cal)?;
            if expiry <= valuation {
                return Err(GasError::MonthExpired { month: m, valuation });
            }
            let t = dc.year_fraction(valuation, expiry, None)?;
            let days = m.delivery()?.days();
            let qty = self.mdq * f64::from(days);
            let f_r = receipt_curve.price(m)?;
            let f_d = delivery_curve.price(m)?;
            let df = discount.discount(m.last_day()?)?;
            let margin = w_del * f_d - f_r - self.commodity_charge;
            let intrinsic_pv = margin.max(0.0) * qty * df;
            let per_unit =
                spread_price(self.method, OptionType::Call, w_del * f_d, f_r, delivery_vol.vol(m), receipt_vol.vol(m), rho, t, self.commodity_charge)?;
            let option_pv = per_unit * qty * df;
            let demand_pv = self.demand_charge * qty * df;
            months.push(TransportMonth {
                month: m,
                days,
                receipt_forward: f_r,
                delivery_forward: f_d,
                forward_margin: margin,
                discount: df,
                intrinsic_pv,
                option_pv,
                demand_pv,
            });
            intr += intrinsic_pv;
            opt += option_pv;
            dem += demand_pv;
        }
        Ok(TransportValuation { months, intrinsic_pv: intr, option_pv: opt, extrinsic_pv: opt - intr, demand_pv: dem, net_pv: opt - dem })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_abs_diff_eq;
    use fq_commod_core::{CurveInterpolation, FuturesQuote, VolQuote};
    use fq_core::time::Date;
    use fq_rates::curve::Interpolation;

    fn ymd(y: i32, m: u32, d: u32) -> Date {
        Date::from_ymd(y, m, d).expect("valid date")
    }

    fn cm(y: i32, m: u32) -> ContractMonth {
        ContractMonth::new(y, m).expect("month")
    }

    fn market() -> (FuturesCurve, FuturesCurve, SeasonalVol, SeasonalVol, DiscountCurve, NymexCalendar) {
        let val = ymd(2026, 11, 9);
        // Dominion South (Appalachia) to Transco Z6 NY: deeply in the money
        // in winter, out of the money in shoulder months.
        let dom = FuturesCurve::new(
            GasHub::DominionSouth.hub(),
            val,
            vec![
                FuturesQuote::new(cm(2027, 1), 3.40),
                FuturesQuote::new(cm(2027, 2), 3.30),
                FuturesQuote::new(cm(2027, 3), 3.00),
                FuturesQuote::new(cm(2027, 4), 2.40),
            ],
            CurveInterpolation::Linear,
        )
        .expect("dom");
        let z6 = FuturesCurve::new(
            GasHub::TranscoZ6NY.hub(),
            val,
            vec![
                FuturesQuote::new(cm(2027, 1), 9.40),
                FuturesQuote::new(cm(2027, 2), 7.20),
                FuturesQuote::new(cm(2027, 3), 3.90),
                FuturesQuote::new(cm(2027, 4), 2.50),
            ],
            CurveInterpolation::Linear,
        )
        .expect("z6");
        let dom_vol = SeasonalVol::flat(cm(2027, 1), 0.60).expect("v");
        let z6_vol = SeasonalVol::new(vec![VolQuote::new(cm(2027, 1), 1.20), VolQuote::new(cm(2027, 3), 0.70), VolQuote::new(cm(2027, 4), 0.55)]).expect("v");
        let nodes: Vec<(Date, f64)> = (1..=24).map(|k| (val.add_months(k).expect("d"), (-0.04 * k as f64 / 12.0).exp())).collect();
        let disc = DiscountCurve::new(val, &nodes, Interpolation::LogLinearDiscount).expect("disc");
        (dom, z6, dom_vol, z6_vol, disc, NymexCalendar::new())
    }

    #[test]
    fn intrinsic_extrinsic_and_demand_charges() {
        let (dom, z6, dv, zv, disc, cal) = market();
        let cap = TransportCapacity::new(GasHub::DominionSouth, GasHub::TranscoZ6NY, cm(2027, 1), cm(2027, 4), 10_000.0, 0.03, 0.10, 0.45).expect("cap");
        let v = cap.value(&dom, &z6, &dv, &zv, 0.75, &disc, &cal, DayCount::Act365Fixed).expect("v");
        assert_eq!(v.months.len(), 4);
        // Jan margin: 0.97*9.40 - 3.40 - 0.10 = 5.618.
        assert_abs_diff_eq!(v.months[0].forward_margin, 0.97 * 9.40 - 3.40 - 0.10, epsilon = 1e-12);
        assert_abs_diff_eq!(v.months[0].intrinsic_pv, v.months[0].forward_margin * 10_000.0 * 31.0 * v.months[0].discount, epsilon = 1e-6);
        // Apr: 0.97*2.50 - 2.40 - 0.10 = -0.075 -> no intrinsic, some option value.
        assert!(v.months[3].forward_margin < 0.0);
        assert_abs_diff_eq!(v.months[3].intrinsic_pv, 0.0);
        assert!(v.months[3].option_pv > 0.0);
        // Option >= intrinsic every month, and totals add up.
        for m in &v.months {
            assert!(m.option_pv >= m.intrinsic_pv - 1e-9);
        }
        assert_abs_diff_eq!(v.extrinsic_pv, v.option_pv - v.intrinsic_pv, epsilon = 1e-9);
        assert_abs_diff_eq!(v.net_pv, v.option_pv - v.demand_pv, epsilon = 1e-9);
        let demand: f64 = v.months.iter().map(|m| 0.45 * 10_000.0 * f64::from(m.days) * m.discount).sum();
        assert_abs_diff_eq!(v.demand_pv, demand, epsilon = 1e-6);
        // Lower correlation makes the spread option worth more.
        let lo = cap.value(&dom, &z6, &dv, &zv, 0.30, &disc, &cal, DayCount::Act365Fixed).expect("v");
        assert!(lo.option_pv > v.option_pv);
    }

    #[test]
    fn validation() {
        let (dom, z6, dv, zv, disc, cal) = market();
        assert!(TransportCapacity::new(GasHub::Waha, GasHub::Waha, cm(2027, 1), cm(2027, 2), 1.0, 0.0, 0.0, 0.0).is_err());
        assert!(TransportCapacity::new(GasHub::Waha, GasHub::HoustonShipChannel, cm(2027, 1), cm(2027, 2), 1.0, 1.0, 0.0, 0.0).is_err());
        let cap = TransportCapacity::new(GasHub::DominionSouth, GasHub::TranscoZ6NY, cm(2027, 1), cm(2027, 2), 1.0, 0.0, 0.0, 0.0).expect("cap");
        // Curves swapped -> wrong hub.
        assert!(matches!(cap.value(&z6, &dom, &dv, &zv, 0.5, &disc, &cal, DayCount::Act365Fixed), Err(GasError::WrongHub { .. })));
    }
}
