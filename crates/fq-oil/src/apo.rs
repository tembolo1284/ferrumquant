//! Average price options (APOs) on the monthly front-month average.
//!
//! The option settles on the same average as [`crate::swap::AverageSwap`],
//! so the fixings are the pricing days of the month, each referencing the
//! contract that is front on that day. Observed fixings (for a month
//! already in progress) are passed in and handled by the core pricer's
//! strike adjustment. Valuation is the core's moment-matched Asian with
//! the quoted vol per contract and a cross-contract correlation for the
//! days that have rolled to the next month, which for adjacent crude
//! contracts is very close to one.

use fq_commod_core::{AsianOption, AsianValuation, ContractMonth, Fixing, FuturesCurve, SeasonalVol};
use fq_core::math::OptionType;
use fq_core::time::{Date, DayCount};
use fq_rates::curve::DiscountCurve;

use crate::contract::{OilContract, OilMarket, front_month, pricing_days};
use crate::error::OilError;
use crate::swap::{check_curve, payment_date};

#[derive(Debug, Clone, PartialEq)]
pub struct AveragePriceOption {
    pub kind: OptionType,
    pub contract: OilContract,
    /// Pricing month.
    pub month: ContractMonth,
    pub strike: f64,
    /// Quantity in the contract's quotation unit.
    pub quantity: f64,
    /// Settles already published, `(date, price)`, for a month in progress.
    pub observed: Vec<(Date, f64)>,
    /// Correlation between fixings on different contract months.
    pub roll_correlation: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ApoValuation {
    pub fixings: usize,
    pub observed: usize,
    pub inner: AsianValuation,
}

impl AveragePriceOption {
    pub fn new(kind: OptionType, contract: OilContract, month: ContractMonth, strike: f64, quantity: f64) -> Result<Self, OilError> {
        if !(strike.is_finite() && strike > 0.0) {
            return Err(OilError::Invalid(format!("strike {strike} must be positive")));
        }
        if !(quantity.is_finite() && quantity > 0.0) {
            return Err(OilError::Invalid(format!("quantity {quantity} must be positive")));
        }
        Ok(Self { kind, contract, month, strike, quantity, observed: Vec::new(), roll_correlation: 0.98 })
    }

    pub fn with_observed(mut self, observed: Vec<(Date, f64)>) -> Self {
        self.observed = observed;
        self
    }

    pub fn with_roll_correlation(mut self, rho: f64) -> Result<Self, OilError> {
        if !(rho.is_finite() && (-1.0..=1.0).contains(&rho)) {
            return Err(OilError::Invalid(format!("correlation {rho} must be in [-1, 1]")));
        }
        self.roll_correlation = rho;
        Ok(self)
    }

    /// Build the fixing schedule: one per pricing day, on that day's front
    /// contract, with observed settles filled in where supplied.
    pub fn fixings(&self, market: &OilMarket) -> Result<Vec<Fixing>, OilError> {
        let days = pricing_days(self.contract, self.month, market)?;
        let mut out = Vec::with_capacity(days.len());
        for d in days {
            let fm = front_month(self.contract, d, market)?;
            let obs = self.observed.iter().find(|(od, _)| *od == d).map(|(_, p)| *p);
            out.push(match obs {
                Some(p) => Fixing::observed(d, fm, p),
                None => Fixing::pending(d, fm),
            });
        }
        Ok(out)
    }

    pub fn value(
        &self,
        curve: &FuturesCurve,
        vols: &SeasonalVol,
        discount: &DiscountCurve,
        market: &OilMarket,
        dc: DayCount,
    ) -> Result<ApoValuation, OilError> {
        check_curve(self.contract, curve)?;
        let fixings = self.fixings(market)?;
        let n = fixings.len();
        let observed = fixings.iter().filter(|f| f.observed.is_some()).count();
        let settlement = payment_date(self.contract, self.month, market)?;
        let asian = AsianOption::new(self.kind, self.strike, fixings, settlement, self.quantity)?.with_cross_contract_corr(self.roll_correlation)?;
        let inner = asian.value(curve, vols, discount, dc)?;
        Ok(ApoValuation { fixings: n, observed, inner })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::swap::tests::{cm, disc, wti_curve, ymd};
    use approx::assert_abs_diff_eq;
    use fq_core::math::black76;

    #[test]
    fn apo_uses_front_month_fixings_and_is_below_a_single_fix_option() {
        let val = ymd(2026, 11, 9);
        let mkt = OilMarket::new();
        let c = wti_curve(val);
        let d = disc(val);
        let vols = SeasonalVol::flat(cm(2027, 1), 0.35).expect("vols");
        let apo = AveragePriceOption::new(OptionType::Call, OilContract::Wti, cm(2027, 1), 70.0, 100_000.0).expect("apo");
        let fx = apo.fixings(&mkt).expect("fixings");
        // Jan 2027: 19 business days (New Year's Day and MLK Day off). Feb-27
        // expires Jan 20, so Jan 21 onwards fixes on Mar-27.
        assert_eq!(fx.len(), 19);
        assert!(fx.iter().take_while(|f| f.date <= ymd(2027, 1, 20)).all(|f| f.contract == cm(2027, 2)));
        assert!(fx.iter().filter(|f| f.date > ymd(2027, 1, 20)).all(|f| f.contract == cm(2027, 3)));
        let v = apo.value(&c, &vols, &d, &mkt, DayCount::Act365Fixed).expect("v");
        assert_eq!(v.fixings, 19);
        assert_eq!(v.observed, 0);
        // Averaging lowers the effective vol below the contract vol.
        assert!(v.inner.effective_vol < 0.35 && v.inner.effective_vol > 0.25);
        // And the price is below a European on the last fixing at the same vol.
        let t = DayCount::Act365Fixed.year_fraction(val, ymd(2027, 1, 29), None).expect("t");
        let eur = black76(OptionType::Call, v.inner.forward_average, 70.0, 0.35, t).expect("b").price;
        assert!(v.inner.forward_price < eur);
        assert!(v.inner.pv > 0.0);
    }

    #[test]
    fn observed_fixings_shrink_the_strike() {
        let val = ymd(2027, 1, 15);
        let mkt = OilMarket::new();
        let c = wti_curve(val);
        let d = disc(val);
        let vols = SeasonalVol::flat(cm(2027, 1), 0.35).expect("vols");
        let observed: Vec<(Date, f64)> =
            pricing_days(OilContract::Wti, cm(2027, 1), &mkt).expect("days").into_iter().filter(|dt| *dt <= val).map(|dt| (dt, 75.0)).collect();
        let n_obs = observed.len();
        let apo = AveragePriceOption::new(OptionType::Call, OilContract::Wti, cm(2027, 1), 70.0, 1.0).expect("apo").with_observed(observed);
        let v = apo.value(&c, &vols, &d, &mkt, DayCount::Act365Fixed).expect("v");
        assert_eq!(v.observed, n_obs);
        assert_abs_diff_eq!(v.inner.fixed_contribution, 75.0 * n_obs as f64 / 19.0, epsilon = 1e-12);
        assert_abs_diff_eq!(v.inner.effective_strike, 70.0 - 75.0 * n_obs as f64 / 19.0, epsilon = 1e-12);
    }

    #[test]
    fn validation() {
        assert!(AveragePriceOption::new(OptionType::Call, OilContract::Wti, cm(2027, 1), -1.0, 1.0).is_err());
        let apo = AveragePriceOption::new(OptionType::Call, OilContract::Wti, cm(2027, 1), 70.0, 1.0).expect("apo");
        assert!(apo.with_roll_correlation(2.0).is_err());
    }
}
