use thiserror::Error;

use fq_core::math::{BlackError, InterpError, SolverError};
use fq_core::time::{Date, DateError, DayCountError};
use fq_rates::curve::CurveError;

use crate::contract::ContractMonth;

#[derive(Debug, Error, Clone, PartialEq)]
pub enum CommodError {
    #[error(transparent)]
    Date(#[from] DateError),
    #[error(transparent)]
    DayCount(#[from] DayCountError),
    #[error(transparent)]
    Curve(#[from] CurveError),
    #[error(transparent)]
    Interp(#[from] InterpError),
    #[error(transparent)]
    Black(#[from] BlackError),
    #[error(transparent)]
    Solver(#[from] SolverError),
    #[error("invalid input: {0}")]
    Invalid(String),
    #[error("month {0} is not valid (1..=12)")]
    InvalidMonth(u32),
    #[error("need at least one quote")]
    NoQuotes,
    #[error("quotes must be strictly increasing by contract month; {0} repeated or out of order")]
    UnsortedQuotes(ContractMonth),
    #[error("price {price} for {month} must be positive")]
    NonPositivePrice { month: ContractMonth, price: f64 },
    #[error("{month} is outside the curve range {first}..={last}")]
    OutOfRange { month: ContractMonth, first: ContractMonth, last: ContractMonth },
    #[error("no contract delivers on or after {0}")]
    NoFrontMonth(Date),
    #[error("curves must share units and reference date; got {0} and {1}")]
    CurveMismatch(String, String),
    #[error("option expiry {expiry} is not after the valuation date {valuation}")]
    Expired { expiry: Date, valuation: Date },
    #[error("expiry {expiry} is after the contract's last trade / delivery start {limit}")]
    ExpiryAfterContract { expiry: Date, limit: Date },
    #[error("correlation {0} must lie in [-1, 1]")]
    BadCorrelation(f64),
    #[error("volatility {0} must be non-negative and finite")]
    BadVol(f64),
    #[error("need at least one fixing")]
    NoFixings,
    #[error("fixings must be in increasing date order")]
    UnsortedFixings,
    #[error("Monte Carlo config invalid: {0}")]
    BadMcConfig(String),
}
