//! Crude oil and refined products on top of `fq-commod-core`.
//!
//! Oil is the most financial of the physical commodities: the benchmarks
//! (WTI, Brent) are deep futures markets, and most physical cargoes and
//! OTC deals price off a *monthly average* of daily settles rather than a
//! single fix. The quant work is therefore less about physical constraints
//! than about getting three things right: which contract is the front
//! month on each pricing day (expiry rules differ by contract), averaging
//! (swaps and Asian options), and spreads (cracks between products and
//! crude, calendar spreads along one curve).
//!
//! * [`contract`]: contract specs, units and conversion factors, exchange
//!   expiry rules, pricing-day calendars and front-month rolls.
//! * [`swap`]: fixed-for-average monthly swaps.
//! * [`apo`]: average price options via the core moment-matched pricer.
//! * [`crack`]: single-product and 3-2-1 crack spreads, forward and as
//!   spread options.
//! * [`calendar_spread`]: calendar spread options and contango carry.

pub mod apo;
pub mod calendar_spread;
pub mod contract;
pub mod crack;
pub mod error;
pub mod swap;

pub use apo::{AveragePriceOption, ApoValuation};
pub use calendar_spread::{CalendarSpreadOption, CalendarSpreadValuation, CarryResult, contango_carry};
pub use contract::{Exchange, OilContract, OilMarket, expiry, front_month, pricing_days};
pub use crack::{Crack321, Crack321Valuation, CrackOption, CrackOptionValuation, CrackSpread, basket_lognormal};
pub use error::OilError;
pub use swap::{AverageSwap, AverageSwapMonth, AverageSwapValuation, average_forward};

#[cfg(test)]
mod thread_safety {
    use super::*;

    fn assert_send_sync<T: Send + Sync>() {}

    #[test]
    fn shared_types_are_send_sync() {
        assert_send_sync::<OilContract>();
        assert_send_sync::<OilMarket>();
        assert_send_sync::<AverageSwap>();
        assert_send_sync::<AveragePriceOption>();
        assert_send_sync::<CrackSpread>();
        assert_send_sync::<Crack321>();
        assert_send_sync::<CalendarSpreadOption>();
    }
}
