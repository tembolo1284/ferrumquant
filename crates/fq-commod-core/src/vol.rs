//! Volatility term structures for commodity futures.
//!
//! Two things make commodity vol different from rates/FX vol. First, vol is
//! attached to a *contract* (Jan-27 gas has its own vol), and seasonal
//! contracts carry structurally higher vol: winter gas, summer power.
//! Second, within one contract the vol rises as it approaches expiry (the
//! Samuelson effect), which is what makes storage and swing options worth
//! more than a flat-vol Black price suggests.
//!
//! [`SeasonalVol`] is the market-quoted layer: one ATM Black vol per quoted
//! contract month, interpolated in month index. [`SamuelsonVol`] is the
//! model layer: an instantaneous vol `a + b e^{-alpha tau}` in time-to-
//! expiry, integrated in closed form to give the Black vol for any expiry
//! on any contract, used by the Monte Carlo model and to extrapolate vol
//! for unquoted months.

use crate::contract::ContractMonth;
use crate::error::CommodError;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VolQuote {
    pub month: ContractMonth,
    /// ATM Black (lognormal) vol, annualised.
    pub vol: f64,
}

impl VolQuote {
    pub fn new(month: ContractMonth, vol: f64) -> Self {
        Self { month, vol }
    }
}

/// ATM vol per delivery month. Linear in month index between quotes, flat
/// outside the quoted range (vol extrapolation is a judgement call and flat
/// is the least surprising default).
#[derive(Debug, Clone, PartialEq)]
pub struct SeasonalVol {
    quotes: Vec<VolQuote>,
}

impl SeasonalVol {
    pub fn new(mut quotes: Vec<VolQuote>) -> Result<Self, CommodError> {
        if quotes.is_empty() {
            return Err(CommodError::NoQuotes);
        }
        quotes.sort_by_key(|q| q.month);
        for (i, q) in quotes.iter().enumerate() {
            if !(q.vol.is_finite() && q.vol >= 0.0) {
                return Err(CommodError::BadVol(q.vol));
            }
            if i > 0 && quotes[i - 1].month == q.month {
                return Err(CommodError::UnsortedQuotes(q.month));
            }
        }
        Ok(Self { quotes })
    }

    /// A single flat vol for every month.
    pub fn flat(month: ContractMonth, vol: f64) -> Result<Self, CommodError> {
        Self::new(vec![VolQuote::new(month, vol)])
    }

    pub fn quotes(&self) -> &[VolQuote] {
        &self.quotes
    }

    pub fn vol(&self, month: ContractMonth) -> f64 {
        let n = self.quotes.len();
        if month <= self.quotes[0].month {
            return self.quotes[0].vol;
        }
        if month >= self.quotes[n - 1].month {
            return self.quotes[n - 1].vol;
        }
        let idx = self.quotes.partition_point(|q| q.month < month);
        let (q0, q1) = (self.quotes[idx - 1], self.quotes[idx]);
        if q1.month == month {
            return q1.vol;
        }
        let w = f64::from(q0.month.months_until(month)) / f64::from(q0.month.months_until(q1.month));
        q0.vol + w * (q1.vol - q0.vol)
    }

    /// Additive vega bump to every quote.
    pub fn shifted(&self, bump: f64) -> Result<Self, CommodError> {
        Self::new(self.quotes.iter().map(|q| VolQuote::new(q.month, (q.vol + bump).max(0.0))).collect())
    }

    /// Multiply each quoted vol by a per-calendar-month factor (index 0 =
    /// January). This is how a desk layers a seasonal shape on a flat vol.
    pub fn with_seasonal_factors(&self, factors: [f64; 12]) -> Result<Self, CommodError> {
        Self::new(self.quotes.iter().map(|q| VolQuote::new(q.month, q.vol * factors[(q.month.month() - 1) as usize])).collect())
    }
}

/// Instantaneous futures vol as a function of time to the contract's expiry
/// `tau`: `sigma(tau) = long_run + (short_end - long_run) exp(-alpha tau)`.
/// `short_end` is the vol a contract shows just before expiry, `long_run`
/// the vol of a far-dated contract, `alpha` the speed at which it climbs.
/// `alpha = 0` gives a flat vol.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SamuelsonVol {
    pub long_run: f64,
    pub short_end: f64,
    pub alpha: f64,
}

impl SamuelsonVol {
    pub fn new(long_run: f64, short_end: f64, alpha: f64) -> Result<Self, CommodError> {
        for v in [long_run, short_end] {
            if !(v.is_finite() && v >= 0.0) {
                return Err(CommodError::BadVol(v));
            }
        }
        if !(alpha.is_finite() && alpha >= 0.0) {
            return Err(CommodError::Invalid(format!("Samuelson alpha {alpha} must be non-negative")));
        }
        Ok(Self { long_run, short_end, alpha })
    }

