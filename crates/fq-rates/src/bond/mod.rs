//! Bonds.

pub mod fixed_rate;
pub mod spread;

pub use fixed_rate::{BondError, Coupon, FixedRateBond, YieldAnalytics, format_32nds, parse_32nds};
pub use spread::{BondSpreads, SpreadError, asset_swap_spread, i_spread, matched_maturity_swap_rate, npv_with_spread, spreads, z_spread};
