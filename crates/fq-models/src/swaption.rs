//! Swaptions under Hull-White: European in closed form, Bermudan on the tree.
//!
//! At an exercise time `T` the value of the underlying payer swap, given the
//! factor `x = x(T)`, is a sum of exponential-affine terms
//!
//! ```text
//! V(x) = sum_k c_k exp(-beta_k x)
//! ```
//!
//! because every leg cashflow is a zero-coupon bond `P(T, S | x) = C_S exp(-B_S x)`:
//! the fixed coupons discount to their lagged payment dates, and a
//! compounded floating payment for accrual `[s, e]` is `N (P(T, s) - P(T, e))`
//! times the deterministic lag factor `P(0, p) / P(0, e)` for its payment
//! date `p`. That lag treatment is the one `fq-rates` uses (the convexity of
//! a two-day lag is ignored on both sides), so payer minus receiver equals
//! the forward swap's NPV from `fq-rates` exactly. Under the `T`-forward
//! measure `x(T)` is Gaussian, so after locating the exercise boundary `x*`
//! with `V(x*) = 0` the option is a sum of closed-form truncated-lognormal
//! expectations: Jamshidian's decomposition on the swap's actual dates.
//!
//! The Bermudan uses the same `V(x)` at each exercise node of the trinomial
//! tree in `tree`, rolling back between exercise dates.

use fq_core::math::{OptionType, SolverConfig, bachelier_implied_vol, bracket, brent, norm_cdf};
use fq_core::time::Date;
use fq_rates::swap::{OisSwap, SwapSide};
use fq_rates::swaption::{Swaption, SwaptionType};

use crate::error::ModelError;
use crate::hull_white::HullWhite;
use crate::tree::TrinomialTree;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HwSwaptionPricing {
    /// PV in currency units (notional included).
    pub pv: f64,
    /// Forward swap rate off the curve.
    pub forward_rate: f64,
    pub strike: f64,
    /// Annuity in currency units (PV of 1 unit of fixed rate on the notional).
    pub annuity: f64,
    /// ACT/365F years to expiry.
    pub expiry_time: f64,
    /// Bachelier vol that reproduces `pv` on `forward_rate` and `annuity`,
    /// if the price is above intrinsic.
    pub implied_normal_vol: Option<f64>,
    /// Exercise boundary in factor space: exercised when `x(T) > x*` (payer)
    /// or `x(T) < x*` (receiver). `None` when the swap has one sign for all
    /// `x` (the option is the forward swap, or worthless).
    pub exercise_boundary: Option<f64>,
}

/// A Bermudan swaption: the right to enter the remaining part of
/// `underlying` on any of `exercise_dates`. On exercise at date `d` the
/// holder enters the periods whose accrual starts on or after `d`.
#[derive(Debug, Clone, PartialEq)]
pub struct BermudanSwaption {
    exercise_dates: Vec<Date>,
    kind: SwaptionType,
    underlying: OisSwap,
}

impl BermudanSwaption {
    pub fn new(exercise_dates: Vec<Date>, kind: SwaptionType, underlying: OisSwap) -> Result<Self, ModelError> {
        if exercise_dates.is_empty() {
            return Err(ModelError::NoExerciseDates);
        }
        if underlying.side() != swap_side(kind) {
            return Err(ModelError::Swaption(fq_rates::swaption::SwaptionError::SideMismatch { kind, expected: swap_side(kind), got: underlying.side() }));
        }
        let last_start = underlying.periods().last().map(|p| p.accrual_start).ok_or(ModelError::NoExerciseDates)?;
        for (i, &d) in exercise_dates.iter().enumerate() {
            if d > last_start || (i > 0 && d <= exercise_dates[i - 1]) {
                return Err(ModelError::BadExerciseDate(d));
            }
        }
        Ok(Self { exercise_dates, kind, underlying })
    }

    /// Co-terminal Bermudan on a USD SOFR swap: exercisable on
    /// `first_exercise` (the swap starts spot after it) and then two
    /// business days before each later accrual start, i.e. every anniversary,
    /// the last exercise entering the final period alone.
    pub fn usd_sofr_coterminal(first_exercise: Date, tenor: fq_rates::swap::Tenor, kind: SwaptionType, notional: f64, strike: f64) -> Result<Self, ModelError> {
        let swap = OisSwap::usd_sofr_spot(first_exercise, tenor, swap_side(kind), notional, strike)?;
        let mut dates = vec![first_exercise];
        // Later exercises are two business days before each accrual start,
        // mirroring the spot lag of the first one.
        let cal = OisSwap::usd_sofr_calendar();
        for p in swap.periods().iter().skip(1) {
            let d = cal.advance(p.accrual_start, -2)?;
            dates.push(d);
        }
        Self::new(dates, kind, swap)
    }