    pub fn flat(vol: f64) -> Result<Self, CommodError> {
        Self::new(vol, vol, 0.0)
    }

    /// Instantaneous vol at `tau` years before the contract's expiry.
    pub fn instantaneous(&self, tau: f64) -> f64 {
        self.long_run + (self.short_end - self.long_run) * (-self.alpha * tau.max(0.0)).exp()
    }

    /// Integrated variance of the contract expiring at `t_contract` over
    /// `[0, t_option]` (both in years from valuation), in closed form.
    pub fn integrated_variance(&self, t_option: f64, t_contract: f64) -> Result<f64, CommodError> {
        if !(t_option.is_finite() && t_contract.is_finite()) || t_option < 0.0 || t_contract < t_option {
            return Err(CommodError::Invalid(format!("need 0 <= t_option ({t_option}) <= t_contract ({t_contract})")));
        }
        let (a, b, k) = (self.long_run, self.short_end - self.long_run, self.alpha);
        if k == 0.0 {
            let s = a + b;
            return Ok(s * s * t_option);
        }
        // sigma(s) = a + b e^{-k (T - s)}, integrate sigma^2 over s in [0, Te].
        let e1 = (-k * (t_contract - t_option)).exp() - (-k * t_contract).exp();
        let e2 = (-2.0 * k * (t_contract - t_option)).exp() - (-2.0 * k * t_contract).exp();
        Ok(a * a * t_option + 2.0 * a * b * e1 / k + b * b * e2 / (2.0 * k))
    }

    /// Black implied vol for an option expiring at `t_option` on the
    /// contract expiring at `t_contract`.
    pub fn black_vol(&self, t_option: f64, t_contract: f64) -> Result<f64, CommodError> {
        if t_option == 0.0 {
            return Ok(self.instantaneous(t_contract));
        }
        Ok((self.integrated_variance(t_option, t_contract)? / t_option).sqrt())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_abs_diff_eq;

    fn cm(y: i32, m: u32) -> ContractMonth {
        ContractMonth::new(y, m).expect("month")
    }

    #[test]
    fn seasonal_vol_interpolates_and_extrapolates_flat() {
        let v = SeasonalVol::new(vec![VolQuote::new(cm(2027, 3), 0.60), VolQuote::new(cm(2027, 1), 0.90), VolQuote::new(cm(2027, 6), 0.45)]).expect("vol");
        assert_abs_diff_eq!(v.vol(cm(2027, 1)), 0.90);
        assert_abs_diff_eq!(v.vol(cm(2027, 2)), 0.75, epsilon = 1e-12);
        assert_abs_diff_eq!(v.vol(cm(2027, 4)), 0.55, epsilon = 1e-12);
        assert_abs_diff_eq!(v.vol(cm(2026, 6)), 0.90);
        assert_abs_diff_eq!(v.vol(cm(2028, 6)), 0.45);
        assert!(SeasonalVol::new(vec![VolQuote::new(cm(2027, 1), -0.1)]).is_err());
        let shaped = v.with_seasonal_factors([1.2, 1.1, 1.0, 1.0, 1.0, 0.9, 0.9, 0.9, 1.0, 1.0, 1.1, 1.2]).expect("shape");
        assert_abs_diff_eq!(shaped.vol(cm(2027, 1)), 1.08, epsilon = 1e-12);
        assert_abs_diff_eq!(shaped.vol(cm(2027, 6)), 0.405, epsilon = 1e-12);
    }

    #[test]
    fn samuelson_closed_form_matches_quadrature() {
        let s = SamuelsonVol::new(0.25, 0.80, 2.0).expect("samuelson");
        let (te, tc) = (0.75, 1.0);
        let n = 200_000;
        let h = te / n as f64;
        let mut num = 0.0;
        for i in 0..n {
            let t = (i as f64 + 0.5) * h;
            let sig = s.instantaneous(tc - t);
            num += sig * sig * h;
        }
        let cf = s.integrated_variance(te, tc).expect("var");
        assert_abs_diff_eq!(cf, num, epsilon = 1e-8);
        assert_abs_diff_eq!(s.black_vol(te, tc).expect("bv"), (cf / te).sqrt(), epsilon = 1e-14);
        // Vol rises into expiry.
        assert!(s.instantaneous(0.0) > s.instantaneous(2.0));
        assert_abs_diff_eq!(s.instantaneous(0.0), 0.80);
    }

    #[test]
    fn flat_samuelson_is_black() {
        let s = SamuelsonVol::flat(0.40).expect("flat");
        assert_abs_diff_eq!(s.black_vol(0.5, 3.0).expect("bv"), 0.40, epsilon = 1e-14);
        assert_abs_diff_eq!(s.black_vol(0.0, 3.0).expect("bv"), 0.40, epsilon = 1e-14);
        assert!(s.integrated_variance(1.5, 1.0).is_err());
    }
}
