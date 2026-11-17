//! Monthly gas swaps: fixed-for-index and basis.
//!
//! A gas swap settles once per delivery month against that month's index
//! (NYMEX last-day settle for Henry Hub, the bid-week index for any other
//! hub). Quantity is quoted per day and multiplied by the days in the
//! month, so a "10,000 MMBtu/d Cal-27" swap is twelve monthly cash flows of
//! different sizes. Each month's forward is the futures price for that
//! contract month, and the PV is the discounted sum of `qty * (F - K)`.
//!
//! A basis swap exchanges a fixed basis for the floating difference
//! `Index(hub) - NYMEX`; its forward is the basis curve, and it is how
//! every non-Henry location is actually traded.

use fq_commod_core::{BasisCurve, ContractMonth, FuturesCurve};
use fq_core::time::Date;
use fq_rates::curve::DiscountCurve;

use crate::error::GasError;
use crate::hub::{GasHub, NymexCalendar, nymex_expiry};

/// Per-month detail of a swap valuation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SwapLegValuation {
    pub month: ContractMonth,
    pub days: i32,
    pub quantity: f64,
    pub forward: f64,
    pub discount: f64,
    /// `quantity * (forward - fixed) * discount * sign`.
    pub pv: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SwapValuation {
    pub months: Vec<SwapLegValuation>,
    pub pv: f64,
    /// Fixed price that sets PV to zero (discount-weighted average forward).
    pub par_fixed: f64,
    /// Total quantity over the term.
    pub total_quantity: f64,
}

/// Payment date convention: gas settles around the 25th of the month after
/// delivery; we approximate with the 25th, clamped to the month.
fn payment_date(month: ContractMonth) -> Result<Date, GasError> {
    let next = month.next();
    let d = Date::from_ymd(next.year(), next.month(), 25)?;
    Ok(d)
}

fn check_period(start: ContractMonth, end: ContractMonth) -> Result<Vec<ContractMonth>, GasError> {
    let months = ContractMonth::range(start, end);
    if months.is_empty() {
        return Err(GasError::EmptyPeriod { start, end });
    }
    Ok(months)
}

fn value_months(
    months: &[ContractMonth],
    daily_quantity: f64,
    fixed: f64,
    sign: f64,
    forward_of: impl Fn(ContractMonth) -> Result<f64, GasError>,
    discount: &DiscountCurve,
    cal: &NymexCalendar,
) -> Result<SwapValuation, GasError> {
    let valuation = discount.reference_date();
    let mut legs = Vec::with_capacity(months.len());
    let (mut pv, mut wsum, mut fsum, mut total) = (0.0, 0.0, 0.0, 0.0);
    for &m in months {
        if nymex_expiry(m, cal)? <= valuation {
            return Err(GasError::MonthExpired { month: m, valuation });
        }
        let days = m.delivery()?.days();
        let qty = daily_quantity * f64::from(days);
        let f = forward_of(m)?;
        let df = discount.discount(payment_date(m)?)?;
        let leg_pv = sign * qty * (f - fixed) * df;
        legs.push(SwapLegValuation { month: m, days, quantity: qty, forward: f, discount: df, pv: leg_pv });
        pv += leg_pv;
        wsum += qty * df;
        fsum += qty * df * f;
        total += qty;
    }
    Ok(SwapValuation { months: legs, pv, par_fixed: fsum / wsum, total_quantity: total })
}

/// Fixed-for-floating swap on a single hub's monthly index. `pay_fixed =
/// true` is the consumer hedge (receive index, pay fixed): long the
/// forward.
#[derive(Debug, Clone, PartialEq)]
pub struct FixedFloatSwap {
    pub hub: GasHub,
    pub start: ContractMonth,
    pub end: ContractMonth,
    /// MMBtu per day.
    pub daily_quantity: f64,
    /// $/MMBtu.
    pub fixed_price: f64,
    pub pay_fixed: bool,
}

impl FixedFloatSwap {
    pub fn new(hub: GasHub, start: ContractMonth, end: ContractMonth, daily_quantity: f64, fixed_price: f64, pay_fixed: bool) -> Result<Self, GasError> {
        check_period(start, end)?;
        if !(daily_quantity.is_finite() && daily_quantity > 0.0) {
            return Err(GasError::Invalid(format!("daily quantity {daily_quantity} must be positive")));
        }
        if !fixed_price.is_finite() {
            return Err(GasError::Invalid(format!("fixed price {fixed_price} must be finite")));
        }
        Ok(Self { hub, start, end, daily_quantity, fixed_price, pay_fixed })
    }

