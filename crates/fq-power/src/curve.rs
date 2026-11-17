//! A hub's forward curve: peak and off-peak by delivery month.
//!
//! Power is quoted as two monthly strips at each hub, on-peak and off-peak,
//! and everything else is derived from them: ATC is the hours-weighted
//! blend, and the 2x16 and 7x8 sub-blocks are taken flat at the off-peak
//! price unless a desk marks them separately. Hour weights come from the
//! hub's ISO calendar, so a 23-hour March Sunday and a 25-hour November
//! Sunday are counted correctly.

use fq_commod_core::{ContractMonth, DeliveryPeriod, FuturesCurve};
use fq_core::time::Date;

use crate::error::PowerError;
use crate::hub::PowerHub;
use crate::iso::{Block, block_hours};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BlockPrice {
    pub month: ContractMonth,
    pub block: Block,
    pub hours: u32,
    pub price: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PowerCurve {
    hub: PowerHub,
    peak: FuturesCurve,
    offpeak: FuturesCurve,
}

impl PowerCurve {
    /// Both curves must carry `hub.hub()` and share a reference date.
    pub fn new(hub: PowerHub, peak: FuturesCurve, offpeak: FuturesCurve) -> Result<Self, PowerError> {
        for c in [&peak, &offpeak] {
            if c.hub().code() != hub.code() {
                return Err(PowerError::WrongHub { expected: hub.code().into(), got: c.hub().to_string() });
            }
        }
        if peak.reference_date() != offpeak.reference_date() {
            return Err(PowerError::Invalid(format!("peak curve dated {} but off-peak {}", peak.reference_date(), offpeak.reference_date())));
        }
        Ok(Self { hub, peak, offpeak })
    }

    pub fn hub(&self) -> PowerHub {
        self.hub
    }

    pub fn reference_date(&self) -> Date {
        self.peak.reference_date()
    }

    pub fn peak(&self) -> &FuturesCurve {
        &self.peak
    }

    pub fn offpeak(&self) -> &FuturesCurve {
        &self.offpeak
    }

    /// Hours of `block` in a delivery month at this hub's ISO.
    pub fn hours(&self, month: ContractMonth, block: Block) -> Result<u32, PowerError> {
        Ok(block_hours(self.hub.iso(), block, month.delivery()?))
    }

    /// Forward price of a block for one delivery month.
    pub fn block_price(&self, month: ContractMonth, block: Block) -> Result<BlockPrice, PowerError> {
        let hours = self.hours(month, block)?;
        let price = match block {
            Block::Peak => self.peak.price(month)?,
            Block::OffPeak | Block::Wrap2x16 | Block::Nights7x8 => self.offpeak.price(month)?,
            Block::Atc => {
                let hp = f64::from(self.hours(month, Block::Peak)?);
                let ho = f64::from(self.hours(month, Block::OffPeak)?);
                (hp * self.peak.price(month)? + ho * self.offpeak.price(month)?) / (hp + ho)
            }
        };
        Ok(BlockPrice { month, block, hours, price })
    }

    /// Hours-weighted block price over an arbitrary delivery period, e.g. a
    /// Q3 or Cal strip. Partial months are weighted by the hours in the
    /// portion of the month actually delivered.
    pub fn period_price(&self, period: DeliveryPeriod, block: Block) -> Result<(u32, f64), PowerError> {
        let iso = self.hub.iso();
        let mut hours_total = 0u32;
        let mut value = 0.0;
        for m in period.months() {
            let ms = m.first_day()?;
            let me = m.last_day()?;
            let lo = if ms > period.start() { ms } else { period.start() };
            let hi = if me < period.end() { me } else { period.end() };
            let sub = DeliveryPeriod::new(lo, hi)?;
            let h = block_hours(iso, block, sub);
            if h == 0 {
                continue;
            }
            let p = match block {
                Block::Atc => {
                    let hp = f64::from(block_hours(iso, Block::Peak, sub));
                    let ho = f64::from(block_hours(iso, Block::OffPeak, sub));
                    (hp * self.peak.price(m)? + ho * self.offpeak.price(m)?) / (hp + ho)
                }
                _ => self.block_price(m, block)?.price,
            };
            hours_total += h;
            value += f64::from(h) * p;
        }
        if hours_total == 0 {
            return Err(PowerError::Invalid(format!("no {} hours in {}", block.code(), period)));
        }
        Ok((hours_total, value / f64::from(hours_total)))
    }

    /// Parallel bump of both strips by an absolute $/MWh amount.
    pub fn shifted(&self, bump: f64) -> Result<Self, PowerError> {
        Ok(Self { hub: self.hub, peak: self.peak.shifted(bump)?, offpeak: self.offpeak.shifted(bump)? })
    }

    /// Bump only one strip, for peak/off-peak split deltas.
    pub fn shifted_block(&self, block: Block, bump: f64) -> Result<Self, PowerError> {
        match block {
            Block::Peak => Ok(Self { hub: self.hub, peak: self.peak.shifted(bump)?, offpeak: self.offpeak.clone() }),
            Block::OffPeak => Ok(Self { hub: self.hub, peak: self.peak.clone(), offpeak: self.offpeak.shifted(bump)? }),
            other => Err(PowerError::Invalid(format!("{} is not a quoted strip; bump Peak or OffPeak", other.code()))),
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use approx::assert_abs_diff_eq;
    use fq_commod_core::{CurveInterpolation, FuturesQuote};

    fn ymd(y: i32, m: u32, d: u32) -> Date {
        Date::from_ymd(y, m, d).expect("valid date")
    }

    fn cm(y: i32, m: u32) -> ContractMonth {
        ContractMonth::new(y, m).expect("month")
    }

    pub(crate) fn pjm_curve() -> PowerCurve {
        let val = ymd(2026, 11, 9);
        let hub = PowerHub::PjmWest;
        let peak = FuturesCurve::new(
            hub.hub(),
            val,
            vec![
                FuturesQuote::new(cm(2027, 1), 95.0),
                FuturesQuote::new(cm(2027, 2), 85.0),
                FuturesQuote::new(cm(2027, 3), 60.0),
                FuturesQuote::new(cm(2027, 7), 80.0),
            ],
            CurveInterpolation::Linear,
        )
        .expect("peak");
        let off = FuturesCurve::new(
            hub.hub(),
            val,
            vec![
                FuturesQuote::new(cm(2027, 1), 60.0),
                FuturesQuote::new(cm(2027, 2), 55.0),
                FuturesQuote::new(cm(2027, 3), 40.0),
                FuturesQuote::new(cm(2027, 7), 45.0),
            ],
            CurveInterpolation::Linear,
        )
        .expect("off");
        PowerCurve::new(hub, peak, off).expect("curve")
    }

    #[test]
    fn atc_is_hours_weighted_blend() {
        let c = pjm_curve();
        let jan = c.block_price(cm(2027, 1), Block::Atc).expect("atc");
        assert_eq!(jan.hours, 744);
        let expected = (320.0 * 95.0 + 424.0 * 60.0) / 744.0;
        assert_abs_diff_eq!(jan.price, expected, epsilon = 1e-12);
        assert_eq!(c.block_price(cm(2027, 1), Block::Peak).expect("pk").hours, 320);
        assert_abs_diff_eq!(c.block_price(cm(2027, 1), Block::Nights7x8).expect("n").price, 60.0);
        // Interpolated month (Apr between Mar and Jul).
        assert_abs_diff_eq!(c.block_price(cm(2027, 4), Block::Peak).expect("pk").price, 65.0, epsilon = 1e-12);
    }

    #[test]
    fn period_price_weights_partial_months_by_hours() {
        let c = pjm_curve();
        // Jan 16 - Feb 15 peak: Jan 16-31 has peak days Mon-Fri: 18,19,20,21,22,25,26,27,28,29 = 10 days;
        // Feb 1-15: 1-5, 8-12, 15 = 11 days.
        let p = DeliveryPeriod::new(ymd(2027, 1, 16), ymd(2027, 2, 15)).expect("p");
        let (h, price) = c.period_price(p, Block::Peak).expect("pp");
        assert_eq!(h, (10 + 11) * 16);
        assert_abs_diff_eq!(price, (160.0 * 95.0 + 176.0 * 85.0) / 336.0, epsilon = 1e-12);
        // Q1 ATC.
        let q1 = DeliveryPeriod::new(ymd(2027, 1, 1), ymd(2027, 3, 31)).expect("q1");
        let (hq, _) = c.period_price(q1, Block::Atc).expect("q1");
        assert_eq!(hq, 744 + 672 + 743);
    }

    #[test]
    fn bumps_and_validation() {
        let c = pjm_curve();
        let up = c.shifted_block(Block::Peak, 1.0).expect("bump");
        assert_abs_diff_eq!(up.block_price(cm(2027, 1), Block::Peak).expect("p").price, 96.0);
        assert_abs_diff_eq!(up.block_price(cm(2027, 1), Block::OffPeak).expect("p").price, 60.0);
        assert!(c.shifted_block(Block::Atc, 1.0).is_err());
        let both = c.shifted(-2.0).expect("shift");
        assert_abs_diff_eq!(both.block_price(cm(2027, 1), Block::OffPeak).expect("p").price, 58.0);
        // Mismatched hub is refused.
        let other = FuturesCurve::new(PowerHub::ErcotNorth.hub(), c.reference_date(), vec![FuturesQuote::new(cm(2027, 1), 50.0)], CurveInterpolation::Linear)
            .expect("c");
        assert!(matches!(PowerCurve::new(PowerHub::PjmWest, other, c.offpeak().clone()), Err(PowerError::WrongHub { .. })));
    }
}
