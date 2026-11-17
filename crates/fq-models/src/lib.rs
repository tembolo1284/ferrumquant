//! ferrumquant models: term-structure models on top of `fq-rates` curves.
//!
//! Hull-White one-factor is the first: closed-form bonds, ZCB options,
//! caplets on compounded rates and European swaptions; calibration to
//! swaption quotes; a trinomial tree for Bermudans; exact short-rate
//! simulation for path-dependent products.

pub mod calibration;
pub mod error;
pub mod hull_white;
pub mod simulation;
pub mod swaption;
pub mod tree;

pub use calibration::{CalibrationInstrument, CalibrationMode, CalibrationResult, InstrumentFit, calibrate};
pub use error::ModelError;
pub use hull_white::{CapletKind, HullWhite};
pub use simulation::{McConfig, ShortRatePaths};
pub use swaption::{BermudanSwaption, HwSwaptionPricing};
pub use tree::TrinomialTree;

// Everything a risk engine shares across threads must stay plain owned data
// (no Rc/RefCell/Cell). This fails to compile if that ever changes.
const _: () = {
    const fn assert_shareable<T: Send + Sync>() {}
    assert_shareable::<HullWhite>();
    assert_shareable::<BermudanSwaption>();
    assert_shareable::<TrinomialTree>();
    assert_shareable::<ShortRatePaths>();
};
