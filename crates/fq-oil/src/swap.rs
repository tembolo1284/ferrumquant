//! Monthly-average (CFD-style) oil swaps.
//!
//! The floating leg of an oil swap is the arithmetic average of the front
//! month's daily settlement over the pricing month. Because the front month
//! rolls inside the month, the forward of that average is a mix of two
//! contracts: the one expiring during the month, weighted by the pricing
//! days up to and including its expiry, and the next one for the rest.
//! Getting that weighting right is most of what distinguishes a correct
//! oil swap mark from a naive "use the month's futures price".

use fq_commod_core::{ContractMonth, FuturesCurve};
use fq_core::time::Date;
use fq_rates::curve::DiscountCurve;

use crate::contract::{OilContract, OilMarket, front_month, pricing_days};
use crate::error::OilError;

/// Forward of the monthly average: `(average, [(contract month, pricing
/// days using it)])`.
pub fn average_forward(
    contract: OilContract,
    curve: &FuturesCurve,
    month: ContractMonth,
    market: &OilMarket,
) -> Result<(f64, Vec<(ContractMonth, usize)>), OilError> {
    check_curve(contract, curve)?;
    let days = pricing_days(contract, month, market)?;
    let mut buckets: Vec<(ContractMonth, usize)> = Vec::new();
    for d in &days {
        let fm = front_month(contract, *d, market)?;
        match buckets.last_mut() {
            Some((m, n)) if *m == fm => *n += 1,
            _ => buckets.push((fm, 1)),
        }
    }
    let mut sum = 0.0;
    for (m, n) in &buckets {
        sum += curve.price(*m)? * *n as f64;
    }
    Ok((sum / days.len() as f64, buckets))
}

pub(crate) fn check_curve(contract: OilContract, curve: &FuturesCurve) -> Result<(), OilError> {
    if curve.hub().code() != contract.code() {
        return Err(OilError::WrongContract { expected: contract.code().into(), got: curve.hub().to_string() });
    }
    Ok(())
}

/// Payment: five exchange business days after the end of the pricing month.
pub(crate) fn payment_date(contract: OilContract, month: ContractMonth, market: &OilMarket) -> Result<Date, OilError> {
    Ok(market.calendar(contract).advance(month.last_day()?, 5)?)
}

