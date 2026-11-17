//! LME forward curves keyed by prompt date, carries, forwards and options.
//!
//! An LME curve is quoted as cash, 3M and a strip of third-Wednesday
//! prompts, with everything in between implied by *carry*: the LME
//! convention is linear interpolation of price between quoted prompts.
//! Carries (borrowing = buy near / sell far, lending the reverse) are the
//! bread and butter of LME trading and are quoted as a price difference,
//! contango positive, backwardation negative.
//!
//! Options are European-style on the third-Wednesday prompt and expire on
//! the first Wednesday of that month (declaration day); they are priced as
//! Black-76 on the prompt forward, discounted to the prompt.

use fq_core::math::{BlackResult, OptionType, black76};
use fq_core::time::{Date, DayCount};
use fq_rates::curve::DiscountCurve;

use crate::error::MetalsError;
use crate::lme::{LmeCalendar, cash_prompt, declaration_day, is_valid_prompt, third_wednesday, three_month_prompt};
use crate::metal::Metal;

#[derive(Debug, Clone, PartialEq)]
pub struct LmeCurve {
    metal: Metal,
    trade_date: Date,
    cash: Date,
    three_month: Date,
    /// `(prompt, USD/tonne)`, strictly increasing by date.
    quotes: Vec<(Date, f64)>,
}

impl LmeCurve {
    /// `quotes` must include the cash and 3M prompts and be valid prompts
    /// for `metal` as of `trade_date`, with positive prices.
    pub fn new(metal: Metal, trade_date: Date, mut quotes: Vec<(Date, f64)>, cal: &LmeCalendar) -> Result<Self, MetalsError> {
        if metal.is_precious() {
            return Err(MetalsError::NotBaseMetal(metal.name()));
        }
        if quotes.len() < 2 {
            return Err(MetalsError::Invalid("need at least cash and 3M quotes".into()));
        }
        quotes.sort_by_key(|q| q.0);
        for w in quotes.windows(2) {
            if w[0].0 == w[1].0 {
                return Err(MetalsError::Invalid(format!("duplicate prompt {}", w[0].0)));
            }
        }
        for (d, p) in &quotes {
            if !(p.is_finite() && *p > 0.0) {
                return Err(MetalsError::Invalid(format!("price {p} at {d} must be positive")));
            }
            if !is_valid_prompt(metal, trade_date, *d, cal)? {
                return Err(MetalsError::InvalidPrompt { date: *d, trade_date });
            }
        }
        let cash = cash_prompt(trade_date, cal)?;
        let three_month = three_month_prompt(trade_date, cal)?;
        for (n, d) in [("cash", cash), ("3M", three_month)] {
            if !quotes.iter().any(|q| q.0 == d) {
                return Err(MetalsError::Invalid(format!("curve needs the {n} prompt {d}")));
            }
        }
        Ok(Self { metal, trade_date, cash, three_month, quotes })
    }

    pub fn metal(&self) -> Metal {
        self.metal
    }

    pub fn trade_date(&self) -> Date {
        self.trade_date
    }

    pub fn cash_date(&self) -> Date {
        self.cash
    }

    pub fn three_month_date(&self) -> Date {
        self.three_month
    }

    pub fn quotes(&self) -> &[(Date, f64)] {
        &self.quotes
    }

    pub fn first_prompt(&self) -> Date {
        self.quotes[0].0
    }

    pub fn last_prompt(&self) -> Date {
        self.quotes[self.quotes.len() - 1].0
    }

    /// Price for any date in range, linear in calendar days between
    /// quoted prompts (the LME carry convention). The date need not be a
    /// valid prompt; callers that trade check that separately.
    pub fn price(&self, d: Date) -> Result<f64, MetalsError> {
        if d < self.first_prompt() || d > self.last_prompt() {
            return Err(MetalsError::OutOfRange(d));
        }
        let idx = self.quotes.partition_point(|q| q.0 < d);
        let (d1, p1) = self.quotes[idx];
        if d1 == d {
            return Ok(p1);
        }
        let (d0, p0) = self.quotes[idx - 1];
        let w = f64::from(d - d0) / f64::from(d1 - d0);
        Ok(p0 + w * (p1 - p0))
    }

    pub fn cash(&self) -> Result<f64, MetalsError> {
        self.price(self.cash)
    }

    pub fn three_month(&self) -> Result<f64, MetalsError> {
        self.price(self.three_month)
    }

    /// `price(to) - price(from)`: positive is contango.
    pub fn carry(&self, from: Date, to: Date) -> Result<f64, MetalsError> {
        if to <= from {
            return Err(MetalsError::Invalid(format!("carry needs from ({from}) before to ({to})")));
        }
        Ok(self.price(to)? - self.price(from)?)
    }

