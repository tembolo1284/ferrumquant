use thiserror::Error;

use fq_commod_core::{CommodError, ContractMonth};
use fq_core::math::BlackError;
use fq_core::time::{Date, DateError, DayCountError};
use fq_rates::curve::CurveError;

#[derive(Debug, Error, Clone, PartialEq)]
pub enum OilError {
    #[error(transparent)]
    Commod(#[from] CommodError),
    #[error(transparent)]
    Date(#[from] DateError),
    #[error(transparent)]
    DayCount(#[from] DayCountError),
    #[error(transparent)]
    Curve(#[from] CurveError),
    #[error(transparent)]
    Black(#[from] BlackError),
    #[error("invalid input: {0}")]
    Invalid(String),
    #[error("curve for {expected} was expected, got {got}")]
    WrongContract { expected: String, got: String },
    #[error("period {start}..={end} is empty or reversed")]
    EmptyPeriod { start: ContractMonth, end: ContractMonth },
    #[error("no listed contract is live on {0}; extend the curve")]
    NoFrontMonth(Date),
    #[error("{month} has already expired relative to valuation date {valuation}")]
    Expired { month: ContractMonth, valuation: Date },
}