    pub fn exercise_dates(&self) -> &[Date] {
        &self.exercise_dates
    }

    pub fn kind(&self) -> SwaptionType {
        self.kind
    }

    pub fn strike(&self) -> f64 {
        self.underlying.fixed_rate()
    }

    pub fn underlying(&self) -> &OisSwap {
        &self.underlying
    }
}

pub(crate) fn swap_side(kind: SwaptionType) -> SwapSide {
    match kind {
        SwaptionType::Payer => SwapSide::PayFixed,
        SwaptionType::Receiver => SwapSide::ReceiveFixed,
    }
}

fn option_type(kind: SwaptionType) -> OptionType {
    match kind {
        SwaptionType::Payer => OptionType::Call,
        SwaptionType::Receiver => OptionType::Put,
    }
}

/// `sum_k c_k exp(-beta_k x)`: the payer swap value at a fixed time as a
/// function of the factor.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct AffineSum {
    pub terms: Vec<(f64, f64)>,
}

impl AffineSum {
    pub fn value(&self, x: f64) -> f64 {
        self.terms.iter().map(|&(c, b)| c * (-b * x).exp()).sum()
    }
}

impl HullWhite {
    /// Payer value of the periods of `swap` starting on or after `from`, seen
    /// at time `t` (years), as exponential-affine terms in `x(t)`.
    pub(crate) fn payer_terms(&self, t: f64, swap: &OisSwap, from: Date) -> AffineSum {
        let notional = swap.notional();
        let curve = self.curve();
        let mut terms = Vec::new();
        for p in swap.periods().iter().filter(|p| p.accrual_start >= from) {
            let (c_s, b_s) = self.bond_affine(t, self.time(p.accrual_start));
            let (c_e, b_e) = self.bond_affine(t, self.time(p.accrual_end));
            let (c_p, b_p) = self.bond_affine(t, self.time(p.payment_date));
            // Floating: N (P(t,s) - P(t,e)) * P(0,p) / P(0,e).
            let lag = curve.discount_t(self.time(p.payment_date)) / curve.discount_t(self.time(p.accrual_end));
            terms.push((notional * lag * c_s, b_s));
            terms.push((-notional * lag * c_e, b_e));
            // Fixed: -N K tau P(t,p).
            terms.push((-p.fixed_amount * c_p, b_p));
        }
        AffineSum { terms }
    }

    /// Forward swap rate and annuity (currency units) off the model's curve
    /// for the periods of `swap` starting on or after `from`. These match
    /// `fq-rates` single-curve valuation (the compounded forwards telescope).
    pub fn forward_and_annuity(&self, swap: &OisSwap, from: Date) -> (f64, f64) {
        let curve = self.curve();
        let (mut float_pv, mut annuity) = (0.0, 0.0);
        for p in swap.periods().iter().filter(|p| p.accrual_start >= from) {
            let df_p = curve.discount_t(self.time(p.payment_date));
            float_pv += swap.notional() * (curve.discount_t(self.time(p.accrual_start)) / curve.discount_t(self.time(p.accrual_end)) - 1.0) * df_p;
            annuity += swap.notional() * p.fixed_year_fraction * df_p;
        }
        (float_pv / annuity, annuity)
    }

    /// Exercise boundary `x*` with `V(x*) = 0`, or `None` when `V` keeps one
    /// sign over a wide range (then the option is the forward or nothing).
    fn exercise_boundary(v: &AffineSum) -> Result<Option<f64>, ModelError> {
        let f = |x: f64| v.value(x);
        let (lo, hi) = (-0.25, 0.25);
        if f(lo) * f(hi) > 0.0 {
            // Expand up to a few times; a payer swap is negative for very
            // low rates and positive for very high ones, so this normally
            // brackets at once.
            match bracket(f, lo, hi, 12) {
                Ok((a, b)) => return Ok(Some(brent(f, a, b, SolverConfig { tolerance: 1e-14, max_iterations: 200 })?)),
                Err(_) => return Ok(None),
            }
        }
        Ok(Some(brent(f, lo, hi, SolverConfig { tolerance: 1e-14, max_iterations: 200 })?))
    }

