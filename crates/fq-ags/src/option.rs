//! Options on agricultural futures.
//!
//! Exchange ag options are American, but like energy they are quoted and
//! risked as Black-76 Europeans on the futures price, and the early
//! exercise premium on a futures option is small. The expiry comes from
//! the exchange rule in [`crate::contract::option_expiry`]; prices and
//! strikes are in the contract's quotation unit and the PV is in dollars
//! using the lot conversion.

use fq_commod_core::{ContractMonth, FuturesCurve, SeasonalVol};
use fq_core::math::{BlackResult, OptionType, black76, shifted_black_implied_vol};
use fq_core::time::{Date, DayCount};
use fq_rates::curve::DiscountCurve;

use crate::contract::{AgCalendar, AgContract, option_expiry};
use crate::error::AgsError;

pub(crate) fn check_curve(contract: AgContract, curve: &FuturesCurve) -> Result<(), AgsError> {
    if curve.hub().code() != contract.code() {
        return Err(AgsError::WrongContract { expected: contract.code(), got: curve.hub().to_string() });
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AgOption {
    pub kind: OptionType,
    pub contract: AgContract,
    pub month: ContractMonth,
    /// In the contract's quotation unit (e.g. cents/bu).
    pub strike: f64,
    pub lots: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AgOptionValuation {
    pub expiry: Date,
    pub forward: f64,
    pub vol: f64,
    pub time_to_expiry: f64,
    pub discount: f64,
    /// Per quotation unit, undiscounted.
    pub black: BlackResult,
    /// Dollars for the whole position.
    pub pv: f64,
    /// Dollar delta per one-point move in the quoted price.
    pub delta_dollars: f64,
}

impl AgOption {
    pub fn new(kind: OptionType, contract: AgContract, month: ContractMonth, strike: f64, lots: f64) -> Result<Self, AgsError> {
        if !contract.is_listed(month) {
            return Err(AgsError::NotListed { contract: contract.code(), month });
        }
        if !(strike.is_finite() && strike > 0.0) {
            return Err(AgsError::Invalid(format!("strike {strike} must be positive")));
        }
        if !(lots.is_finite() && lots > 0.0) {
            return Err(AgsError::Invalid(format!("lots {lots} must be positive")));
        }
        Ok(Self { kind, contract, month, strike, lots })
    }

    pub fn value(
        &self,
        curve: &FuturesCurve,
        vols: &SeasonalVol,
        discount: &DiscountCurve,
        cal: &AgCalendar,
        dc: DayCount,
    ) -> Result<AgOptionValuation, AgsError> {
        check_curve(self.contract, curve)?;
        let valuation = curve.reference_date();
        let expiry = option_expiry(self.contract, self.month, cal)?;
        if expiry <= valuation {
            return Err(AgsError::Expired { expiry, valuation });
        }
        let t = dc.year_fraction(valuation, expiry, None)?;
        let f = curve.price(self.month)?;
        let vol = vols.vol(self.month);
        let df = discount.discount(expiry)?;
        let black = black76(self.kind, f, self.strike, vol, t)?;
        // One quoted point on one lot is worth to_dollars * lot_size.
        let point_value = self.contract.lot_value(1.0) * self.lots;
        Ok(AgOptionValuation {
            expiry,
            forward: f,
            vol,
            time_to_expiry: t,
            discount: df,
            black,
            pv: black.price * df * point_value,
            delta_dollars: black.delta * df * point_value,
        })
    }

    /// Black vol reproducing an undiscounted premium in quotation units.
    pub fn implied_vol(&self, curve: &FuturesCurve, premium: f64, cal: &AgCalendar, dc: DayCount) -> Result<f64, AgsError> {
        check_curve(self.contract, curve)?;
        let expiry = option_expiry(self.contract, self.month, cal)?;
        let t = dc.year_fraction(curve.reference_date(), expiry, None)?;
        Ok(shifted_black_implied_vol(self.kind, curve.price(self.month)?, self.strike, t, 0.0, premium, 10.0)?)
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

    pub(crate) fn curve(c: AgContract, val: Date, quotes: &[(ContractMonth, f64)]) -> FuturesCurve {
        FuturesCurve::new(c.hub(), val, quotes.iter().map(|&(m, p)| FuturesQuote::new(m, p)).collect(), CurveInterpolation::Linear).expect("curve")
    }

    pub(crate) fn disc(val: Date) -> DiscountCurve {
        let nodes: Vec<(Date, f64)> = (1..=24).map(|k| (val.add_months(k).expect("d"), (-0.04 * k as f64 / 12.0).exp())).collect();
        DiscountCurve::new(val, &nodes, Interpolation::LogLinearDiscount).expect("disc")
    }

    #[test]
    fn corn_call_and_implied_vol() {
        let val = ymd(2026, 11, 9);
        let cal = AgCalendar::new();
        let corn = curve(AgContract::Corn, val, &[(cm(2026, 12), 440.0), (cm(2027, 3), 452.0), (cm(2027, 5), 458.0)]);
        let vols = SeasonalVol::flat(cm(2027, 3), 0.22).expect("v");
        let opt = AgOption::new(OptionType::Call, AgContract::Corn, cm(2027, 3), 460.0, 10.0).expect("opt");
        let v = opt.value(&corn, &vols, &disc(val), &cal, DayCount::Act365Fixed).expect("v");
        assert_eq!(v.expiry, ymd(2027, 2, 19));
        assert_abs_diff_eq!(v.forward, 452.0);
        // 10 lots * 5,000 bu * $0.01 per cent = $500 per cent of premium.
        assert_abs_diff_eq!(v.pv, v.black.price * v.discount * 500.0, epsilon = 1e-9);
        assert!(v.delta_dollars > 0.0 && v.delta_dollars < 500.0);
        let iv = opt.implied_vol(&corn, v.black.price, &cal, DayCount::Act365Fixed).expect("iv");
        assert_abs_diff_eq!(iv, 0.22, epsilon = 1e-9);
        // Put-call parity in cents.
        let put = AgOption::new(OptionType::Put, AgContract::Corn, cm(2027, 3), 460.0, 10.0).expect("put");
        let p = put.value(&corn, &vols, &disc(val), &cal, DayCount::Act365Fixed).expect("p");
        assert_abs_diff_eq!(v.black.price - p.black.price, 452.0 - 460.0, epsilon = 1e-9);
    }

    #[test]
    fn validation() {
        assert!(matches!(AgOption::new(OptionType::Call, AgContract::Corn, cm(2027, 4), 450.0, 1.0), Err(AgsError::NotListed { .. })));
        let val = ymd(2026, 11, 9);
        let wheat = curve(AgContract::Wheat, val, &[(cm(2027, 3), 600.0)]);
        let opt = AgOption::new(OptionType::Call, AgContract::Corn, cm(2027, 3), 460.0, 1.0).expect("opt");
        let vols = SeasonalVol::flat(cm(2027, 3), 0.2).expect("v");
        assert!(matches!(opt.value(&wheat, &vols, &disc(val), &AgCalendar::new(), DayCount::Act365Fixed), Err(AgsError::WrongContract { .. })));
    }
}
