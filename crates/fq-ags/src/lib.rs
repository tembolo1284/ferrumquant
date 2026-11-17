//! Agricultural commodities on top of `fq-commod-core`.
//!
//! Ags are the oldest futures markets and the most idiosyncratic in their
//! conventions. Each contract trades a handful of listed months a year
//! (corn has five, soybeans seven), quotes in its own unit (cents per
//! bushel, cents per pound, dollars per short ton) and has its own last-
//! trading-day rule. Two features matter for pricing: whether the
//! commodity is *storable* (grains and softs yes, livestock no, which
//! breaks the carry link between months), and the processing spreads that
//! tie contracts together, above all the soybean crush.
//!
//! * [`contract`]: contracts, units, listed months, expiry rules and an
//!   exchange calendar that adds Good Friday to US settlement holidays.
//! * [`option`]: Black-76 options on futures with the exchange expiry.
//! * [`crush`]: the board crush margin and a crush spread option.
//! * [`spread`]: calendar spreads with a full-carry benchmark for
//!   storables and spread options for both kinds.

pub mod contract;
pub mod crush;
pub mod error;
pub mod option;
pub mod spread;

pub use contract::{AgCalendar, AgContract, AgSector, Exchange, QuoteUnit, expiry, option_expiry};
pub use crush::{Crush, CrushOptionValuation, CrushValuation, basket_lognormal};
pub use error::AgsError;
pub use option::{AgOption, AgOptionValuation};
pub use spread::{AgCalendarSpread, AgCalendarSpreadValuation, CarryBenchmark, full_carry};

#[cfg(test)]
mod thread_safety {
    use super::*;

    fn assert_send_sync<T: Send + Sync>() {}

    #[test]
    fn shared_types_are_send_sync() {
        assert_send_sync::<AgContract>();
        assert_send_sync::<AgCalendar>();
        assert_send_sync::<AgOption>();
        assert_send_sync::<Crush>();
        assert_send_sync::<AgCalendarSpread>();
    }
}
