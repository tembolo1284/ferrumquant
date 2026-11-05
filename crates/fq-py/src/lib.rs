//! Python bindings for ferrumquant.
//!
//! The module is deliberately thin: every class wraps a `fq-core`/`fq-rates`
//! type by value and forwards to it, errors become `FerrumError`, and dates
//! are accepted as `ferrumquant.Date`, `datetime.date`, `"YYYY-MM-DD"` or an
//! Excel serial. Nothing is priced here that is not priced in Rust.
//!
//! Modules: `time` (Date, Period), `bond` (FixedRateBond), `curve`
//! (DiscountCurve, OvernightIndex, bootstrap, Jacobian), `swap` (OisSwap,
//! Swaption), `future` (TreasuryFuture, SofrFuture), `vol` (SwaptionVolCube,
//! CapFloor).

use std::fmt::Display;

use pyo3::create_exception;
use pyo3::exceptions::PyException;
use pyo3::prelude::*;

mod bond;
mod curve;
mod future;
mod swap;
mod time;
mod vol;

pub use bond::{PyBondSpreads, PyFixedRateBond, PyYieldAnalytics};
pub use curve::{PyBootstrapResult, PyCurveJacobian, PyDiscountCurve, PyOvernightIndex};
pub use future::{PyDeliveryOption, PySofrFuture, PyTreasuryFuture};
pub use swap::{PyOisSwap, PySwapValuation, PySwaption, PySwaptionPricing};
pub use time::{DateLike, PeriodLike, PyDate, PyPeriod};
pub use vol::{PyCapFloor, PyCapFloorPricing, PySwaptionVolCube};

create_exception!(ferrumquant, FerrumError, PyException, "Error raised by the ferrumquant library.");

/// Map any `Display` error (all the library's `thiserror` enums) to `FerrumError`.
pub(crate) fn err<E: Display>(e: E) -> PyErr {
    FerrumError::new_err(e.to_string())
}

#[pymodule]
fn ferrumquant(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    m.add("FerrumError", m.py().get_type::<FerrumError>())?;
    time::register(m)?;
    bond::register(m)?;
    curve::register(m)?;
    swap::register(m)?;
    future::register(m)?;
    vol::register(m)?;
    Ok(())
}
