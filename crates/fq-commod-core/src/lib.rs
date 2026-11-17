//! Commodity primitives shared by the sector crates (fq-gas, fq-power,
//! fq-oil, fq-metals, fq-ags).
//!
//! Everything a commodity desk prices starts from a strip of futures or
//! forwards keyed by *delivery period* rather than by a single tenor axis,
//! so the central object here is a [`FuturesCurve`] over [`ContractMonth`]s.
//! On top of that sit the formulas every sector reuses: Black-76 on a
//! futures contract, moment-matched Asian (average-price) options, spread
//! options (Margrabe / Kirk / Bjerksund-Stensland) for cracks, sparks and
//! basis, a seasonal term structure of vol with the Samuelson effect, and a
//! one/two-factor Clewlow-Strickland model for Monte Carlo on the whole
//! forward curve.
//!
//! Market data is passed explicitly as values that clone cheaply for bumps,
//! as in `fq-rates` and `fq-fx`. Discounting always comes from an
//! `fq_rates::curve::DiscountCurve`.

pub mod asian;
pub mod contract;
pub mod curve;
pub mod error;
pub mod model;
pub mod option;
pub mod spread;
pub mod vol;

pub use asian::{AsianOption, AsianValuation, Fixing};
pub use contract::{Commodity, ContractMonth, DeliveryPeriod, Hub, Unit};
pub use curve::{BasisCurve, CalendarSpread, CurveInterpolation, FuturesCurve, FuturesQuote};
pub use error::CommodError;
pub use model::{ClewlowStrickland, CurvePaths, McConfig, Xoshiro256StarStar};
pub use option::{FuturesOption, FuturesOptionValuation};
pub use spread::{SpreadMethod, SpreadOption, SpreadValuation};
pub use vol::{SamuelsonVol, SeasonalVol, VolQuote};

#[cfg(test)]
mod thread_safety {
    use super::*;

    fn assert_send_sync<T: Send + Sync>() {}

    #[test]
    fn shared_types_are_send_sync() {
        assert_send_sync::<ContractMonth>();
        assert_send_sync::<FuturesCurve>();
        assert_send_sync::<BasisCurve>();
        assert_send_sync::<SeasonalVol>();
        assert_send_sync::<FuturesOption>();
        assert_send_sync::<AsianOption>();
        assert_send_sync::<SpreadOption>();
        assert_send_sync::<ClewlowStrickland>();
        assert_send_sync::<CurvePaths>();
    }
}
