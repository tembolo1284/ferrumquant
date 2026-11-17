//! Fixed-price block swaps (financial power forwards).
//!
//! A block swap exchanges a fixed $/MWh price for the realised average
//! day-ahead (or real-time) price over the block's hours in each delivery
//! month, on a fixed MW quantity. Each month's notional is `MW * block
//! hours`, which is why January 5x16 and February 5x16 of the same deal
//! have different sizes, and why the ISO calendar matters to the PV.

use fq_commod_core::ContractMonth;
use fq_core::time::Date;
use fq_rates::curve::DiscountCurve;

use crate::curve::PowerCurve;
use crate::error::PowerError;
use crate::hub::PowerHub;
use crate::iso::Block;

#[derive(Debug, Clone, PartialEq)]
pub struct BlockSwap {
    pub hub: PowerHub,
    pub block: Block,
    pub start: ContractMonth,
    pub end: ContractMonth,
    pub mw: f64,
    pub fixed_price: f64,
    /// `true`: pay fixed, receive floating (long power).
    pub pay_fixed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BlockSwapMonth {
    pub month: ContractMonth,
    pub hours: u32,
    pub mwh: f64,
    pub forward: f64,
    pub discount: f64,
    pub pv: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BlockSwapValuation {
    pub months: Vec<BlockSwapMonth>,
    pub pv: f64,
    pub total_mwh: f64,
    /// Fixed price that zeroes the PV.
    pub par_fixed: f64,
}

/// ISO settlements pay around the 20th of the following month.
pub(crate) fn payment_date(month: ContractMonth) -> Result<Date, PowerError> {
    let n = month.next();
    Ok(Date::from_ymd(n.year(), n.month(), 20)?)
}

impl BlockSwap {
    pub fn new(hub: PowerHub, block: Block, start: ContractMonth, end: ContractMonth, mw: f64, fixed_price: f64, pay_fixed: bool) -> Result<Self, PowerError> {
        if ContractMonth::range(start, end).is_empty() {
            return Err(PowerError::EmptyPeriod { start, end });
        }
        if !(mw.is_finite() && mw > 0.0) {
            return Err(PowerError::Invalid(format!("MW {mw} must be positive")));
        }
        if !fixed_price.is_finite() {
            return Err(PowerError::Invalid(format!("fixed price {fixed_price} must be finite")));
        }
        Ok(Self { hub, block, start, end, mw, fixed_price, pay_fixed })
    }

    pub fn months(&self) -> Vec<ContractMonth> {
        ContractMonth::range(self.start, self.end)
    }

    pub fn value(&self, curve: &PowerCurve, discount: &DiscountCurve) -> Result<BlockSwapValuation, PowerError> {
        if curve.hub() != self.hub {
            return Err(PowerError::WrongHub { expected: self.hub.code().into(), got: curve.hub().code().into() });
        }
        let valuation = discount.reference_date();
        let sign = if self.pay_fixed { 1.0 } else { -1.0 };
        let mut months = Vec::new();
        let (mut pv, mut total, mut wsum, mut fsum) = (0.0, 0.0, 0.0, 0.0);
        for m in self.months() {
            if m.first_day()? <= valuation {
                return Err(PowerError::MonthStarted { month: m, valuation });
            }
            let bp = curve.block_price(m, self.block)?;
            let mwh = self.mw * f64::from(bp.hours);
            let df = discount.discount(payment_date(m)?)?;
            let leg = sign * mwh * (bp.price - self.fixed_price) * df;
            months.push(BlockSwapMonth { month: m, hours: bp.hours, mwh, forward: bp.price, discount: df, pv: leg });
            pv += leg;
            total += mwh;
            wsum += mwh * df;
            fsum += mwh * df * bp.price;
        }
        Ok(BlockSwapValuation { months, pv, total_mwh: total, par_fixed: fsum / wsum })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::curve::tests::pjm_curve;
    use approx::assert_abs_diff_eq;
    use fq_rates::curve::Interpolation;

    fn cm(y: i32, m: u32) -> ContractMonth {
        ContractMonth::new(y, m).expect("month")
    }

    fn disc(val: Date) -> DiscountCurve {
        let nodes: Vec<(Date, f64)> = (1..=24).map(|k| (val.add_months(k).expect("d"), (-0.04 * k as f64 / 12.0).exp())).collect();
        DiscountCurve::new(val, &nodes, Interpolation::LogLinearDiscount).expect("disc")
    }

    #[test]
    fn q1_peak_swap() {
        let c = pjm_curve();
        let d = disc(c.reference_date());
        let swap = BlockSwap::new(PowerHub::PjmWest, Block::Peak, cm(2027, 1), cm(2027, 3), 50.0, 80.0, true).expect("swap");
        let v = swap.value(&c, &d).expect("v");
        assert_eq!(v.months.len(), 3);
        // Jan 320h, Feb 320h (20 peak days), Mar 368h (23 peak days).
        assert_eq!(v.months[0].hours, 320);
        assert_eq!(v.months[1].hours, 320);
        assert_eq!(v.months[2].hours, 368);
        assert_abs_diff_eq!(v.total_mwh, 50.0 * 1008.0, epsilon = 1e-9);
        let mut expected = 0.0;
        for (m, f, h) in [(cm(2027, 1), 95.0, 320.0), (cm(2027, 2), 85.0, 320.0), (cm(2027, 3), 60.0, 368.0)] {
            expected += 50.0 * h * (f - 80.0) * d.discount(payment_date(m).expect("pd")).expect("df");
        }
        assert_abs_diff_eq!(v.pv, expected, epsilon = 1e-6);
        let par = BlockSwap::new(PowerHub::PjmWest, Block::Peak, cm(2027, 1), cm(2027, 3), 50.0, v.par_fixed, false).expect("swap");
        assert_abs_diff_eq!(par.value(&c, &d).expect("v").pv, 0.0, epsilon = 1e-6);
    }

    #[test]
    fn validation() {
        let c = pjm_curve();
        let d = disc(c.reference_date());
        assert!(BlockSwap::new(PowerHub::PjmWest, Block::Peak, cm(2027, 3), cm(2027, 1), 1.0, 1.0, true).is_err());
        let started = BlockSwap::new(PowerHub::PjmWest, Block::Atc, cm(2026, 11), cm(2027, 1), 1.0, 1.0, true).expect("swap");
        assert!(matches!(started.value(&c, &d), Err(PowerError::MonthStarted { .. })));
        let wrong = BlockSwap::new(PowerHub::ErcotNorth, Block::Atc, cm(2027, 1), cm(2027, 1), 1.0, 1.0, true).expect("swap");
        assert!(matches!(wrong.value(&c, &d), Err(PowerError::WrongHub { .. })));
    }
}
