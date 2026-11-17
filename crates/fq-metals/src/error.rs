use thiserror::Error;

use fq_commod_core::CommodError;
use fq_core::math::BlackError;
use fq_core::time::{Date, DateError, DayCountError};
use fq_rates::curve::CurveError;

#[derive(Debug, Error, Clone, PartialEq)]
pub enum MetalsError {
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
    #[error("{0} is a precious metal; use the precious module")]
    NotBaseMetal(&'static str),
    #[error("{0} is a base metal; use the lme modules")]
    NotPreciousMetal(&'static str),
    #[error("{date} is not a valid LME prompt date as of {trade_date}")]
    InvalidPrompt { date: Date, trade_date: Date },
    #[error("date {0} is before the curve's first prompt or after its last")]
    OutOfRange(Date),
    #[error("expiry {expiry} is not after valuation {valuation}")]
    Expired { expiry: Date, valuation: Date },
    #[error("curve for {expected} was expected, got {got}")]
    WrongMetal { expected: &'static str, got: String },
}
