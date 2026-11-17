use thiserror::Error;

use fq_commod_core::{CommodError, ContractMonth};
use fq_core::math::BlackError;
use fq_core::time::{Date, DateError, DayCountError};
use fq_rates::curve::CurveError;

#[derive(Debug, Error, Clone, PartialEq)]
pub enum PowerError {
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
    WrongHub { expected: String, got: String },
    #[error("period {start}..={end} is empty or reversed")]
    EmptyPeriod { start: ContractMonth, end: ContractMonth },
    #[error("{month} delivery starts on or before valuation date {valuation}")]
    MonthStarted { month: ContractMonth, valuation: Date },
    #[error("dispatch: {0}")]
    Dispatch(String),
}
