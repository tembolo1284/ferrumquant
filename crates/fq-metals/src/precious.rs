//! Precious metals as currencies.
//!
//! Gold loco London settles spot on T+2 (London and New York both open),
//! and a forward is spot carried at the USD rate less the metal *lease
//! rate*, the yield earned by lending metal:
//!
//! ```text
//! F(T) = S * [DF_lease(T) / DF_lease(spot)] / [DF_usd(T) / DF_usd(spot)]
//! ```
//!
//! which is covered interest parity with the metal as the base currency.
//! GOFO (the gold forward offered rate) is the simple Act/360 rate implied
//! by `F / S`, approximately `USD rate - lease rate`. Options are vanilla
//! Black on the forward, discounted in USD.
//!
//! This is deliberately parallel to `fq-fx`: the market snapshot is spot
//! plus two discount curves, cloned cheaply for bumps.

use fq_commod_core::ContractMonth;
use fq_core::math::{BlackResult, OptionType, black76};
use fq_core::time::{Calendar, Date, DayCount, JoinRule, Market};
use fq_rates::curve::{DiscountCurve, Interpolation};

use crate::error::MetalsError;
use crate::metal::Metal;

#[derive(Debug, Clone, PartialEq)]
pub struct PreciousMarket {
    metal: Metal,
    trade_date: Date,
    spot_date: Date,
    /// USD per troy ounce.
    spot: f64,
    usd: DiscountCurve,
    lease: DiscountCurve,
}

/// London + New York settlement calendar for spot and delivery dates.
pub fn precious_calendar() -> Calendar {
    Calendar::joint([Market::UkSettlement, Market::UsSettlement], JoinRule::Holidays)
}

/// A flat lease-rate "discount curve" `exp(-l t)` with monthly nodes out
/// to `years`, referenced at `reference`.
pub fn flat_lease_curve(reference: Date, lease_rate: f64, years: u32) -> Result<DiscountCurve, MetalsError> {
    if !lease_rate.is_finite() {
        return Err(MetalsError::Invalid(format!("lease rate {lease_rate} must be finite")));
    }
    let mut nodes = Vec::new();
    for k in 1..=(12 * years as i32) {
        let d = reference.add_months(k)?;
        let t = DayCount::Act365Fixed.year_fraction(reference, d, None)?;
        nodes.push((d, (-lease_rate * t).exp()));
    }
    Ok(DiscountCurve::new(reference, &nodes, Interpolation::LogLinearDiscount)?)
}

impl PreciousMarket {
    /// Both curves must be referenced at `trade_date`.
    pub fn new(metal: Metal, trade_date: Date, spot: f64, usd: DiscountCurve, lease: DiscountCurve) -> Result<Self, MetalsError> {
        if !metal.is_precious() {
            return Err(MetalsError::NotPreciousMetal(metal.name()));
        }
        if !(spot.is_finite() && spot > 0.0) {
            return Err(MetalsError::Invalid(format!("spot {spot} must be positive")));
        }
        for (n, c) in [("USD", &usd), ("lease", &lease)] {
            if c.reference_date() != trade_date {
                return Err(MetalsError::Invalid(format!("{n} curve is referenced at {} but the trade date is {trade_date}", c.reference_date())));
            }
        }
        let spot_date = precious_calendar().advance(trade_date, 2)?;
        Ok(Self { metal, trade_date, spot_date, spot, usd, lease })
    }

    pub fn metal(&self) -> Metal {
        self.metal
    }

    pub fn trade_date(&self) -> Date {
        self.trade_date
    }

    pub fn spot_date(&self) -> Date {
        self.spot_date
    }

    pub fn spot(&self) -> f64 {
        self.spot
    }

    pub fn usd_curve(&self) -> &DiscountCurve {
        &self.usd
    }

    pub fn lease_curve(&self) -> &DiscountCurve {
        &self.lease
    }

    pub fn with_spot(&self, spot: f64) -> Result<Self, MetalsError> {
        Self::new(self.metal, self.trade_date, spot, self.usd.clone(), self.lease.clone())
    }

    /// Outright forward for a value date on or after spot.
    pub fn forward(&self, value_date: Date) -> Result<f64, MetalsError> {
        if value_date < self.spot_date {
            return Err(MetalsError::Invalid(format!("value date {value_date} is before spot {}", self.spot_date)));
        }
        let lease_ratio = self.lease.discount(value_date)? / self.lease.discount(self.spot_date)?;
        let usd_ratio = self.usd.discount(value_date)? / self.usd.discount(self.spot_date)?;
        Ok(self.spot * lease_ratio / usd_ratio)
    }

    /// Simple Act/360 forward rate implied by `F / S` (the GOFO convention).
    pub fn gofo(&self, value_date: Date) -> Result<f64, MetalsError> {
        let f = self.forward(value_date)?;
        let days = f64::from(value_date - self.spot_date);
        if days <= 0.0 {
            return Err(MetalsError::Invalid("GOFO needs a value date after spot".into()));
        }
        Ok((f / self.spot - 1.0) * 360.0 / days)
    }

