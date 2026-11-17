//! North American wholesale power on top of `fq-commod-core`.
//!
//! Power is the commodity where *when* matters most: it cannot be stored,
//! so a megawatt-hour at 5pm on a July weekday and one at 3am on a Sunday
//! are different products with different prices and vols. The market deals
//! with that by trading standard *blocks* (on-peak 5x16, off-peak wrap,
//! around-the-clock) by delivery month at a hub, and everything in this
//! crate is organised around block hours.
//!
//! * [`iso`] knows the ISO/RTO calendars: NERC holidays, the 5x16 vs 6x16
//!   peak rule, and daylight-saving hour counts.
//! * [`curve`] holds a hub's peak and off-peak forward curves together and
//!   prices any block or delivery period from them.
//! * [`swap`] values fixed-price block swaps.
//! * [`heat_rate`] prices heat-rate (spark spread) options and tolling
//!   agreements as strips of spread options with the core pricers.
//! * [`shape`] turns block forwards into an hourly price series with a
//!   calibrated intraday profile.
//! * [`dispatch`] runs a unit-commitment dynamic program for a thermal
//!   unit with start costs and minimum up/down times against hourly
//!   prices: the physical-asset view of what [`heat_rate`] prices
//!   financially.

pub mod curve;
pub mod dispatch;
pub mod error;
pub mod heat_rate;
pub mod hub;
pub mod iso;
pub mod shape;
pub mod swap;

pub use curve::{BlockPrice, PowerCurve};
pub use dispatch::{DispatchResult, ThermalUnit, dispatch};
pub use error::PowerError;
pub use heat_rate::{HeatRateMonth, HeatRateOption, HeatRateValuation, TollingAgreement, TollingValuation};
pub use hub::PowerHub;
pub use iso::{Block, Iso, block_hours, hours_in_day, is_nerc_holiday, is_peak_day, is_peak_hour};
pub use shape::{HourlyPrices, HourlyProfile, shape_month};
pub use swap::{BlockSwap, BlockSwapValuation};

#[cfg(test)]
mod thread_safety {
    use super::*;

    fn assert_send_sync<T: Send + Sync>() {}

    #[test]
    fn shared_types_are_send_sync() {
        assert_send_sync::<Iso>();
        assert_send_sync::<PowerHub>();
        assert_send_sync::<PowerCurve>();
        assert_send_sync::<BlockSwap>();
        assert_send_sync::<HeatRateOption>();
        assert_send_sync::<TollingAgreement>();
        assert_send_sync::<ThermalUnit>();
        assert_send_sync::<HourlyPrices>();
    }
}
