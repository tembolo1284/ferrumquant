use thiserror::Error;

use fq_commod_core::{CommodError, ContractMonth};
use fq_core::math::BlackError;
use fq_core::time::{Date, DateError, DayCountError};
use fq_rates::curve::CurveError;

#[derive(Debug, Error, Clone, PartialEq)]
pub enum AgsError {
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
    WrongContract { expected: &'static str, got: String },
    #[error("{month} is not a listed month for {contract}")]
    NotListed { contract: &'static str, month: ContractMonth },
    #[error("expiry {expiry} is not after valuation {valuation}")]
    Expired { expiry: Date, valuation: Date },
}