    /// Continuously compounded USD rate from spot to `value_date`.
    pub fn usd_rate(&self, value_date: Date) -> Result<f64, MetalsError> {
        let t = DayCount::Act365Fixed.year_fraction(self.spot_date, value_date, None)?;
        if t <= 0.0 {
            return Err(MetalsError::Invalid("rate needs a value date after spot".into()));
        }
        Ok(-(self.usd.discount(value_date)? / self.usd.discount(self.spot_date)?).ln() / t)
    }

    /// Lease rate implied by a quoted outright forward, continuously
    /// compounded: `l = r - ln(F/S) / t`.
    pub fn implied_lease_rate(&self, value_date: Date, quoted_forward: f64) -> Result<f64, MetalsError> {
        if !(quoted_forward.is_finite() && quoted_forward > 0.0) {
            return Err(MetalsError::Invalid(format!("forward {quoted_forward} must be positive")));
        }
        let t = DayCount::Act365Fixed.year_fraction(self.spot_date, value_date, None)?;
        if t <= 0.0 {
            return Err(MetalsError::Invalid("lease rate needs a value date after spot".into()));
        }
        Ok(self.usd_rate(value_date)? - (quoted_forward / self.spot).ln() / t)
    }

    /// The forward for a contract month, taken at the month's delivery
    /// start, so precious metals can sit on a `fq-commod-core` curve.
    pub fn forward_for_month(&self, month: ContractMonth) -> Result<f64, MetalsError> {
        self.forward(month.first_day()?)
    }
}

/// Outright forward purchase (positive ounces) or sale (negative).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PreciousForward {
    pub metal: Metal,
    pub ounces: f64,
    pub strike: f64,
    pub value_date: Date,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PreciousForwardValuation {
    pub forward: f64,
    pub discount: f64,
    pub pv: f64,
}

impl PreciousForward {
    pub fn new(metal: Metal, ounces: f64, strike: f64, value_date: Date) -> Result<Self, MetalsError> {
        if !metal.is_precious() {
            return Err(MetalsError::NotPreciousMetal(metal.name()));
        }
        if !(ounces.is_finite() && ounces != 0.0) {
            return Err(MetalsError::Invalid(format!("ounces {ounces} must be non-zero")));
        }
        if !(strike.is_finite() && strike > 0.0) {
            return Err(MetalsError::Invalid(format!("strike {strike} must be positive")));
        }
        Ok(Self { metal, ounces, strike, value_date })
    }

    pub fn value(&self, market: &PreciousMarket) -> Result<PreciousForwardValuation, MetalsError> {
        if market.metal() != self.metal {
            return Err(MetalsError::WrongMetal { expected: self.metal.code(), got: market.metal().code().into() });
        }
        let f = market.forward(self.value_date)?;
        let df = market.usd_curve().discount(self.value_date)?;
        Ok(PreciousForwardValuation { forward: f, discount: df, pv: self.ounces * (f - self.strike) * df })
    }
}

/// European option on the metal, delivered two business days after expiry.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PreciousVanilla {
    pub kind: OptionType,
    pub metal: Metal,
    pub ounces: f64,
    pub strike: f64,
    pub expiry: Date,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PreciousVanillaValuation {
    pub delivery: Date,
    pub forward: f64,
    pub time_to_expiry: f64,
    pub discount: f64,
    /// Per-ounce Black result on the forward (undiscounted).
    pub black: BlackResult,
    pub pv: f64,
    /// Spot delta in ounces-equivalent: dPV/dS.
    pub spot_delta: f64,
}

impl PreciousVanilla {
    pub fn new(kind: OptionType, metal: Metal, ounces: f64, strike: f64, expiry: Date) -> Result<Self, MetalsError> {
        if !metal.is_precious() {
            return Err(MetalsError::NotPreciousMetal(metal.name()));
        }
        if !(ounces.is_finite() && ounces > 0.0) {
            return Err(MetalsError::Invalid(format!("ounces {ounces} must be positive")));
        }
        if !(strike.is_finite() && strike > 0.0) {
            return Err(MetalsError::Invalid(format!("strike {strike} must be positive")));
        }
        Ok(Self { kind, metal, ounces, strike, expiry })
    }

