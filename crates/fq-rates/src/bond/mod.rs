//! Bonds.

pub mod fixed_rate;

pub use fixed_rate::{BondError, Coupon, FixedRateBond, YieldAnalytics, format_32nds, parse_32nds};
