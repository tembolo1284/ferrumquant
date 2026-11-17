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
//! CapFloor), `fx` (CurrencyPair, FxMarket, FxForward, FxNdf, FxVanilla,
//! FxVolSurface), and the commodities stack: `commod` (ContractMonth, Hub,
//! FuturesCurve, BasisCurve, SeasonalVol, FuturesOption, AsianOption,
//! SpreadOption, ClewlowStrickland), `gas`, `power`, `oil`, `metals`, `ags`.
//!
//! Commodity valuations are returned as plain dicts (one per `value()`
//! call) rather than one pyclass per result type, since there are dozens
//! of result shapes and a dict is what fqview and pandas want anyway.

use std::fmt::Display;

use pyo3::create_exception;
use pyo3::exceptions::PyException;
use pyo3::prelude::*;

mod ags;
mod bond;
mod commod;
mod curve;
mod future;
mod fx;
mod gas;
mod metals;
mod oil;
mod power;
mod swap;
mod time;
mod vol;

pub use bond::{PyBondSpreads, PyFixedRateBond, PyYieldAnalytics};
pub use curve::{PyBootstrapResult, PyCurveJacobian, PyDiscountCurve, PyOvernightIndex};
pub use future::{PyDeliveryOption, PySofrFuture, PyTreasuryFuture};
pub use fx::{PyCurrencyPair, PyFxForward, PyFxMarket, PyFxNdf, PyFxSlice, PyFxVanilla, PyFxVolSurface};
pub use swap::{PyOisSwap, PySwapValuation, PySwaption, PySwaptionPricing};
pub use time::{DateLike, PeriodLike, PyDate, PyPeriod};
pub use vol::{PyCapFloor, PyCapFloorPricing, PySwaptionVolCube};

/// `dict!(py, "k" => v, ...)` builds a `PyDict`; items must be `IntoPyObject`.
macro_rules! dict {
    ($py:expr, $($k:literal => $v:expr),* $(,)?) => {{
        let d = pyo3::types::PyDict::new($py);
        $( d.set_item($k, $v)?; )*
        d
    }};
}
pub(crate) use dict;

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
    fx::register(m)?;
    commod::register(m)?;
    gas::register(m)?;
    power::register(m)?;
    oil::register(m)?;
    metals::register(m)?;
    ags::register(m)?;
    Ok(())
}
