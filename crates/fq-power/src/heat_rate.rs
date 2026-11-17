//! Heat-rate (spark spread) options and tolling agreements.
//!
//! A gas plant converts fuel to power at its heat rate `H` (MMBtu per MWh)
//! and runs when `P_power > H * P_gas + VOM`. Financially that is a call on
//! the spread `P_power - H * P_gas` struck at the variable O&M, and a
//! strip of them, one per block-month, is the standard way to mark a
//! plant or a tolling deal before getting into unit-commitment detail
//! (which [`crate::dispatch`] does).
//!
//! The spread pricers come from `fq-commod-core` (Bjerksund-Stensland by
//! default). The gas leg is any `FuturesCurve` in MMBtu, so the same code
//! prices a PJM West / TETCO M3 spark or an ERCOT North / Houston Ship
//! Channel one.

use fq_commod_core::spread::spread_price;
use fq_commod_core::{ContractMonth, FuturesCurve, SeasonalVol, SpreadMethod, Unit};
use fq_core::math::OptionType;
use fq_core::time::DayCount;
use fq_rates::curve::DiscountCurve;

use crate::curve::PowerCurve;
use crate::error::PowerError;
use crate::hub::PowerHub;
use crate::iso::Block;
use crate::swap::payment_date;

#[derive(Debug, Clone, PartialEq)]
pub struct HeatRateOption {
    pub kind: OptionType,
    pub hub: PowerHub,
    pub block: Block,
    pub start: ContractMonth,
    pub end: ContractMonth,
    pub mw: f64,
    /// MMBtu per MWh.
    pub heat_rate: f64,
    /// Variable O&M, $/MWh: the strike on the spread.
    pub vom: f64,
    pub method: SpreadMethod,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HeatRateMonth {
    pub month: ContractMonth,
    pub hours: u32,
    pub power_forward: f64,
    pub gas_forward: f64,
    /// `power - heat_rate * gas - vom`, $/MWh.
    pub forward_spark: f64,
    /// Market-implied heat rate `power / gas`.
    pub implied_heat_rate: f64,
    pub time_to_expiry: f64,
    pub discount: f64,
    pub intrinsic_pv: f64,
    pub option_pv: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HeatRateValuation {
    pub months: Vec<HeatRateMonth>,
    pub intrinsic_pv: f64,
    pub option_pv: f64,
    pub extrinsic_pv: f64,
}

impl HeatRateOption {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        kind: OptionType,
        hub: PowerHub,
        block: Block,
        start: ContractMonth,
        end: ContractMonth,
        mw: f64,
        heat_rate: f64,
        vom: f64,
    ) -> Result<Self, PowerError> {
        if ContractMonth::range(start, end).is_empty() {
            return Err(PowerError::EmptyPeriod { start, end });
        }
        for (n, x) in [("MW", mw), ("heat rate", heat_rate)] {
            if !(x.is_finite() && x > 0.0) {
                return Err(PowerError::Invalid(format!("{n} {x} must be positive")));
            }
        }
        if !(vom.is_finite() && vom >= 0.0) {
            return Err(PowerError::Invalid(format!("VOM {vom} must be non-negative")));
        }
        Ok(Self { kind, hub, block, start, end, mw, heat_rate, vom, method: SpreadMethod::default() })
    }

    pub fn with_method(mut self, method: SpreadMethod) -> Self {
        self.method = method;
        self
    }

    pub fn months(&self) -> Vec<ContractMonth> {
        ContractMonth::range(self.start, self.end)
    }

    /// `power_vol` is the vol of the block strip at the hub, `gas_vol` the
    /// gas contract vol, `rho` their log-return correlation. Options
    /// expire the day before delivery starts.
    #[allow(clippy::too_many_arguments)]
    pub fn value(
        &self,
        power: &PowerCurve,
        gas: &FuturesCurve,
        power_vol: &SeasonalVol,
        gas_vol: &SeasonalVol,
        rho: f64,
        discount: &DiscountCurve,
        dc: DayCount,
    ) -> Result<HeatRateValuation, PowerError> {
        if power.hub() != self.hub {
            return Err(PowerError::WrongHub { expected: self.hub.code().into(), got: power.hub().code().into() });
        }
        if gas.hub().unit() != Unit::MmBtu {
            return Err(PowerError::Invalid(format!("gas curve must be in MMBtu, got {}", gas.hub())));
        }
        let valuation = discount.reference_date();
        let mut months = Vec::new();
        let (mut intr, mut opt) = (0.0, 0.0);
        for m in self.months() {
            let first = m.first_day()?;
            if first <= valuation {
                return Err(PowerError::MonthStarted { month: m, valuation });
            }
            let expiry = first - 1;
            let t = dc.year_fraction(valuation, expiry, None)?;
            let bp = power.block_price(m, self.block)?;
            let g = gas.price(m)?;
            let df = discount.discount(payment_date(m)?)?;
            let qty = self.mw * f64::from(bp.hours);
            let spark = bp.price - self.heat_rate * g - self.vom;
            let intrinsic = (self.kind.sign() * spark).max(0.0) * qty * df;
            let per_unit = spread_price(self.method, self.kind, bp.price, self.heat_rate * g, power_vol.vol(m), gas_vol.vol(m), rho, t, self.vom)?;
            let option = per_unit * qty * df;
            months.push(HeatRateMonth {
                month: m,
                hours: bp.hours,
                power_forward: bp.price,
                gas_forward: g,
                forward_spark: spark,
                implied_heat_rate: bp.price / g,
                time_to_expiry: t,
                discount: df,
                intrinsic_pv: intrinsic,
                option_pv: option,
            });
            intr += intrinsic;
            opt += option;
        }
        Ok(HeatRateValuation { months, intrinsic_pv: intr, option_pv: opt, extrinsic_pv: opt - intr })
    }
}

/// A tolling agreement: the toller pays a fixed capacity payment for the
/// right to call on a plant's output at its heat rate and VOM, which is a
/// strip of spark spread calls less the fixed payments.
#[derive(Debug, Clone, PartialEq)]
pub struct TollingAgreement {
    pub option: HeatRateOption,
    /// $/kW-month paid regardless of dispatch.
    pub capacity_payment: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TollingValuation {
    pub spark: HeatRateValuation,
    pub capacity_pv: f64,
    /// `spark.option_pv - capacity_pv`.
    pub net_pv: f64,
}

impl TollingAgreement {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        hub: PowerHub,
        block: Block,
        start: ContractMonth,
        end: ContractMonth,
        mw: f64,
        heat_rate: f64,
        vom: f64,
        capacity_payment: f64,
    ) -> Result<Self, PowerError> {
        if !(capacity_payment.is_finite() && capacity_payment >= 0.0) {
            return Err(PowerError::Invalid(format!("capacity payment {capacity_payment} must be non-negative")));
        }
        Ok(Self { option: HeatRateOption::new(OptionType::Call, hub, block, start, end, mw, heat_rate, vom)?, capacity_payment })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn value(
        &self,
        power: &PowerCurve,
        gas: &FuturesCurve,
        power_vol: &SeasonalVol,
        gas_vol: &SeasonalVol,
        rho: f64,
        discount: &DiscountCurve,
        dc: DayCount,
    ) -> Result<TollingValuation, PowerError> {
        let spark = self.option.value(power, gas, power_vol, gas_vol, rho, discount, dc)?;
        let mut capacity_pv = 0.0;
        for m in &spark.months {
            capacity_pv += self.capacity_payment * self.option.mw * 1000.0 * m.discount;
        }
        Ok(TollingValuation { net_pv: spark.option_pv - capacity_pv, spark, capacity_pv })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::curve::tests::pjm_curve;
    use approx::assert_abs_diff_eq;
    use fq_commod_core::{Commodity, CurveInterpolation, FuturesQuote, Hub};
    use fq_core::time::Date;
    use fq_rates::curve::Interpolation;

    fn cm(y: i32, m: u32) -> ContractMonth {
        ContractMonth::new(y, m).expect("month")
    }

    fn market() -> (PowerCurve, FuturesCurve, SeasonalVol, SeasonalVol, DiscountCurve) {
        let power = pjm_curve();
        let val = power.reference_date();
        let gas = FuturesCurve::new(
            Hub::new("TETM3", Commodity::NaturalGas, Unit::MmBtu),
            val,
            vec![
                FuturesQuote::new(cm(2027, 1), 8.0),
                FuturesQuote::new(cm(2027, 2), 7.0),
                FuturesQuote::new(cm(2027, 3), 4.0),
                FuturesQuote::new(cm(2027, 7), 3.0),
            ],
            CurveInterpolation::Linear,
        )
        .expect("gas");
        let pv = SeasonalVol::flat(cm(2027, 1), 0.80).expect("v");
        let gv = SeasonalVol::flat(cm(2027, 1), 0.90).expect("v");
        let nodes: Vec<(Date, f64)> = (1..=24).map(|k| (val.add_months(k).expect("d"), (-0.045 * k as f64 / 12.0).exp())).collect();
        let disc = DiscountCurve::new(val, &nodes, Interpolation::LogLinearDiscount).expect("disc");
        (power, gas, pv, gv, disc)
    }

    #[test]
    fn spark_strip_intrinsic_and_option() {
        let (power, gas, pv, gv, disc) = market();
        // 7.5 HR, $3 VOM, 100 MW peak, Jan-Mar.
        let hro = HeatRateOption::new(OptionType::Call, PowerHub::PjmWest, Block::Peak, cm(2027, 1), cm(2027, 3), 100.0, 7.5, 3.0).expect("hro");
        let v = hro.value(&power, &gas, &pv, &gv, 0.55, &disc, DayCount::Act365Fixed).expect("v");
        assert_eq!(v.months.len(), 3);
        // Jan: 95 - 7.5*8 - 3 = 32; Mar: 60 - 30 - 3 = 27.
        assert_abs_diff_eq!(v.months[0].forward_spark, 32.0, epsilon = 1e-12);
        assert_abs_diff_eq!(v.months[2].forward_spark, 27.0, epsilon = 1e-12);
        assert_abs_diff_eq!(v.months[0].implied_heat_rate, 95.0 / 8.0, epsilon = 1e-12);
        assert_abs_diff_eq!(v.months[0].intrinsic_pv, 32.0 * 100.0 * 320.0 * v.months[0].discount, epsilon = 1e-6);
        for m in &v.months {
            assert!(m.option_pv >= m.intrinsic_pv - 1e-9);
        }
        assert_abs_diff_eq!(v.extrinsic_pv, v.option_pv - v.intrinsic_pv, epsilon = 1e-9);
        // Out-of-the-money with a high heat rate: zero intrinsic, positive option.
        let otm = HeatRateOption::new(OptionType::Call, PowerHub::PjmWest, Block::Peak, cm(2027, 1), cm(2027, 1), 100.0, 12.0, 3.0).expect("hro");
        let vo = otm.value(&power, &gas, &pv, &gv, 0.55, &disc, DayCount::Act365Fixed).expect("v");
        assert_abs_diff_eq!(vo.intrinsic_pv, 0.0);
        assert!(vo.option_pv > 0.0);
        // Put-call parity per month on the forward spark.
        let put = HeatRateOption::new(OptionType::Put, PowerHub::PjmWest, Block::Peak, cm(2027, 1), cm(2027, 1), 100.0, 7.5, 3.0).expect("p");
        let vp = put.value(&power, &gas, &pv, &gv, 0.55, &disc, DayCount::Act365Fixed).expect("v");
        let c0 = &v.months[0];
        let p0 = &vp.months[0];
        assert_abs_diff_eq!(c0.option_pv - p0.option_pv, 32.0 * 100.0 * 320.0 * c0.discount, epsilon = 1e-6);
    }

    #[test]
    fn tolling_nets_capacity_payments() {
        let (power, gas, pv, gv, disc) = market();
        let toll = TollingAgreement::new(PowerHub::PjmWest, Block::Peak, cm(2027, 1), cm(2027, 3), 100.0, 7.5, 3.0, 4.0).expect("toll");
        let v = toll.value(&power, &gas, &pv, &gv, 0.55, &disc, DayCount::Act365Fixed).expect("v");
        let expected_cap: f64 = v.spark.months.iter().map(|m| 4.0 * 100.0 * 1000.0 * m.discount).sum();
        assert_abs_diff_eq!(v.capacity_pv, expected_cap, epsilon = 1e-6);
        assert_abs_diff_eq!(v.net_pv, v.spark.option_pv - v.capacity_pv, epsilon = 1e-9);
    }

    #[test]
    fn validation() {
        let (power, _, pv, gv, disc) = market();
        assert!(HeatRateOption::new(OptionType::Call, PowerHub::PjmWest, Block::Peak, cm(2027, 1), cm(2027, 1), 0.0, 7.5, 3.0).is_err());
        let hro = HeatRateOption::new(OptionType::Call, PowerHub::PjmWest, Block::Peak, cm(2027, 1), cm(2027, 1), 1.0, 7.5, 3.0).expect("hro");
        // Gas leg must be MMBtu.
        let oil = FuturesCurve::new(
            Hub::new("WTI", Commodity::Crude, Unit::Barrel),
            power.reference_date(),
            vec![FuturesQuote::new(cm(2027, 1), 70.0)],
            CurveInterpolation::Linear,
        )
        .expect("c");
        assert!(hro.value(&power, &oil, &pv, &gv, 0.5, &disc, DayCount::Act365Fixed).is_err());
    }
}
