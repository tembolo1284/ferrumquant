//! North American natural gas on top of `fq-commod-core`.
//!
//! Gas is the physical commodity where the core's futures curve maps most
//! directly onto traded instruments: NYMEX Henry Hub futures by delivery
//! month, basis swaps at every other hub, and three kinds of optionality the
//! desk owns physically rather than as listed options: pipeline transport
//! (a basis spread option per month), storage (a calendar spread option
//! with inventory constraints) and swing (a strip of daily calls with
//! volume constraints).
//!
//! Valuation philosophy is the one a trading desk actually uses: every
//! physical asset gets an *intrinsic* value off today's forward curve,
//! which is what can be locked in with futures right now, and an
//! *extrinsic* value from re-optimising as the curve moves. For transport
//! the extrinsic piece is a closed-form spread option. For storage it is a
//! rolling-intrinsic Monte Carlo on Clewlow-Strickland curve paths, a
//! lower bound that captures most of the true option value and is the
//! number most desks mark to.

pub mod error;
pub mod hub;
pub mod storage;
pub mod swap;
pub mod swing;
pub mod transport;

pub use error::GasError;
pub use hub::{GasHub, NymexCalendar, bidweek, nymex_expiry};
pub use storage::{StorageFacility, StorageSchedule, StorageValuation, intrinsic_storage, rolling_intrinsic_storage};
pub use swap::{BasisSwap, FixedFloatSwap, SwapLegValuation, SwapValuation};
pub use swing::{SwingOption, SwingValuation};
pub use transport::{TransportCapacity, TransportMonth, TransportValuation};

#[cfg(test)]
mod thread_safety {
    use super::*;

    fn assert_send_sync<T: Send + Sync>() {}

    #[test]
    fn shared_types_are_send_sync() {
        assert_send_sync::<GasHub>();
        assert_send_sync::<FixedFloatSwap>();
        assert_send_sync::<BasisSwap>();
        assert_send_sync::<TransportCapacity>();
        assert_send_sync::<StorageFacility>();
        assert_send_sync::<SwingOption>();
    }
}