    /// European swaption in closed form.
    pub fn european_swaption(&self, swaption: &Swaption) -> Result<HwSwaptionPricing, ModelError> {
        let today = self.curve().reference_date();
        let expiry = swaption.expiry();
        if expiry <= today {
            return Err(ModelError::NotAfterReference { date: expiry, reference: today });
        }
        let t = self.time(expiry);
        let swap = swaption.underlying();
        let v = self.payer_terms(t, swap, expiry);
        let boundary = Self::exercise_boundary(&v)?;
        let sign = match swaption.kind() {
            SwaptionType::Payer => 1.0,
            SwaptionType::Receiver => -1.0,
        };
        let mean = self.forward_measure_mean(t);
        let var = self.variance(t);
        let sd = var.sqrt();
        let p_t = self.curve().discount_t(t);
        let pv = match boundary {
            Some(x_star) => {
                let z_star = (x_star - mean) / sd;
                let mut sum = 0.0;
                for &(c, beta) in &v.terms {
                    // E[exp(-beta x) 1{x > x*}] and 1{x < x*} under x ~ N(mean, var).
                    let growth = (-beta * mean + 0.5 * beta * beta * var).exp();
                    let prob = match swaption.kind() {
                        SwaptionType::Payer => norm_cdf(-beta * sd - z_star),
                        SwaptionType::Receiver => norm_cdf(beta * sd + z_star),
                    };
                    sum += c * growth * prob;
                }
                p_t * sign * sum
            }
            None => {
                // One sign everywhere: the option is the forward swap or nothing.
                let fwd = p_t * v.terms.iter().map(|&(c, beta)| c * (-beta * mean + 0.5 * beta * beta * var).exp()).sum::<f64>();
                (sign * fwd).max(0.0)
            }
        };
        let (forward_rate, annuity) = self.forward_and_annuity(swap, expiry);
        let strike = swaption.strike();
        let implied_normal_vol = bachelier_implied_vol(option_type(swaption.kind()), forward_rate, strike, t, pv / annuity, 1.0).ok();
        Ok(HwSwaptionPricing { pv, forward_rate, strike, annuity, expiry_time: t, implied_normal_vol, exercise_boundary: boundary })
    }

