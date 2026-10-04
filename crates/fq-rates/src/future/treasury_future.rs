//! US Treasury futures: deliverable basket, invoice price, and basis analytics.
//!
//! Carry uses term repo financing on the dirty price (ACT/360 simple), with
//! coupons received before delivery reinvested at the same repo rate. The fair
//! futures price here is CTD-only: `min_i forward_clean_i / CF_i`, which
//! ignores the short's delivery options (quality, timing, end-of-month).

use thiserror::Error;

use fq_core::time::{BusinessDayConvention, Calendar, Date, DateError, Market};

use super::conversion_factor::{ContractError, DeliveryMonth, TreasuryContract, conversion_factor};
use crate::bond::{BondError, FixedRateBond};

#[derive(Debug, Error, Clone, PartialEq)]
pub enum FutureError {
    #[error(transparent)]
    Bond(#[from] BondError),
    #[error(transparent)]
    Contract(#[from] ContractError),
    #[error(transparent)]
    Date(#[from] DateError),
    #[error("no deliverable bonds for {contract} {delivery}")]
    EmptyBasket { contract: TreasuryContract, delivery: DeliveryMonth },
    #[error("basket index {0} out of range")]
    IndexOutOfRange(usize),
    #[error("{prices} prices given for a basket of {basket}")]
    LengthMismatch { prices: usize, basket: usize },
    #[error("settlement {settle} must not be after delivery {delivery}")]
    InvalidWindow { settle: Date, delivery: Date },
    #[error("implied repo undefined (zero financing base)")]
    DegenerateRepo,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Deliverable {
    pub bond: FixedRateBond,
    pub conversion_factor: f64,
}

/// Cash-futures basis measures for one deliverable, prices per 100 face.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BasisAnalytics {
    pub clean_price: f64,
    pub conversion_factor: f64,
    /// clean - F * CF
    pub gross_basis: f64,
    /// clean - forward clean (coupon income less financing)
    pub carry: f64,
    /// gross basis - carry = forward clean - F * CF
    pub net_basis: f64,
    pub forward_clean: f64,
    /// forward clean / CF: the futures price this bond alone implies.
    pub converted_forward: f64,
    pub implied_repo: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TreasuryFuture {
    contract: TreasuryContract,
    delivery: DeliveryMonth,
    basket: Vec<Deliverable>,
    calendar: Calendar,
}

/// Financing inputs for one bond between settlement and delivery.
struct CarryTerms {
    dirty_spot: f64,
    tau: f64,
    /// (amount, reinvestment year fraction to delivery)
    coupons: Vec<(f64, f64)>,
    accrued_at_delivery: f64,
}

impl TreasuryFuture {
    /// Keeps the candidates deliverable into `contract`/`delivery` and
    /// attaches their conversion factors, preserving input order.
    pub fn new(
        contract: TreasuryContract,
        delivery: DeliveryMonth,
        candidates: impl IntoIterator<Item = FixedRateBond>,
    ) -> Result<Self, FutureError> {
        let mut basket = Vec::new();
        for bond in candidates {
            let Some(dated) = bond.coupons().first().map(|c| c.accrual_start) else { continue };
            if contract.is_deliverable(dated, bond.maturity(), delivery)? {
                let cf = conversion_factor(contract, bond.coupon_rate(), bond.maturity(), delivery)?;
                basket.push(Deliverable { bond, conversion_factor: cf });
            }
        }
        if basket.is_empty() {
            return Err(FutureError::EmptyBasket { contract, delivery });
        }
        Ok(Self { contract, delivery, basket, calendar: Calendar::new(Market::UsGovernmentBond) })
    }

    pub fn contract(&self) -> TreasuryContract {
        self.contract
    }

    pub fn delivery_month(&self) -> DeliveryMonth {
        self.delivery
    }

    pub fn basket(&self) -> &[Deliverable] {
        &self.basket
    }

    /// First business day of the delivery month.
    pub fn first_delivery_date(&self) -> Result<Date, FutureError> {
        Ok(self.calendar.adjust(self.delivery.first_day()?, BusinessDayConvention::Following)?)
    }

    /// Last business day of the month (ZN, TN, TWE, ZB, UB), or the 3rd
    /// business day after it (ZT, Z3N, ZF).
    pub fn last_delivery_date(&self) -> Result<Date, FutureError> {
        let last_trading = self.calendar.adjust(self.delivery.last_day()?, BusinessDayConvention::Preceding)?;
        if self.contract.is_short_end() {
            Ok(self.calendar.advance(last_trading, 3)?)
        } else {
            Ok(last_trading)
        }
    }

    /// Invoice price per 100 face: F * CF + accrued at delivery.
    pub fn invoice_price(&self, index: usize, futures_price: f64, delivery_date: Date) -> Result<f64, FutureError> {
        let d = self.deliverable(index)?;
        Ok(futures_price * d.conversion_factor + d.bond.accrued(delivery_date)?)
    }

    pub fn basis(
        &self,
        index: usize,
        clean_price: f64,
        futures_price: f64,
        settle: Date,
        delivery_date: Date,
        repo: f64,
    ) -> Result<BasisAnalytics, FutureError> {
        let d = self.deliverable(index)?;
        let terms = carry_terms(&d.bond, clean_price, settle, delivery_date)?;
        let forward_clean = forward_clean(&terms, repo);
        let invoice = futures_price * d.conversion_factor + terms.accrued_at_delivery;

        // dirty (1 + r tau) = invoice + sum c_i (1 + r tau_i), solved for r.
        let coupon_sum: f64 = terms.coupons.iter().map(|(c, _)| c).sum();
        let coupon_tau: f64 = terms.coupons.iter().map(|(c, t)| c * t).sum();
        let denom = terms.dirty_spot * terms.tau - coupon_tau;
        if denom.abs() < 1e-14 {
            return Err(FutureError::DegenerateRepo);
        }
        let implied_repo = (invoice + coupon_sum - terms.dirty_spot) / denom;

        let gross_basis = clean_price - futures_price * d.conversion_factor;
        let carry = clean_price - forward_clean;
        Ok(BasisAnalytics {
            clean_price,
            conversion_factor: d.conversion_factor,
            gross_basis,
            carry,
            net_basis: gross_basis - carry,
            forward_clean,
            converted_forward: forward_clean / d.conversion_factor,
            implied_repo,
        })
    }

    /// Basis for every deliverable; returns the cheapest-to-deliver index
    /// (highest implied repo) alongside the per-bond results.
    pub fn cheapest_to_deliver(
        &self,
        clean_prices: &[f64],
        futures_price: f64,
        settle: Date,
        delivery_date: Date,
        repo: f64,
    ) -> Result<(usize, Vec<BasisAnalytics>), FutureError> {
        self.check_prices(clean_prices)?;
        let all = clean_prices
            .iter()
            .enumerate()
            .map(|(i, &p)| self.basis(i, p, futures_price, settle, delivery_date, repo))
            .collect::<Result<Vec<_>, _>>()?;
        let ctd = argmax(all.iter().map(|b| b.implied_repo));
        Ok((ctd, all))
    }

    /// CTD-only fair futures price, min_i forward_clean_i / CF_i, with the index achieving it.
    pub fn fair_price(
        &self,
        clean_prices: &[f64],
        settle: Date,
        delivery_date: Date,
        repo: f64,
    ) -> Result<(usize, f64), FutureError> {
        self.check_prices(clean_prices)?;
        let converted = clean_prices
            .iter()
            .zip(&self.basket)
            .map(|(&p, d)| {
                let terms = carry_terms(&d.bond, p, settle, delivery_date)?;
                Ok(forward_clean(&terms, repo) / d.conversion_factor)
            })
            .collect::<Result<Vec<f64>, FutureError>>()?;
        let best = argmax(converted.iter().map(|x| -x));
        Ok((best, converted[best]))
    }

    fn deliverable(&self, index: usize) -> Result<&Deliverable, FutureError> {
        self.basket.get(index).ok_or(FutureError::IndexOutOfRange(index))
    }

    fn check_prices(&self, prices: &[f64]) -> Result<(), FutureError> {
        if prices.len() != self.basket.len() {
            return Err(FutureError::LengthMismatch { prices: prices.len(), basket: self.basket.len() });
        }
        Ok(())
    }
}

fn carry_terms(bond: &FixedRateBond, clean: f64, settle: Date, delivery: Date) -> Result<CarryTerms, FutureError> {
    if settle > delivery {
        return Err(FutureError::InvalidWindow { settle, delivery });
    }
    let coupons = bond
        .coupons()
        .iter()
        .filter(|c| c.accrual_end > settle && c.accrual_end <= delivery)
        .map(|c| (c.amount, f64::from((delivery - c.payment_date).max(0)) / 360.0))
        .collect();
    Ok(CarryTerms {
        dirty_spot: clean + bond.accrued(settle)?,
        tau: f64::from(delivery - settle) / 360.0,
        coupons,
        accrued_at_delivery: bond.accrued(delivery)?,
    })
}

fn forward_clean(t: &CarryTerms, repo: f64) -> f64 {
    let reinvested: f64 = t.coupons.iter().map(|(c, tau_i)| c * (1.0 + repo * tau_i)).sum();
    t.dirty_spot * (1.0 + repo * t.tau) - reinvested - t.accrued_at_delivery
}

fn argmax(values: impl Iterator<Item = f64>) -> usize {
    values
        .enumerate()
        .fold((0, f64::NEG_INFINITY), |(bi, bv), (i, v)| if v > bv { (i, v) } else { (bi, bv) })
        .0
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_abs_diff_eq;

    type R = Result<(), FutureError>;

    fn d(y: i32, m: u32, dd: u32) -> Date {
        Date::from_ymd(y, m, dd).expect("valid test date")
    }

    fn ust(dated: Date, maturity: Date, cpn: f64) -> FixedRateBond {
        FixedRateBond::us_treasury(dated, maturity, cpn, None).expect("valid test bond")
    }

    /// ZN March 2027 with two deliverables and one bond that should drop out.
    fn zn_h27() -> Result<TreasuryFuture, FutureError> {
        TreasuryFuture::new(
            TreasuryContract::TenYear,
            DeliveryMonth::new(2027, 3)?,
            [
                ust(d(2026, 8, 15), d(2036, 8, 15), 0.0425),
                ust(d(2024, 2, 15), d(2034, 2, 15), 0.035),
                ust(d(2026, 8, 15), d(2056, 8, 15), 0.045), // 30Y: not deliverable
            ],
        )
    }

    #[test]
    fn basket_filters_and_assigns_factors() -> R {
        let f = zn_h27()?;
        assert_eq!(f.basket().len(), 2);
        assert_abs_diff_eq!(f.basket()[0].conversion_factor, 0.8771, epsilon = 1e-12);
        assert!(f.basket()[1].conversion_factor < 1.0);
        Ok(())
    }

    #[test]
    fn delivery_dates() -> R {
        let f = zn_h27()?;
        assert_eq!(f.first_delivery_date()?, d(2027, 3, 1));
        assert_eq!(f.last_delivery_date()?, d(2027, 3, 31));
        Ok(())
    }

    #[test]
    fn invoice_price() -> R {
        let f = zn_h27()?;
        let (fut, dd) = (112.5, d(2027, 3, 31));
        let expected = fut * 0.8771 + 2.125 * 44.0 / 181.0;
        assert_abs_diff_eq!(f.invoice_price(0, fut, dd)?, expected, epsilon = 1e-12);
        Ok(())
    }

    #[test]
    fn forward_price_by_hand() -> R {
        // Feb-15-2027 coupon falls in the window; it pays Feb 16 (Presidents Day).
        let f = zn_h27()?;
        let (settle, dd, repo, clean) = (d(2027, 1, 4), d(2027, 3, 31), 0.04, 99.0);
        let dirty = clean + 2.125 * 142.0 / 184.0;
        let fwd_dirty = dirty * (1.0 + repo * 86.0 / 360.0) - 2.125 * (1.0 + repo * 43.0 / 360.0);
        let expected = fwd_dirty - 2.125 * 44.0 / 181.0;
        let b = f.basis(0, clean, 112.0, settle, dd, repo)?;
        assert_abs_diff_eq!(b.forward_clean, expected, epsilon = 1e-12);
        assert!(b.carry > 0.0, "4.25% coupon vs 4% repo should carry positively");
        Ok(())
    }

    #[test]
    fn implied_repo_recovers_repo_at_fair_price() -> R {
        let f = zn_h27()?;
        let (settle, dd, repo) = (d(2027, 1, 4), d(2027, 3, 31), 0.0385);
        let prices = [99.25, 96.75];
        let (fair_idx, fair) = f.fair_price(&prices, settle, dd, repo)?;
        let (ctd, all) = f.cheapest_to_deliver(&prices, fair, settle, dd, repo)?;
        assert_eq!(ctd, fair_idx);
        assert_abs_diff_eq!(all[ctd].implied_repo, repo, epsilon = 1e-12);
        assert_abs_diff_eq!(all[ctd].net_basis, 0.0, epsilon = 1e-10);
        let other = 1 - ctd;
        assert!(all[other].implied_repo < repo);
        assert!(all[other].net_basis > 0.0);
        Ok(())
    }

    #[test]
    fn basis_identities() -> R {
        let f = zn_h27()?;
        let b = f.basis(1, 96.75, 110.0, d(2027, 1, 4), d(2027, 3, 31), 0.04)?;
        assert_abs_diff_eq!(b.gross_basis, 96.75 - 110.0 * b.conversion_factor, epsilon = 1e-12);
        assert_abs_diff_eq!(b.net_basis, b.forward_clean - 110.0 * b.conversion_factor, epsilon = 1e-12);
        Ok(())
    }

    #[test]
    fn errors() -> R {
        let f = zn_h27()?;
        assert!(matches!(f.invoice_price(5, 110.0, d(2027, 3, 31)), Err(FutureError::IndexOutOfRange(5))));
        assert!(matches!(
            f.cheapest_to_deliver(&[99.0], 110.0, d(2027, 1, 4), d(2027, 3, 31), 0.04),
            Err(FutureError::LengthMismatch { .. })
        ));
        assert!(matches!(
            f.basis(0, 99.0, 110.0, d(2027, 4, 1), d(2027, 3, 31), 0.04),
            Err(FutureError::InvalidWindow { .. })
        ));
        let empty = TreasuryFuture::new(TreasuryContract::UltraBond, DeliveryMonth::new(2027, 3)?, [ust(d(2026, 8, 15), d(2036, 8, 15), 0.04)]);
        assert!(matches!(empty, Err(FutureError::EmptyBasket { .. })));
        Ok(())
    }
}