    /// Cash-to-3M carry, the headline "C/B" number.
    pub fn cash_to_3m(&self) -> Result<f64, MetalsError> {
        self.carry(self.cash, self.three_month)
    }

    /// Annualised simple rate implied by a carry: `carry / price(from) *
    /// 365 / days`. Compare to USD rates plus storage to spot rich or
    /// cheap carries.
    pub fn implied_carry_rate(&self, from: Date, to: Date) -> Result<f64, MetalsError> {
        let c = self.carry(from, to)?;
        Ok(c / self.price(from)? * 365.0 / f64::from(to - from))
    }

    pub fn is_contango(&self) -> Result<bool, MetalsError> {
        Ok(self.cash_to_3m()? > 0.0)
    }

    /// Parallel bump by an absolute $/t amount.
    pub fn shifted(&self, bump: f64, cal: &LmeCalendar) -> Result<Self, MetalsError> {
        Self::new(self.metal, self.trade_date, self.quotes.iter().map(|(d, p)| (*d, p + bump)).collect(), cal)
    }

    /// Bump the cash-to-3M carry by moving every prompt after cash by
    /// `bump * (days from cash) / (days cash to 3M)`, so 3M moves by
    /// exactly `bump` and the back of the curve moves with it.
    pub fn carry_bumped(&self, bump: f64, cal: &LmeCalendar) -> Result<Self, MetalsError> {
        let span = f64::from(self.three_month - self.cash);
        let quotes = self.quotes.iter().map(|(d, p)| (*d, p + bump * f64::from(*d - self.cash) / span)).collect();
        Self::new(self.metal, self.trade_date, quotes, cal)
    }
}

/// Outright forward for a prompt date. Positive tonnes is a purchase.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LmeForward {
    pub metal: Metal,
    pub tonnes: f64,
    pub price: f64,
    pub prompt: Date,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LmeForwardValuation {
    pub forward: f64,
    pub discount: f64,
    pub pv: f64,
}

impl LmeForward {
    pub fn new(metal: Metal, tonnes: f64, price: f64, prompt: Date) -> Result<Self, MetalsError> {
        if metal.is_precious() {
            return Err(MetalsError::NotBaseMetal(metal.name()));
        }
        if !(tonnes.is_finite() && tonnes != 0.0) {
            return Err(MetalsError::Invalid(format!("tonnes {tonnes} must be non-zero")));
        }
        if !(price.is_finite() && price > 0.0) {
            return Err(MetalsError::Invalid(format!("price {price} must be positive")));
        }
        Ok(Self { metal, tonnes, price, prompt })
    }

    pub fn value(&self, curve: &LmeCurve, discount: &DiscountCurve, cal: &LmeCalendar) -> Result<LmeForwardValuation, MetalsError> {
        if curve.metal() != self.metal {
            return Err(MetalsError::WrongMetal { expected: self.metal.code(), got: curve.metal().code().into() });
        }
        if !is_valid_prompt(self.metal, curve.trade_date(), self.prompt, cal)? {
            return Err(MetalsError::InvalidPrompt { date: self.prompt, trade_date: curve.trade_date() });
        }
        let f = curve.price(self.prompt)?;
        let df = discount.discount(self.prompt)?;
        Ok(LmeForwardValuation { forward: f, discount: df, pv: self.tonnes * (f - self.price) * df })
    }
}

/// A carry trade: buy `near`, sell `far` (a *borrow*) when `tonnes > 0`;
/// the reverse (a *lend*) when negative.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LmeCarry {
    pub metal: Metal,
    pub tonnes: f64,
    pub near: Date,
    pub near_price: f64,
    pub far: Date,
    pub far_price: f64,
}

impl LmeCarry {
    pub fn new(metal: Metal, tonnes: f64, near: Date, near_price: f64, far: Date, far_price: f64) -> Result<Self, MetalsError> {
        if far <= near {
            return Err(MetalsError::Invalid(format!("far prompt {far} must be after near {near}")));
        }
        LmeForward::new(metal, tonnes, near_price, near)?;
        LmeForward::new(metal, -tonnes, far_price, far)?;
        Ok(Self { metal, tonnes, near, near_price, far, far_price })
    }

    /// The traded carry, `far_price - near_price`.
    pub fn traded_carry(&self) -> f64 {
        self.far_price - self.near_price
    }

