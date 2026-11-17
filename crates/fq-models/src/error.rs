use thiserror::Error;

use fq_core::math::{BlackError, SolverError};
use fq_core::time::{Date, DateError};
use fq_rates::curve::CurveError;
use fq_rates::swap::SwapError;
use fq_rates::swaption::SwaptionError;

#[derive(Debug, Error, Clone, PartialEq)]
pub enum ModelError {
    #[error("mean reversion {0} must be finite and positive")]
    BadMeanReversion(f64),
    #[error("volatility {0} must be finite and positive")]
    BadVolatility(f64),
    #[error("times must satisfy 0 <= t <= T; got t = {t}, T = {big_t}")]
    BadTimes { t: f64, big_t: f64 },
    #[error("strike {0} must be finite and positive")]
    BadStrike(f64),
    #[error("date {date} is not after the curve reference date {reference}")]
    NotAfterReference { date: Date, reference: Date },
    #[error("accrual end {end} must be after accrual start {start}")]
    BadAccrual { start: Date, end: Date },
    #[error("exercise dates must be strictly increasing, after today and before the last period start; offending date {0}")]
    BadExerciseDate(Date),
    #[error("no exercise dates")]
    NoExerciseDates,
    #[error("tree needs at least one step per year; got {0}")]
    BadTreeSteps(usize),
    #[error("simulation needs at least one path and one time step")]
    BadSimulation,
    #[error("simulation times must be strictly increasing and positive; offending time {0}")]
    BadSimulationTime(f64),
    #[error("calibration needs at least one instrument")]
    NoInstruments,
    #[error("calibration instrument {index}: {reason}")]
    BadInstrument { index: usize, reason: String },
    #[error("calibration did not converge (rmse {rmse})")]
    NoConvergence { rmse: f64 },
    #[error(transparent)]
    Date(#[from] DateError),
    #[error(transparent)]
    Curve(#[from] CurveError),
    #[error(transparent)]
    Swap(#[from] SwapError),
    #[error(transparent)]
    Swaption(#[from] SwaptionError),
    #[error(transparent)]
    Solver(#[from] SolverError),
    #[error(transparent)]
    Black(#[from] BlackError),
}
