# Design choices
 
**No observer graph.** QuantLib's `Handle`/`Observer` machinery lets a curve
update propagate to every instrument. That design fights Rust's ownership
model, so here market data is passed explicitly: a pricer takes `&DiscountCurve`
and `&OvernightIndex` as arguments and returns a value. Risk is bump-and-reprice
over cloned curves (`DiscountCurve::shifted`, `with_node_shift`,
`FuturesCurve::shifted`, `bumped_month`), which also makes everything
trivially parallel. Every shared type is asserted `Send + Sync` at compile
time so a multithreaded risk engine can hold them as plain owned data.
Algorithmic differentiation is planned via a generic `Real` type.
 
**Dates are serial numbers.** `Date` is an Excel-compatible serial (days since
1899-12-30), range-checked to QuantLib's 1901 to 2199. Calendar lookups and
date arithmetic are integer operations, and golden values from Excel or
QuantLib-Python line up day for day.
 
**Calendars are values.** A `Calendar` is a set of markets plus a join rule,
so the USD swap calendar is literally `"USGS+USNY".parse()`. No trait objects,
cheap to clone, easy to pass through FFI. Exchange calendars in the commodity
crates (`NymexCalendar`, `OilMarket`, `LmeCalendar`, `AgCalendar`) wrap these
and add the expiry rules.

**Contract months are the commodity key.** A `FuturesCurve` is a map from
`ContractMonth` to settlement price, not a time-to-maturity curve. Expiry
dates, pricing days and front-month rolls come from the exchange rules, so an
instrument says "Jan-27 Henry Hub" and the calendar decides the rest.
 
**Errors are typed.** Every module has a `thiserror` enum; higher-level errors
wrap lower-level ones with `#[from]`, so a bad date inside a swaption surfaces
as `SwaptionError::Swap(SwapError::Schedule(...))` with the original message,
and a bad curve inside a storage valuation as `GasError::Curve(...)`.
 
**Conventions are explicit and USD-first.** `FixedRateBond::us_treasury`,
`OisSwap::usd_sofr_spot`, `Swaption::usd_sofr` and `OvernightIndex::sofr`
bake in the market conventions below. The general constructors take every
convention as a parameter so other markets can be added without touching the
pricing code.
 
**No external dependencies in the numerics.** The Monte Carlo RNG is a
self-contained xoshiro256** with a splitmix64 seeder, so results are
reproducible across platforms and the workspace depends only on `thiserror`
and `num-traits` outside tests.