    pub fn value(&self, curve: &LmeCurve, discount: &DiscountCurve, cal: &LmeCalendar) -> Result<f64, MetalsError> {
        let near = LmeForward::new(self.metal, self.tonnes, self.near_price, self.near)?.value(curve, discount, cal)?;
        let far = LmeForward::new(self.metal, -self.tonnes, self.far_price, self.far)?.value(curve, discount, cal)?;
        Ok(near.pv + far.pv)
    }
}

/// Option on the third-Wednesday prompt of `year`/`month`, declared on the
/// first Wednesday.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LmeOption {
    pub kind: OptionType,
    pub metal: Metal,
    pub tonnes: f64,
    pub strike: f64,
    pub year: i32,
    pub month: u32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LmeOptionValuation {
    pub expiry: Date,
    pub prompt: Date,
    pub forward: f64,
    pub time_to_expiry: f64,
    pub discount: f64,
    pub black: BlackResult,
    pub pv: f64,
}

impl LmeOption {
    pub fn new(kind: OptionType, metal: Metal, tonnes: f64, strike: f64, year: i32, month: u32) -> Result<Self, MetalsError> {
        if metal.is_precious() {
            return Err(MetalsError::NotBaseMetal(metal.name()));
        }
        if !(tonnes.is_finite() && tonnes > 0.0) {
            return Err(MetalsError::Invalid(format!("tonnes {tonnes} must be positive")));
        }
        if !(strike.is_finite() && strike > 0.0) {
            return Err(MetalsError::Invalid(format!("strike {strike} must be positive")));
        }
        if !(1..=12).contains(&month) {
            return Err(MetalsError::Invalid(format!("month {month} out of range")));
        }
        Ok(Self { kind, metal, tonnes, strike, year, month })
    }