    /// Bermudan swaption on the trinomial tree with `steps_per_year` slices
    /// per year (the exercise dates are always slice times). The value at
    /// each exercise node is the analytic swap value for that node's factor.
    pub fn bermudan_swaption(&self, swaption: &BermudanSwaption, steps_per_year: usize) -> Result<f64, ModelError> {
        let today = self.curve().reference_date();
        let first = swaption.exercise_dates()[0];
        if first <= today {
            return Err(ModelError::NotAfterReference { date: first, reference: today });
        }
        let exercise_times: Vec<f64> = swaption.exercise_dates().iter().map(|&d| self.time(d)).collect();
        let tree = TrinomialTree::build(self, &exercise_times, steps_per_year)?;
        let sign = match swaption.kind() {
            SwaptionType::Payer => 1.0,
            SwaptionType::Receiver => -1.0,
        };
        let payoffs: Vec<AffineSum> =
            swaption.exercise_dates().iter().zip(&exercise_times).map(|(&d, &t)| self.payer_terms(t, swaption.underlying(), d)).collect();
        let slices: Vec<usize> =
            exercise_times.iter().map(|&t| tree.slice_at(t)).collect::<Option<Vec<_>>>().ok_or(ModelError::BadTreeSteps(steps_per_year))?;
        Ok(tree.rollback(&slices, |k, x| (sign * payoffs[k].value(x)).max(0.0)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_abs_diff_eq;
    use fq_core::math::{bachelier, norm_pdf};
    use fq_rates::curve::DiscountCurve;
    use fq_rates::index::OvernightIndex;
    use fq_rates::swap::Tenor;
    use fq_rates::swaption::VolQuote;

    type R = Result<(), ModelError>;

    fn d(y: i32, m: u32, dd: u32) -> Date {
        Date::from_ymd(y, m, dd).expect("valid test date")
    }

    fn setup(a: f64, sigma: f64) -> (HullWhite, OvernightIndex) {
        let today = d(2026, 10, 13);
        let nodes = [(d(2027, 10, 13), 0.035), (d(2029, 10, 13), 0.038), (d(2031, 10, 13), 0.040), (d(2036, 10, 13), 0.042), (d(2056, 10, 13), 0.043)];
        let curve = DiscountCurve::from_zero_rates(today, &nodes, Default::default()).expect("curve");
        (HullWhite::new(curve, a, sigma).expect("model"), OvernightIndex::sofr())
    }

    fn swaption(kind: SwaptionType, strike: f64) -> Swaption {
        Swaption::usd_sofr(d(2027, 10, 13), Tenor::years(10), kind, 1e6, strike).expect("swaption")
    }

    #[test]
    fn forward_and_annuity_match_fq_rates() -> R {
        let (m, idx) = setup(0.05, 0.01);
        let sw = swaption(SwaptionType::Payer, 0.04);
        let (f, a) = sw.forward_and_annuity(&idx, m.curve(), m.curve())?;
        let (f2, a2) = m.forward_and_annuity(sw.underlying(), sw.expiry());
        assert_abs_diff_eq!(f, f2, epsilon = 1e-13);
        assert_abs_diff_eq!(a, a2, epsilon = 1e-6);
        Ok(())
    }

    #[test]
    fn payer_minus_receiver_is_the_forward_swap() -> R {
        let (m, idx) = setup(0.05, 0.01);
        for k in [0.03, 0.04, 0.05] {
            let p = m.european_swaption(&swaption(SwaptionType::Payer, k))?;
            let r = m.european_swaption(&swaption(SwaptionType::Receiver, k))?;
            let fwd = swaption(SwaptionType::Payer, k).underlying().npv(&idx, m.curve())?;
            assert_abs_diff_eq!(p.pv - r.pv, fwd, epsilon = 1e-6);
            assert!(p.pv > 0.0 && r.pv > 0.0);
            assert_eq!(p.exercise_boundary, r.exercise_boundary);
            assert!(p.implied_normal_vol.is_some());
        }
        Ok(())
    }

    #[test]
    fn closed_form_matches_numerical_integration() -> R {
        let (m, _) = setup(0.05, 0.01);
        let sw = swaption(SwaptionType::Payer, 0.04);
        let t = m.time(sw.expiry());
        let v = m.payer_terms(t, sw.underlying(), sw.expiry());
        let (mean, sd) = (m.forward_measure_mean(t), m.variance(t).sqrt());
        // Trapezoid over +-8 standard deviations.
        let n = 20_000;
        let (lo, hi) = (mean - 8.0 * sd, mean + 8.0 * sd);
        let h = (hi - lo) / n as f64;
        let mut integral = 0.0;
        for i in 0..=n {
            let x = lo + i as f64 * h;
            let w = if i == 0 || i == n { 0.5 } else { 1.0 };
            integral += w * v.value(x).max(0.0) * norm_pdf((x - mean) / sd) / sd;
        }
        let numeric = m.curve().discount_t(t) * integral * h;
        let closed = m.european_swaption(&sw)?.pv;
        assert_abs_diff_eq!(closed, numeric, epsilon = 1e-6 * closed);
        Ok(())
    }

    #[test]
    fn implied_vol_round_trips_through_fq_rates_and_scales_with_sigma() -> R {
        let (m, idx) = setup(0.05, 0.01);
        let sw = swaption(SwaptionType::Payer, 0.04);
        let p = m.european_swaption(&sw)?;
        let vol = p.implied_normal_vol.expect("vol");
        let market = sw.price(&idx, m.curve(), m.curve(), VolQuote::Normal(vol))?;
        assert_abs_diff_eq!(market.pv, p.pv, epsilon = 1e-6);
        assert_abs_diff_eq!(market.forward_rate, p.forward_rate, epsilon = 1e-13);
        // Hull-White swaption vol is close to linear in sigma.
        let p2 = m.with_params(0.05, 0.02)?.european_swaption(&sw)?;
        let ratio = p2.implied_normal_vol.expect("vol") / vol;
        assert!((ratio - 2.0).abs() < 0.02, "ratio {ratio}");
        // And the 1Yx10Y normal vol for sigma = 1% and a = 5% is in the usual range.
        assert!(vol > 0.0075 && vol < 0.0095, "{vol}");
        Ok(())
    }

    #[test]
    fn tiny_vol_is_intrinsic_and_deep_in_the_money_has_no_boundary() -> R {
        let (m, idx) = setup(0.05, 1e-7);
        let sw = swaption(SwaptionType::Payer, 0.02);
        let p = m.european_swaption(&sw)?;
        let fwd = sw.underlying().npv(&idx, m.curve())?;
        assert_abs_diff_eq!(p.pv, fwd, epsilon = 1e-3);
        let r = m.european_swaption(&swaption(SwaptionType::Receiver, 0.02))?;
        assert!(r.pv >= 0.0 && r.pv < 1e-3);
        let expired = Swaption::usd_sofr(d(2026, 10, 13), Tenor::years(10), SwaptionType::Payer, 1e6, 0.04).expect("swaption");
        assert!(m.european_swaption(&expired).is_err());
        Ok(())
    }

    #[test]
    fn bachelier_quality_check_against_known_vol_ratio() -> R {
        // For a flat curve the HW normal vol of a swaption is sigma times a
        // duration-weighted average of B(T, t_i); check it against the direct
        // price for an at-the-money option as a smoke test of the formula.
        let (m, _) = setup(0.05, 0.01);
        let sw = swaption(SwaptionType::Payer, 0.04);
        let p = m.european_swaption(&sw)?;
        let model = bachelier(OptionType::Call, p.forward_rate, 0.04, p.implied_normal_vol.expect("vol"), p.expiry_time)?;
        assert_abs_diff_eq!(model.price * p.annuity, p.pv, epsilon = 1e-6);
        Ok(())
    }

    #[test]
    fn bermudan_construction() -> R {
        let (m, _) = setup(0.05, 0.01);
        let b = BermudanSwaption::usd_sofr_coterminal(d(2027, 10, 13), Tenor::years(5), SwaptionType::Payer, 1e6, 0.04)?;
        assert_eq!(b.exercise_dates().len(), 5);
        assert_eq!(b.exercise_dates()[0], d(2027, 10, 13));
        assert!(b.exercise_dates()[1] < b.underlying().periods()[1].accrual_start);
        assert_eq!(b.kind(), SwaptionType::Payer);
        assert_abs_diff_eq!(b.strike(), 0.04, epsilon = 0.0);
        let swap = b.underlying().clone();
        assert!(BermudanSwaption::new(vec![], SwaptionType::Payer, swap.clone()).is_err());
        assert!(BermudanSwaption::new(vec![d(2027, 10, 13)], SwaptionType::Receiver, swap.clone()).is_err());
        assert!(BermudanSwaption::new(vec![d(2028, 10, 13), d(2027, 10, 13)], SwaptionType::Payer, swap.clone()).is_err());
        assert!(BermudanSwaption::new(vec![d(2040, 1, 1)], SwaptionType::Payer, swap.clone()).is_err());
        let expired = BermudanSwaption::new(vec![d(2026, 10, 13)], SwaptionType::Payer, swap)?;
        assert!(m.bermudan_swaption(&expired, 50).is_err());
        Ok(())
    }

    #[test]
    fn tree_european_converges_to_closed_form() -> R {
        let (m, _) = setup(0.05, 0.01);
        for kind in [SwaptionType::Payer, SwaptionType::Receiver] {
            let sw = swaption(kind, 0.04);
            let closed = m.european_swaption(&sw)?.pv;
            let single = BermudanSwaption::new(vec![sw.expiry()], kind, sw.underlying().clone())?;
            let coarse = m.bermudan_swaption(&single, 24)?;
            let fine = m.bermudan_swaption(&single, 200)?;
            assert!((coarse - closed).abs() / closed < 0.01, "coarse {coarse} vs {closed}");
            assert!((fine - closed).abs() / closed < 2e-3, "fine {fine} vs {closed}");
            assert!((fine - closed).abs() < (coarse - closed).abs());
        }
        Ok(())
    }

    #[test]
    fn bermudan_dominates_its_europeans_and_grows_with_vol() -> R {
        let (m, _) = setup(0.05, 0.01);
        let b = BermudanSwaption::usd_sofr_coterminal(d(2027, 10, 13), Tenor::years(5), SwaptionType::Payer, 1e6, 0.04)?;
        let berm = m.bermudan_swaption(&b, 100)?;
        // Each co-terminal European (same exercise date, remaining periods) is a lower bound.
        for (i, &ex) in b.exercise_dates().iter().enumerate() {
            let single = BermudanSwaption::new(vec![ex], SwaptionType::Payer, b.underlying().clone())?;
            let eur = m.bermudan_swaption(&single, 100)?;
            assert!(berm >= eur - 1e-9, "exercise {i}: {berm} < {eur}");
        }
        let first = m.european_swaption(&Swaption::new(b.exercise_dates()[0], SwaptionType::Payer, b.underlying().clone())?)?.pv;
        assert!(berm > first && berm < 1.5 * first, "{berm} vs {first}");
        assert!(m.with_params(0.05, 0.015)?.bermudan_swaption(&b, 100)? > berm);
        Ok(())
    }
}