#[derive(Debug, Clone, PartialEq)]
pub struct AverageSwap {
    pub contract: OilContract,
    /// First and last pricing months.
    pub start: ContractMonth,
    pub end: ContractMonth,
    /// Quantity per month in the contract's quotation unit (bbl, gal, t).
    pub quantity: f64,
    pub fixed_price: f64,
    /// `true`: pay fixed, receive the average (long).
    pub pay_fixed: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AverageSwapMonth {
    pub month: ContractMonth,
    pub pricing_days: usize,
    /// Which contracts the average draws on and for how many days.
    pub roll: Vec<(ContractMonth, usize)>,
    pub average_forward: f64,
    pub discount: f64,
    pub pv: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AverageSwapValuation {
    pub months: Vec<AverageSwapMonth>,
    pub pv: f64,
    pub par_fixed: f64,
}

impl AverageSwap {
    pub fn new(contract: OilContract, start: ContractMonth, end: ContractMonth, quantity: f64, fixed_price: f64, pay_fixed: bool) -> Result<Self, OilError> {
        if ContractMonth::range(start, end).is_empty() {
            return Err(OilError::EmptyPeriod { start, end });
        }
        if !(quantity.is_finite() && quantity > 0.0) {
            return Err(OilError::Invalid(format!("quantity {quantity} must be positive")));
        }
        if !fixed_price.is_finite() {
            return Err(OilError::Invalid(format!("fixed price {fixed_price} must be finite")));
        }
        Ok(Self { contract, start, end, quantity, fixed_price, pay_fixed })
    }

    pub fn months(&self) -> Vec<ContractMonth> {
        ContractMonth::range(self.start, self.end)
    }

    /// Months whose pricing has not yet started are valued off the curve;
    /// a month already in progress is refused here (use the APO path with
    /// observed fixings for partially priced periods).
    pub fn value(&self, curve: &FuturesCurve, discount: &DiscountCurve, market: &OilMarket) -> Result<AverageSwapValuation, OilError> {
        check_curve(self.contract, curve)?;
        let valuation = discount.reference_date();
        let sign = if self.pay_fixed { 1.0 } else { -1.0 };
        let mut months = Vec::new();
        let (mut pv, mut wsum, mut fsum) = (0.0, 0.0, 0.0);
        for m in self.months() {
            if m.first_day()? <= valuation {
                return Err(OilError::Expired { month: m, valuation });
            }
            let (avg, roll) = average_forward(self.contract, curve, m, market)?;
            let n_days: usize = roll.iter().map(|r| r.1).sum();
            let df = discount.discount(payment_date(self.contract, m, market)?)?;
            let leg = sign * self.quantity * (avg - self.fixed_price) * df;
            months.push(AverageSwapMonth { month: m, pricing_days: n_days, roll, average_forward: avg, discount: df, pv: leg });
            pv += leg;
            wsum += self.quantity * df;
            fsum += self.quantity * df * avg;
        }
        Ok(AverageSwapValuation { months, pv, par_fixed: fsum / wsum })
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use approx::assert_abs_diff_eq;
    use fq_commod_core::{CurveInterpolation, FuturesQuote};
    use fq_rates::curve::Interpolation;

    pub(crate) fn ymd(y: i32, m: u32, d: u32) -> Date {
        Date::from_ymd(y, m, d).expect("valid date")
    }

    pub(crate) fn cm(y: i32, m: u32) -> ContractMonth {
        ContractMonth::new(y, m).expect("month")
    }

    pub(crate) fn wti_curve(val: Date) -> FuturesCurve {
        let q = vec![
            FuturesQuote::new(cm(2026, 12), 72.0),
            FuturesQuote::new(cm(2027, 1), 71.0),
            FuturesQuote::new(cm(2027, 2), 70.0),
            FuturesQuote::new(cm(2027, 3), 69.2),
            FuturesQuote::new(cm(2027, 4), 68.5),
            FuturesQuote::new(cm(2027, 5), 68.0),
            FuturesQuote::new(cm(2027, 6), 67.6),
        ];
        FuturesCurve::new(OilContract::Wti.hub(), val, q, CurveInterpolation::Linear).expect("curve")
    }

    pub(crate) fn disc(val: Date) -> DiscountCurve {
        let nodes: Vec<(Date, f64)> = (1..=24).map(|k| (val.add_months(k).expect("d"), (-0.04 * k as f64 / 12.0).exp())).collect();
        DiscountCurve::new(val, &nodes, Interpolation::LogLinearDiscount).expect("disc")
    }

    #[test]
    fn december_average_splits_at_the_jan_expiry() {
        let val = ymd(2026, 11, 9);
        let mkt = OilMarket::new();
        let (avg, roll) = average_forward(OilContract::Wti, &wti_curve(val), cm(2026, 12), &mkt).expect("avg");
        // 22 pricing days: 15 through Dec 21 on Jan-27, 7 after on Feb-27.
        assert_eq!(roll, vec![(cm(2027, 1), 15), (cm(2027, 2), 7)]);
        assert_abs_diff_eq!(avg, (15.0 * 71.0 + 7.0 * 70.0) / 22.0, epsilon = 1e-12);
    }

    #[test]
    fn swap_pv_and_par() {
        let val = ymd(2026, 11, 9);
        let mkt = OilMarket::new();
        let c = wti_curve(val);
        let d = disc(val);
        let swap = AverageSwap::new(OilContract::Wti, cm(2026, 12), cm(2027, 2), 100_000.0, 70.0, true).expect("swap");
        let v = swap.value(&c, &d, &mkt).expect("v");
        assert_eq!(v.months.len(), 3);
        let mut expected = 0.0;
        for m in &v.months {
            let df = d.discount(payment_date(OilContract::Wti, m.month, &mkt).expect("pd")).expect("df");
            assert_abs_diff_eq!(m.discount, df);
            expected += 100_000.0 * (m.average_forward - 70.0) * df;
        }
        assert_abs_diff_eq!(v.pv, expected, epsilon = 1e-6);
        let par = AverageSwap::new(OilContract::Wti, cm(2026, 12), cm(2027, 2), 100_000.0, v.par_fixed, false).expect("swap");
        assert_abs_diff_eq!(par.value(&c, &d, &mkt).expect("v").pv, 0.0, epsilon = 1e-6);
    }

    #[test]
    fn validation() {
        let val = ymd(2026, 11, 9);
        let mkt = OilMarket::new();
        let c = wti_curve(val);
        let d = disc(val);
        assert!(AverageSwap::new(OilContract::Wti, cm(2027, 2), cm(2027, 1), 1.0, 70.0, true).is_err());
        let started = AverageSwap::new(OilContract::Wti, cm(2026, 11), cm(2026, 12), 1.0, 70.0, true).expect("swap");
        assert!(matches!(started.value(&c, &d, &mkt), Err(OilError::Expired { .. })));
        let wrong = AverageSwap::new(OilContract::Brent, cm(2026, 12), cm(2026, 12), 1.0, 70.0, true).expect("swap");
        assert!(matches!(wrong.value(&c, &d, &mkt), Err(OilError::WrongContract { .. })));
    }
}
