//! Interest rate swaps.

pub mod ois_swap;

pub use ois_swap::{OisSwap, SwapCashflow, SwapError, SwapPeriod, SwapSide, SwapValuation, Tenor};