    pub fn value(&self, curve: &LmeCurve, vol: f64, discount: &DiscountCurve, cal: &LmeCalendar, dc: DayCount) -> Result<LmeOptionValuation, MetalsError> {
        if curve.metal() != self.metal {
            return Err(MetalsError::WrongMetal { expected: self.metal.code(), got: curve.metal().code().into() });
        }
        let valuation = curve.trade_date();
        let expiry = declaration_day(self.year, self.month, cal)?;
        if expiry <= valuation {
            return Err(MetalsError::Expired { expiry, valuation });
        }
        let prompt = third_wednesday(self.year, self.month, cal)?;
        let f = curve.price(prompt)?;
        let t = dc.year_fraction(valuation, expiry, None)?;
        let df = discount.discount(prompt)?;
        let black = black76(self.kind, f, self.strike, vol, t)?;
        Ok(LmeOptionValuation { expiry, prompt, forward: f, time_to_expiry: t, discount: df, black, pv: black.price * df * self.tonnes })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_abs_diff_eq;
    use fq_rates::curve::Interpolation;

    fn ymd(y: i32, m: u32, d: u32) -> Date {
        Date::from_ymd(y, m, d).expect("valid date")
    }

    fn disc(val: Date) -> DiscountCurve {
        let nodes: Vec<(Date, f64)> = (1..=36).map(|k| (val.add_months(k).expect("d"), (-0.045 * k as f64 / 12.0).exp())).collect();
        DiscountCurve::new(val, &nodes, Interpolation::LogLinearDiscount).expect("disc")
    }

    /// Copper as of Mon Nov 9 2026: cash Nov 11, 3M Feb 9 2027, then third
    /// Wednesdays, in a gentle contango.
    fn copper(cal: &LmeCalendar) -> LmeCurve {
        let td = ymd(2026, 11, 9);
        let q = vec![
            (ymd(2026, 11, 11), 9_000.0),
            (ymd(2027, 2, 9), 9_060.0),
            (ymd(2027, 3, 17), 9_080.0),
            (ymd(2027, 6, 16), 9_130.0),
            (ymd(2027, 12, 15), 9_220.0),
        ];
        LmeCurve::new(Metal::Copper, td, q, cal).expect("curve")
    }

    #[test]
    fn curve_interpolates_linearly_and_reports_carries() {
        let cal = LmeCalendar::new();
        let c = copper(&cal);
        assert_abs_diff_eq!(c.cash().expect("c"), 9_000.0);
        assert_abs_diff_eq!(c.three_month().expect("3m"), 9_060.0);
        assert_abs_diff_eq!(c.cash_to_3m().expect("cb"), 60.0, epsilon = 1e-12);
        assert!(c.is_contango().expect("ct"));
        // Dec 10 2026 is 29 of the 90 days from cash to 3M.
        let p = c.price(ymd(2026, 12, 10)).expect("p");
        assert_abs_diff_eq!(p, 9_000.0 + 60.0 * 29.0 / 90.0, epsilon = 1e-9);
        // Implied carry rate: 60/9000 * 365/90 = 2.7%.
        assert_abs_diff_eq!(c.implied_carry_rate(c.cash_date(), c.three_month_date()).expect("r"), 60.0 / 9_000.0 * 365.0 / 90.0, epsilon = 1e-12);
        assert!(matches!(c.price(ymd(2026, 11, 10)), Err(MetalsError::OutOfRange(_))));
        // Bumps.
        let up = c.shifted(10.0, &cal).expect("shift");
        assert_abs_diff_eq!(up.cash().expect("c"), 9_010.0);
        let steeper = c.carry_bumped(20.0, &cal).expect("cb");
        assert_abs_diff_eq!(steeper.cash().expect("c"), 9_000.0);
        assert_abs_diff_eq!(steeper.cash_to_3m().expect("cb"), 80.0, epsilon = 1e-9);
    }

    #[test]
    fn curve_validation() {
        let cal = LmeCalendar::new();
        let td = ymd(2026, 11, 9);
        // Missing 3M.
        assert!(LmeCurve::new(Metal::Copper, td, vec![(ymd(2026, 11, 11), 9_000.0), (ymd(2027, 3, 17), 9_080.0)], &cal).is_err());
        // Invalid prompt (a Thursday beyond 3M).
        assert!(matches!(
            LmeCurve::new(Metal::Copper, td, vec![(ymd(2026, 11, 11), 9_000.0), (ymd(2027, 2, 9), 9_060.0), (ymd(2027, 3, 11), 9_070.0)], &cal),
            Err(MetalsError::InvalidPrompt { .. })
        ));
        assert!(matches!(LmeCurve::new(Metal::Gold, td, vec![], &cal), Err(MetalsError::NotBaseMetal(_))));
    }

    #[test]
    fn forward_carry_and_option() {
        let cal = LmeCalendar::new();
        let c = copper(&cal);
        let d = disc(c.trade_date());
        let fwd = LmeForward::new(Metal::Copper, 100.0, 9_050.0, ymd(2027, 3, 17)).expect("fwd");
        let v = fwd.value(&c, &d, &cal).expect("v");
        assert_abs_diff_eq!(v.forward, 9_080.0);
        assert_abs_diff_eq!(v.pv, 100.0 * 30.0 * v.discount, epsilon = 1e-9);
        // A Thursday prompt beyond 3M is refused.
        let bad = LmeForward::new(Metal::Copper, 100.0, 9_050.0, ymd(2027, 3, 11)).expect("fwd");
        assert!(matches!(bad.value(&c, &d, &cal), Err(MetalsError::InvalidPrompt { .. })));

        // Borrow cash/3M (buy cash, sell 3M) receiving a 50 contango when
        // the market pays 60: the far leg was sold 10/t too cheap.
        let carry = LmeCarry::new(Metal::Copper, 25.0, c.cash_date(), 9_000.0, c.three_month_date(), 9_050.0).expect("carry");
        assert_abs_diff_eq!(carry.traded_carry(), 50.0);
        let pv = carry.value(&c, &d, &cal).expect("pv");
        let df_3m = d.discount(c.three_month_date()).expect("df");
        assert_abs_diff_eq!(pv, -25.0 * (9_060.0 - 9_050.0) * df_3m, epsilon = 1e-9);
        // The lender on the other side gains the same.
        let lend = LmeCarry::new(Metal::Copper, -25.0, c.cash_date(), 9_000.0, c.three_month_date(), 9_050.0).expect("lend");
        assert_abs_diff_eq!(lend.value(&c, &d, &cal).expect("pv"), -pv, epsilon = 1e-9);

        // Jun-27 option: declared Jun 2, on the Jun 16 prompt at 9,130.
        let opt = LmeOption::new(OptionType::Call, Metal::Copper, 25.0, 9_200.0, 2027, 6).expect("opt");
        let ov = opt.value(&c, 0.22, &d, &cal, DayCount::Act365Fixed).expect("ov");
        assert_eq!(ov.expiry, ymd(2027, 6, 2));
        assert_eq!(ov.prompt, ymd(2027, 6, 16));
        assert_abs_diff_eq!(ov.forward, 9_130.0);
        assert!(ov.pv > 0.0);
        let put = LmeOption::new(OptionType::Put, Metal::Copper, 25.0, 9_200.0, 2027, 6).expect("put");
        let pv_ = put.value(&c, 0.22, &d, &cal, DayCount::Act365Fixed).expect("pv");
        assert_abs_diff_eq!(ov.pv - pv_.pv, 25.0 * (9_130.0 - 9_200.0) * ov.discount, epsilon = 1e-6);
    }
}