    pub fn months(&self) -> Vec<ContractMonth> {
        ContractMonth::range(self.start, self.end)
    }

    /// `curve` must be the outright curve for `self.hub`.
    pub fn value(&self, curve: &FuturesCurve, discount: &DiscountCurve, cal: &NymexCalendar) -> Result<SwapValuation, GasError> {
        if curve.hub().code() != self.hub.code() {
            return Err(GasError::WrongHub { expected: self.hub.code().into(), got: curve.hub().to_string() });
        }
        let sign = if self.pay_fixed { 1.0 } else { -1.0 };
        value_months(&self.months(), self.daily_quantity, self.fixed_price, sign, |m| Ok(curve.price(m)?), discount, cal)
    }
}

/// Basis swap: receive `Index(hub) - NYMEX`, pay a fixed basis (or the
/// reverse). Valued off the hub's basis curve.
#[derive(Debug, Clone, PartialEq)]
pub struct BasisSwap {
    pub hub: GasHub,
    pub start: ContractMonth,
    pub end: ContractMonth,
    pub daily_quantity: f64,
    /// Fixed basis in $/MMBtu, usually negative for producing regions.
    pub fixed_basis: f64,
    /// `true`: receive floating basis, pay fixed (long basis).
    pub receive_basis: bool,
}

impl BasisSwap {
    pub fn new(hub: GasHub, start: ContractMonth, end: ContractMonth, daily_quantity: f64, fixed_basis: f64, receive_basis: bool) -> Result<Self, GasError> {
        if hub == GasHub::HenryHub {
            return Err(GasError::Invalid("a Henry Hub basis swap is a fixed-float swap; use FixedFloatSwap".into()));
        }
        check_period(start, end)?;
        if !(daily_quantity.is_finite() && daily_quantity > 0.0) {
            return Err(GasError::Invalid(format!("daily quantity {daily_quantity} must be positive")));
        }
        if !fixed_basis.is_finite() {
            return Err(GasError::Invalid(format!("fixed basis {fixed_basis} must be finite")));
        }
        Ok(Self { hub, start, end, daily_quantity, fixed_basis, receive_basis })
    }

    pub fn months(&self) -> Vec<ContractMonth> {
        ContractMonth::range(self.start, self.end)
    }

