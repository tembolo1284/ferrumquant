//! Numerical building blocks.

pub mod black;
pub mod interpolation;
pub mod sabr;
pub mod solver;

pub use black::{BlackError, BlackResult, OptionType, bachelier, bachelier_implied_vol, black76, norm_cdf, norm_inv, norm_pdf, shifted_black, shifted_black_implied_vol};
pub use interpolation::{Bilinear, CubicSpline, InterpError, Linear};
pub use sabr::{SabrCalibration, SabrError, SabrParams, SabrVolType, calibrate as calibrate_sabr};
pub use solver::{SolverConfig, SolverError, bracket, brent, newton_safe};