    pub fn value(&self, market: &PreciousMarket, vol: f64, dc: DayCount) -> Result<PreciousVanillaValuation, MetalsError> {
        if market.metal() != self.metal {
            return Err(MetalsError::WrongMetal { expected: self.metal.code(), got: market.metal().code().into() });
        }
        let valuation = market.trade_date();
        if self.expiry <= valuation {
            return Err(MetalsError::Expired { expiry: self.expiry, valuation });
        }
        let delivery = precious_calendar().advance(self.expiry, 2)?;
        let f = market.forward(delivery)?;
        let t = dc.year_fraction(valuation, self.expiry, None)?;
        let df = market.usd_curve().discount(delivery)?;
        let black = black76(self.kind, f, self.strike, vol, t)?;
        // dF/dS = F/S, so spot delta = forward delta * F/S.
        let spot_delta = black.delta * (f / market.spot()) * df * self.ounces;
        Ok(PreciousVanillaValuation { delivery, forward: f, time_to_expiry: t, discount: df, black, pv: black.price * df * self.ounces, spot_delta })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_abs_diff_eq;

    fn ymd(y: i32, m: u32, d: u32) -> Date {
        Date::from_ymd(y, m, d).expect("valid date")
    }

    fn usd_curve(val: Date, r: f64) -> DiscountCurve {
        let nodes: Vec<(Date, f64)> = (1..=36)
            .map(|k| {
                let d = val.add_months(k).expect("d");
                let t = DayCount::Act365Fixed.year_fraction(val, d, None).expect("t");
                (d, (-r * t).exp())
            })
            .collect();
        DiscountCurve::new(val, &nodes, Interpolation::LogLinearDiscount).expect("disc")
    }

    fn market() -> PreciousMarket {
        let val = ymd(2026, 11, 9);
        PreciousMarket::new(Metal::Gold, val, 2_400.0, usd_curve(val, 0.045), flat_lease_curve(val, 0.005, 3).expect("lease")).expect("market")
    }

    #[test]
    fn spot_date_and_forward_follow_cip() {
        let m = market();
        // Mon Nov 9 2026 + 2 London/NY business days (Nov 11 is Veterans Day) = Thu Nov 12.
        assert_eq!(m.spot_date(), ymd(2026, 11, 12));
        let one_year = ymd(2027, 11, 12);
        let f = m.forward(one_year).expect("f");
        let t = DayCount::Act365Fixed.year_fraction(m.spot_date(), one_year, None).expect("t");
        assert_abs_diff_eq!(f, 2_400.0 * ((0.045 - 0.005) * t).exp(), epsilon = 1e-6);
        // Implied lease rate round trip, GOFO roughly r - l.
        assert_abs_diff_eq!(m.implied_lease_rate(one_year, f).expect("l"), 0.005, epsilon = 1e-9);
        let gofo = m.gofo(one_year).expect("gofo");
        assert!(gofo > 0.039 && gofo < 0.042, "gofo {gofo}");
        assert!(m.forward(ymd(2026, 11, 10)).is_err());
    }

    #[test]
    fn forward_pv_and_vanilla() {
        let m = market();
        let vd = ymd(2027, 5, 12);
        let fwd = PreciousForward::new(Metal::Gold, 1_000.0, 2_400.0, vd).expect("fwd");
        let v = fwd.value(&m).expect("v");
        assert!(v.forward > 2_400.0);
        assert_abs_diff_eq!(v.pv, 1_000.0 * (v.forward - 2_400.0) * v.discount, epsilon = 1e-9);
        let short = PreciousForward::new(Metal::Gold, -1_000.0, 2_400.0, vd).expect("fwd");
        assert_abs_diff_eq!(short.value(&m).expect("v").pv, -v.pv, epsilon = 1e-9);

        let call = PreciousVanilla::new(OptionType::Call, Metal::Gold, 1_000.0, 2_450.0, ymd(2027, 5, 10)).expect("call");
        let put = PreciousVanilla::new(OptionType::Put, Metal::Gold, 1_000.0, 2_450.0, ymd(2027, 5, 10)).expect("put");
        let c = call.value(&m, 0.16, DayCount::Act365Fixed).expect("c");
        let p = put.value(&m, 0.16, DayCount::Act365Fixed).expect("p");
        assert_eq!(c.delivery, ymd(2027, 5, 12));
        assert_abs_diff_eq!(c.pv - p.pv, 1_000.0 * (c.forward - 2_450.0) * c.discount, epsilon = 1e-6);
        assert!(c.spot_delta > 0.0 && c.spot_delta < 1_000.0);
        assert!(c.black.vega > 0.0);
    }

    #[test]
    fn validation() {
        let val = ymd(2026, 11, 9);
        assert!(matches!(
            PreciousMarket::new(Metal::Copper, val, 9_000.0, usd_curve(val, 0.04), flat_lease_curve(val, 0.0, 1).expect("l")),
            Err(MetalsError::NotPreciousMetal(_))
        ));
        assert!(PreciousForward::new(Metal::Gold, 0.0, 2_400.0, val).is_err());
        let m = market();
        let silver = PreciousForward::new(Metal::Silver, 1.0, 30.0, ymd(2027, 1, 12)).expect("f");
        assert!(matches!(silver.value(&m), Err(MetalsError::WrongMetal { .. })));
    }
}