    /// `basis` must be the hub's basis curve against Henry Hub.
    pub fn value(&self, basis: &BasisCurve, discount: &DiscountCurve, cal: &NymexCalendar) -> Result<SwapValuation, GasError> {
        if basis.hub().code() != self.hub.code() {
            return Err(GasError::WrongHub { expected: self.hub.code().into(), got: basis.hub().to_string() });
        }
        if basis.benchmark().code() != GasHub::HenryHub.code() {
            return Err(GasError::WrongHub { expected: GasHub::HenryHub.code().into(), got: basis.benchmark().to_string() });
        }
        let sign = if self.receive_basis { 1.0 } else { -1.0 };
        value_months(&self.months(), self.daily_quantity, self.fixed_basis, sign, |m| Ok(basis.basis(m)?), discount, cal)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_abs_diff_eq;
    use fq_commod_core::{CurveInterpolation, FuturesQuote};
    use fq_rates::curve::Interpolation;

    fn ymd(y: i32, m: u32, d: u32) -> Date {
        Date::from_ymd(y, m, d).expect("valid date")
    }

    fn cm(y: i32, m: u32) -> ContractMonth {
        ContractMonth::new(y, m).expect("month")
    }

    fn market() -> (FuturesCurve, FuturesCurve, DiscountCurve, NymexCalendar) {
        let val = ymd(2026, 11, 9);
        let hh = FuturesCurve::new(
            GasHub::HenryHub.hub(),
            val,
            vec![FuturesQuote::new(cm(2027, 1), 4.40), FuturesQuote::new(cm(2027, 2), 4.20), FuturesQuote::new(cm(2027, 3), 3.60)],
            CurveInterpolation::Linear,
        )
        .expect("hh");
        let z6 = FuturesCurve::new(
            GasHub::TranscoZ6NY.hub(),
            val,
            vec![FuturesQuote::new(cm(2027, 1), 9.40), FuturesQuote::new(cm(2027, 2), 7.20), FuturesQuote::new(cm(2027, 3), 3.90)],
            CurveInterpolation::Linear,
        )
        .expect("z6");
        let nodes: Vec<(Date, f64)> = (1..=24).map(|k| (val.add_months(k).expect("d"), (-0.04 * k as f64 / 12.0).exp())).collect();
        let disc = DiscountCurve::new(val, &nodes, Interpolation::LogLinearDiscount).expect("disc");
        (hh, z6, disc, NymexCalendar::new())
    }

    #[test]
    fn fixed_float_swap_pv_and_par() {
        let (hh, _, disc, cal) = market();
        let swap = FixedFloatSwap::new(GasHub::HenryHub, cm(2027, 1), cm(2027, 3), 10_000.0, 4.00, true).expect("swap");
        let v = swap.value(&hh, &disc, &cal).expect("v");
        assert_eq!(v.months.len(), 3);
        assert_abs_diff_eq!(v.total_quantity, 10_000.0 * 90.0, epsilon = 1e-9);
        // Hand-computed PV.
        let mut expected = 0.0;
        for (m, f, days) in [(cm(2027, 1), 4.40, 31.0), (cm(2027, 2), 4.20, 28.0), (cm(2027, 3), 3.60, 31.0)] {
            let df = disc.discount(payment_date(m).expect("pd")).expect("df");
            expected += 10_000.0 * days * (f - 4.00) * df;
        }
        assert_abs_diff_eq!(v.pv, expected, epsilon = 1e-6);
        // Receiving fixed flips the sign; par fixed zeroes the PV.
        let rec = FixedFloatSwap::new(GasHub::HenryHub, cm(2027, 1), cm(2027, 3), 10_000.0, 4.00, false).expect("swap");
        assert_abs_diff_eq!(rec.value(&hh, &disc, &cal).expect("v").pv, -expected, epsilon = 1e-6);
        let par = FixedFloatSwap::new(GasHub::HenryHub, cm(2027, 1), cm(2027, 3), 10_000.0, v.par_fixed, true).expect("swap");
        assert_abs_diff_eq!(par.value(&hh, &disc, &cal).expect("v").pv, 0.0, epsilon = 1e-6);
        assert!(v.par_fixed > 3.60 && v.par_fixed < 4.40);
    }

    #[test]
    fn basis_swap_off_basis_curve() {
        let (hh, z6, disc, cal) = market();
        let basis = BasisCurve::from_outrights(&z6, &hh).expect("basis");
        let swap = BasisSwap::new(GasHub::TranscoZ6NY, cm(2027, 1), cm(2027, 2), 5_000.0, 3.50, true).expect("swap");
        let v = swap.value(&basis, &disc, &cal).expect("v");
        assert_abs_diff_eq!(v.months[0].forward, 5.00, epsilon = 1e-12);
        assert_abs_diff_eq!(v.months[1].forward, 3.00, epsilon = 1e-12);
        assert!(v.months[0].pv > 0.0 && v.months[1].pv < 0.0);
        // A Henry basis swap makes no sense.
        assert!(BasisSwap::new(GasHub::HenryHub, cm(2027, 1), cm(2027, 2), 1.0, 0.0, true).is_err());
        // Wrong curve is refused.
        assert!(matches!(swap.value(&BasisCurve::from_outrights(&hh, &z6).expect("b"), &disc, &cal), Err(GasError::WrongHub { .. })));
    }

    #[test]
    fn validation() {
        let (hh, _, disc, cal) = market();
        assert!(matches!(FixedFloatSwap::new(GasHub::HenryHub, cm(2027, 3), cm(2027, 1), 1.0, 4.0, true), Err(GasError::EmptyPeriod { .. })));
        let expired = FixedFloatSwap::new(GasHub::HenryHub, cm(2026, 11), cm(2027, 1), 1.0, 4.0, true).expect("swap");
        assert!(matches!(expired.value(&hh, &disc, &cal), Err(GasError::MonthExpired { .. })));
        let wrong = FixedFloatSwap::new(GasHub::Waha, cm(2027, 1), cm(2027, 2), 1.0, 4.0, true).expect("swap");
        assert!(matches!(wrong.value(&hh, &disc, &cal), Err(GasError::WrongHub { .. })));
    }
}
