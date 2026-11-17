# ferrumquant
 
A QuantLib-inspired Rust library for pricing and risk across rates, FX and
commodities, with Python bindings (PyO3) and a trader-style Python view on
top. Excel bindings are planned. The name is Latin *ferrum* (iron) plus
quant; rust is iron oxide.
 
Everything is a Rust 2024 workspace of `fq-*` crates. Market data is passed
by value, errors are typed, and nothing is priced in Python that is not
priced in Rust. The rates slice is validated against QuantLib-Python 1.43;
the commodity slice is validated against exchange date rules checked by hand
and against pricing identities (parity, par swaps, bounds, Monte Carlo vs
closed form).
 
## What is covered
 
| Area | Crate | Products |
|---|---|---|
| Time and numerics | `fq-core` | Dates, day counts, calendars, schedules, periods, IMM dates; Brent and safeguarded Newton, normal CDF, Bachelier, shifted Black, Black-76, SABR, 1D/2D interpolation |
| USD rates | `fq-rates` | Treasury notes/bonds with Z-, I- and asset-swap spreads to the SOFR curve, Treasury futures (conversion factors, basis, CTD, delivery option), SOFR futures, deposits, SOFR OIS swaps, bootstrap from deposits/SR3/OIS, bucketed risk via a par Jacobian, European swaptions, swaption vol cube with SABR smiles, caps and floors |
| FX | `fq-fx` | Currencies and pairs with conventions, spot and forward markets on two curves, deliverable forwards and NDFs, vanillas (Garman-Kohlhagen), delta-quoted vol surfaces |
| Commodities core | `fq-commod-core` | Contract months, hubs, futures curves by delivery month, basis curves, seasonal and Samuelson vol, Black-76 futures options, Asian options, spread options, Clewlow-Strickland forward-curve Monte Carlo |
| Natural gas | `fq-gas` | 18 North American hubs, NYMEX expiry and bidweek, index and basis swaps, transport capacity, storage (intrinsic and rolling intrinsic), swing |
| Power | `fq-power` | 8 ISOs with NERC holidays and block hour counts, 20 hubs, peak/off-peak curves, block swaps, heat-rate options and tolling, hourly shaping, unit-commitment dispatch |
| Oil and products | `fq-oil` | WTI, Brent, RBOB, heating oil, gasoil: expiry rules and front-month rolls, calendar-month-average swaps and APOs, crack options and the 3-2-1, calendar spread options, contango carry |
| Metals | `fq-metals` | Precious metals on USD and lease curves (forwards, GOFO, vanillas); LME prompt rules, prompt-keyed curves, forwards, carries, third-Wednesday options |
| Agriculturals | `fq-ags` | Grains, oilseeds, softs and livestock with listed months and expiry rules, options, the soybean crush, full-carry benchmark, calendar spread options |
| Python | `fq-py`, `python/fqview` | The whole library as the `ferrumquant` module; a Treasury blotter and a cross-commodity book on top |
 
## Layout
 
```
ferrumquant/
├── Cargo.toml                 workspace root (members, shared deps, lints, profiles)
├── build.sh                   build/test driver with per-product targets and the Python venv
├── README.md
├── tools/
│   └── quantlib_golden.py     generates the QuantLib reference values for fq-rates
├── crates/
│   ├── fq-core/               time and math; no market knowledge
│   ├── fq-rates/              USD rates products
│   ├── fq-fx/                 FX
│   ├── fq-commod-core/        commodity primitives shared by every sector crate
│   ├── fq-gas/                natural gas
│   ├── fq-power/              power
│   ├── fq-oil/                oil and refined products
│   ├── fq-metals/             precious and LME base metals
│   ├── fq-ags/                agriculturals
│   └── fq-py/                 PyO3 bindings -> python module `ferrumquant`
└── python/
    ├── pytest.ini
    ├── fqview/                trader-style views (pure Python on ferrumquant)
    └── tests/                 pytest suites for the bindings
```
 
### Core and rates
 
```
crates/fq-core/src
├── time/
│   ├── date.rs          Date as an Excel-compatible serial, weekday/EOM/nth-weekday helpers
│   ├── daycount.rs      ACT/360, ACT/365F, ACT/ACT ISDA and ICMA, 30/360 variants
│   ├── calendar.rs      Calendar as a set of markets plus a join rule, Easter, business-day conventions
│   ├── markets.rs       USGS (SIFMA), USNY (Fed), UK settlement, ...
│   ├── schedule.rs      Backward/Forward generation with stubs, EOM rule, reference periods
│   ├── period.rs        Tenors (D/W/M/Y), parsing, normalisation
│   └── imm.rs           IMM dates and quarterly contract codes
└── math/
    ├── solver.rs        Brent, safeguarded Newton
    ├── black.rs         normal CDF/inverse, Bachelier, shifted Black, Black-76 with Greeks
    ├── sabr.rs          Hagan SABR implied vol and calibration
    └── interpolation.rs 1D/2D interpolators with flat extrapolation
 
crates/fq-rates/src
├── bond/
│   ├── fixed_rate.rs    Treasury cashflows, accrued, Street yield, DV01/duration/convexity, 32nds
│   └── spread.rs        Z-spread, I-spread and asset-swap spread to the SOFR curve
├── future/
│   ├── conversion_factor.rs  CME 6% conversion factors
│   ├── treasury_future.rs    basket, invoice, gross/net basis, carry, implied repo, CTD
│   ├── delivery_option.rs    the short's switch option via yield scenarios
│   └── sofr_future.rs        SR3 futures, compounded settlement, convexity adjustment
├── index/overnight.rs   SOFR fixings and forwards, compounded-in-arrears with lookback/shift/lockout
├── instrument/deposit.rs  overnight and term deposits
├── curve/
│   ├── discount_curve.rs  dated nodes, log-linear DF or linear zero, bumps
│   ├── bootstrap.rs       sequential OIS bootstrap (deposits, SR3 futures, OIS)
│   └── jacobian.rs        par-quote bucketed risk via the curve Jacobian
├── swap/ois_swap.rs     fixed vs compounded SOFR, annuity, par rate, DV01
├── swaption/
│   ├── european.rs      Bachelier and shifted Black on the OIS forward, Greeks, implied vol
│   └── vol_cube.rs      ATM normal-vol grid with a SABR smile per node
├── capfloor/cap_floor.rs  caps and floors on compounded SOFR
└── tests/golden_quantlib.rs  pinned QuantLib-Python values
 
crates/fq-fx/src
├── currency.rs     Currency and CurrencyPair with quoting and settlement conventions
├── forward.rs      FxMarket (spot + two curves), FxForward, FxNdf
├── vanilla.rs      FxVanilla under Garman-Kohlhagen, delta and ATM conventions, premium currency
└── vol_surface.rs  FxSmile and FxVolSurface from ATM/RR/BF quotes, expiry and delivery rules
```
 
### Commodities
 
```
crates/fq-commod-core/src
├── contract.rs   Unit, Commodity, Hub, ContractMonth, DeliveryPeriod
├── curve.rs      FuturesCurve (by delivery month), strips, CalendarSpread, BasisCurve
├── vol.rs        SeasonalVol (per-contract ATM quotes), SamuelsonVol (closed-form integration)
├── option.rs     FuturesOption: Black-76 on one contract, Greeks, implied vol
├── asian.rs      AsianOption: moment-matched average-price options, partial fixings
├── spread.rs     Margrabe / Kirk / Bjerksund-Stensland spread options, implied correlation
└── model.rs      Clewlow-Strickland 1F/2F forward-curve Monte Carlo, xoshiro256** RNG
 
crates/fq-gas/src
├── hub.rs        GasHub registry (18 NA hubs), NYMEX expiry and bidweek rules
├── swap.rs       FixedFloatSwap (monthly index), BasisSwap (hub minus NYMEX)
├── transport.rs  TransportCapacity: strip of basis spread options, net of demand charges
├── storage.rs    StorageFacility: intrinsic DP on an inventory grid, rolling-intrinsic MC
└── swing.rs      SwingOption: greedy intrinsic nomination, Black-76 upper bound, rolling MC
 
crates/fq-power/src
├── iso.rs        Iso (PJM/MISO/ERCOT/NYISO/ISONE/SPP/CAISO/WECC), NERC holidays, 5x16 vs 6x16, DST hours, Block hour counts
├── hub.rs        PowerHub registry (20 hubs/zones) with ISO mapping
├── curve.rs      PowerCurve: peak + off-peak strips, ATC blend, period prices, block bumps
├── swap.rs       BlockSwap: fixed-price block swaps sized MW x block hours
├── heat_rate.rs  HeatRateOption strip (spark spread via core pricers), TollingAgreement net of capacity payments
├── shape.rs      HourlyProfile + shape_month: hourly prices matching peak/off-peak forwards, DST-aware
└── dispatch.rs   ThermalUnit + dispatch(): unit-commitment DP with start cost, min up/down, min load
 
crates/fq-oil/src
├── contract.rs        OilContract (CL, BRN, RB, HO, GAS) specs and units, exchange expiry rules, pricing days, front-month roll
├── swap.rs            average_forward (roll-weighted monthly average), AverageSwap
├── apo.rs             AveragePriceOption on the front-month average via the core Asian pricer, observed fixings
├── crack.rs           CrackSpread / CrackOption (product per bbl vs crude), Crack321 with basket moment matching
└── calendar_spread.rs CalendarSpreadOption (explicit or Clewlow-Strickland-implied correlation), contango_carry
 
crates/fq-metals/src
├── metal.rs      Metal (XAU/XAG/XPT/XPD, LME CA/AH/ZS/NI/PB/SN), units, lot sizes, LME horizons
├── precious.rs   PreciousMarket (spot + USD and lease curves, T+2 London/NY), forwards, GOFO, implied lease, vanillas
├── lme.rs        LME prompt rules: cash, 3M roll, daily/weekly/third-Wednesday prompts, declaration days
└── lme_curve.rs  LmeCurve keyed by prompt (linear carry), C/B, carry bumps, LmeForward, LmeCarry, LmeOption
 
crates/fq-ags/src
├── contract.rs   AgContract (ZC ZW ZS ZM ZL, KC SB CC CT OJ, LE GF HE), units, lots, listed months, expiry rules, AgCalendar (+Good Friday)
├── option.rs     AgOption: Black-76 on futures with exchange option expiry, dollar PV via lot conversion, implied vol
├── crush.rs      Crush: board crush margin (0.022 ZM + 0.11 ZL - ZS/100), oil share, crush option via basket matching
└── spread.rs     full_carry benchmark (storables only), AgCalendarSpread option for storable and livestock months
```
 
Each crate also has `lib.rs` (re-exports and a Send + Sync assertion) and
`error.rs` (a `thiserror` enum). The sector crates depend only on
`fq-commod-core`, `fq-core` and `fq-rates` (for `DiscountCurve`), never on
each other.
 
### Python
 
```
crates/fq-py
├── Cargo.toml       cdylib + rlib, PyO3 0.23, abi3-py39
├── pyproject.toml   maturin config, module-name ferrumquant
└── src
    ├── lib.rs        module init, FerrumError, dict! helper for valuation results
    ├── time.rs       Date, Period; DateLike accepts Date / ISO string / serial / datetime.date
    ├── curve.rs      DiscountCurve, OvernightIndex, bootstrap, CurveJacobian
    ├── bond.rs       FixedRateBond, YieldAnalytics, BondSpreads
    ├── future.rs     TreasuryFuture, DeliveryOption, SofrFuture
    ├── swap.rs       OisSwap, Swaption
    ├── vol.rs        SwaptionVolCube, CapFloor
    ├── commod.rs     ContractMonth (MonthLike: "F27", "2027-01", (2027, 1), a date), Hub, FuturesCurve,
    │                 BasisCurve, SeasonalVol/SamuelsonVol (VolLike: SeasonalVol or flat number),
    │                 FuturesOption, AsianOption, SpreadOption, spread_price, implied_correlation,
    │                 ClewlowStrickland, CurvePaths
    ├── gas.rs        NymexCalendar, gas_hubs/gas_hub, GasSwap, GasBasisSwap, TransportCapacity,
    │                 StorageFacility (intrinsic, rolling_intrinsic), SwingOption
    ├── power.rs      power_hubs, nerc_holidays, block_hours, PowerCurve (block_price, shape),
    │                 BlockSwap, HeatRateOption, TollingAgreement, ThermalUnit.dispatch
    ├── oil.rs        OilMarket (expiry, pricing_days, front_month), AverageSwap, AveragePriceOption,
    │                 CrackOption, Crack321, OilCalendarSpreadOption, contango_carry
    ├── metals.rs     metals, flat_lease_curve, PreciousMarket/Forward/Vanilla, LmeCalendar (prompts),
    │                 LmeCurve, LmeForward, LmeCarry, LmeOption
    └── ags.rs        ag_contracts, AgCalendar (expiry, option_expiry), AgOption, Crush, full_carry,
                      AgCalendarSpread
 
python
├── pytest.ini
├── fqview/
│   ├── __init__.py   exports
│   ├── __main__.py   `python -m fqview [DATE]` (Treasury blotter), `python -m fqview commod [DATE] [PATHS]`
│   ├── bonds.py      TreasuryBlotter, TreasuryPosition, sample_book
│   └── commod.py     CommodMarket (curves, vols, calendars, CS model), CommodPosition builders
│                     (gas_swap, gas_basis_swap, transport, storage, swing, block_swap, heat_rate_option,
│                     tolling, average_swap, apo, crack_option, precious_forward, precious_vanilla,
│                     lme_forward, lme_option, ag_option, crush, ag_spread),
│                     CommodBook (PV, intrinsic/extrinsic, 1c outright delta, sector totals)
└── tests/
    ├── test_bonds.py
    └── test_commod.py   date rules vs the Rust fixtures, pricing identities, the sample book
```
 
## Building and testing
 
Requirements: Rust 1.85+ (edition 2024), and for the Python side Python 3.9+
with `venv`. `build.sh` works with the bash 3.2 that ships on macOS.
 
```
./build.sh                         # build + test every crate (dev profile)
./build.sh -r -b                   # release build only
./build.sh -p bond -p swaption     # tests for selected products
./build.sh -p bond -t yield        # bond tests whose names contain "yield"
./build.sh -p golden               # QuantLib golden tests only
./build.sh -p gas-storage -n       # one commodity module, with println! output
./build.sh -c fq-oil               # everything in one crate
./build.sh --clippy --fmt          # lint pass (clippy -D warnings, fmt --check)
./build.sh --list                  # products and the test filters they map to
 
./build.sh --py setup              # create .venv, install maturin and pytest
./build.sh --py build -r           # maturin develop the ferrumquant module into .venv
./build.sh --py test               # pytest python/tests
./build.sh --py test -p py-commod  # just the commodity suite
./build.sh --py view -- 2026-11-09 # print the sample Treasury blotter
./build.sh --py view -- commod 2026-11-09 2000   # print the sample commodities book with 2000 MC paths
./build.sh --py all --with-quantlib              # venv, module, pytest (incl. QuantLib sweeps), blotter
./build.sh --py shell              # shell with the venv active and python/ on PYTHONPATH
```
 
Products are module-path filters, so a test is "labelled" by where it lives.
Adding a product is one line in `build.sh`. Plain `cargo build` and
`cargo test` skip `fq-py` (it is not a default member); `--workspace`
includes it.
 
Current counts: 300 Rust tests across the nine library crates and 54 Python
tests, plus QuantLib-Python cross-check sweeps that run when QuantLib is
installed.
 
## Using it from Python
 
```python
import ferrumquant as fq
 
ref = fq.Date(2026, 11, 9)
disc = fq.DiscountCurve.flat(ref, 0.04, "2036-12-31")
 
# A Henry Hub curve keyed by contract month, winter-shaped
months = [fq.ContractMonth(2026, 12).add_months(i) for i in range(36)]
hh = fq.FuturesCurve(fq.gas_hub("HH"), ref, [(m, 3.2 + (1.1 if m.is_winter else 0.0)) for m in months])
vol = fq.SeasonalVol([(m, 0.6 if m.is_winter else 0.4) for m in months])
cal = fq.NymexCalendar()
 
# Storage: intrinsic DP, then rolling intrinsic under Clewlow-Strickland
fac = fq.StorageFacility("HH", "Z26", "X27", 1_000_000, 10_000, 20_000, 0.02, 0.02, 0.01)
print(fac.intrinsic(hh, disc, cal)["intrinsic_pv"])
cs = fq.ClewlowStrickland(0.9, 2.5, 0.25, 0.3)
print(fac.rolling_intrinsic(hh, cs, disc, cal, paths=2000)["extrinsic_pv"])
 
# Spark spread strip against a PJM West peak/off-peak curve
pk = fq.FuturesCurve(fq.power_hub("PJMW"), ref, [(m, 55.0) for m in months])
op = fq.FuturesCurve(fq.power_hub("PJMW"), ref, [(m, 32.0) for m in months])
power = fq.PowerCurve("PJMW", pk, op)
hro = fq.HeatRateOption("call", "PJMW", "PK", "F27", "Z27", 100, 7.5, 2.0)
print(hro.value(power, hh, 0.5, vol, 0.6, disc)["extrinsic_pv"])
 
# Copper on LME prompts
lme = fq.LmeCalendar()
cu = fq.LmeCurve("copper", ref, [(d, 9500.0) for d, k in lme.prompts("CA", ref, 15) if k in ("cash", "3m", "monthly")], lme)
print(fq.LmeOption("call", "CA", 25, 9600.0, 2027, 6).value(cu, 0.22, disc, lme)["pv"])
```
 
Conventions on the Python side:
 
- Dates accept `fq.Date`, `datetime.date`, `"YYYY-MM-DD"` or an Excel serial.
- Contract months accept `ContractMonth`, `"F27"`, `"2027-01"`, `(2027, 1)` or
  a date (its month).
- Vols accept a `SeasonalVol` or a flat number.
- Enum-like inputs are strings: `"call"`/`"put"`, hub and contract codes
  (`"HH"`, `"PJMW"`, `"CL"`, `"ZC"`), metal codes or names (`"CA"` or
  `"copper"`), blocks (`"PK"`, `"OP"`, `"ATC"`, `"2x16"`, `"7x8"`), spread
  methods (`"margrabe"`, `"kirk"`, `"bs"`) and day counts (`"ACT/365F"`).
- Every commodity `value()` returns a plain `dict`, so results drop straight
  into pandas. Registries (`gas_hubs()`, `power_hubs()`, `oil_contracts()`,
  `metals()`, `ag_contracts()`) are dicts too.
- Errors raise `fq.FerrumError` with the Rust error message.
`fqview` is the desk layer. `CommodMarket` carries curves, vols, calendars
and the Monte Carlo model; a `CommodPosition` is a label plus a pricer
closure; `CommodBook` prices the lot, bumps each position's outright curve by
one cent for a delta column and totals by sector. `sample_commod_market` and
`sample_commod_positions` build an 18-line book across gas, power, oil,
metals and ags.
 
## Design choices
 
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
 
## The finance, module by module
 
### Day counts (`fq-core::time::daycount`)
 
A day count turns two dates into a year fraction, which is the exponent in
every discount factor and the multiplier on every coupon. ACT/360 and ACT/365F
are trivial. The two that matter here are:
 
*ACT/ACT ICMA* (government bonds): the fraction is
`days(d1, d2) / (frequency × days(reference period))`, where the reference
period is the coupon period the accrual sits in. For a regular semiannual
coupon every full period is exactly 0.5 regardless of its length in days,
which is why a 4.25% Treasury pays exactly 2.125 per 100 every coupon. Odd
first or last coupons are handled by walking *notional* regular periods
forward or back from the real coupon dates; the implementation is a direct
port of QuantLib's `ActualActual::ISMA` and reproduces the ISDA memo examples
(0.4975 vs 0.5000 for ISDA vs ICMA on the same regular period, 0.9158 vs
0.9151 on a long first coupon).
 
*30/360 variants*: Bond Basis (ISDA 2006), 30/360 US with the end-of-February
rules, and 30E/360. The only difference between them is how a 31st, and
February's last day, are clamped to 30.
 
### Calendars and schedules (`calendar`, `schedule`)
 
Two US calendars are built in. *USGS* follows SIFMA's recommendations for the
government securities market and is used for Treasury settlement and SOFR
fixings. *USNY* follows the Federal Reserve. They differ on Good Friday (USGS
closed, USNY open) and on Saturday holidays (USNY moves New Year's and
Veterans Day to the Friday, USGS does not). One deliberate divergence from
QuantLib: Good Friday is always a USGS holiday here, even in payroll-release
years when SIFMA recommends only an early close, because SIFMA still says those
days are not good settlement days. A UK settlement calendar backs the LME
and the precious metals T+2 rule.
 
`Schedule` mirrors QuantLib's Backward/Forward generation with explicit stub
dates, the end-of-month rule, and a regularity flag per period.
`Schedule::ref_period(i)` returns the notional coupon period that ACT/ACT ICMA
needs for a stub, which is how the bond gets accrued interest right on odd
coupons without any special casing.
 
### Treasury bonds (`fq-rates::bond::fixed_rate`)
 
Conventions: semiannual, ACT/ACT ICMA on unadjusted coupon dates, payments
rolled Following on USGS, T+1 settlement, EOM rule when the maturity is a
month end (so a note maturing Aug 31 pays Feb 28/29 and Aug 31).
 
Yield follows the *Street convention*, which is what Bloomberg's YA screen and
Treasury's own calculations use:
 
```
P_dirty = Σ_k CF_k / (1 + y/2)^(2 t_k)
```
 
with `t_k` the ICMA year fraction from settlement to each coupon date,
accumulated period by period, on *unadjusted* dates. In the final coupon
period the market switches to simple interest, `P_dirty = CF / (1 + y t)`;
QuantLib calls this `SimpleThenCompounded`. The yield solve uses a
safeguarded Newton on the analytic derivative inside a bracket, so it cannot
diverge.
 
Risk comes from the same loop that prices: `dP/dy` and `d²P/dy²` give modified
duration `−(1/P) dP/dy`, convexity `(1/P) d²P/dy²`, DV01 `−dP/dy × 10⁻⁴`, and
Macaulay duration `Σ t_k PV_k / P`.
 
Treasury prices quote in 32nds: `99-16` is 99 + 16/32, a trailing `+` adds
half a 32nd, and a third digit is eighths of a 32nd (`99-162` = 99 + 16.25/32).
 
`bond::spread` adds the three spreads to the SOFR curve: Z-spread (the flat
shift to the zero curve that reprices the bond), I-spread (yield minus the
matched-maturity par swap rate) and the par asset-swap spread.
 
### Treasury futures (`fq-rates::future`)
 
A Treasury future is a contract to deliver any bond from a basket, with the
invoice price adjusted by a *conversion factor* so that every bond in the
basket is roughly equivalent. The CME factor is the price of $1 par at a 6%
yield, with remaining term measured in whole months from the first day of the
delivery month and rounded down to quarters (ZN, TN, TWE, ZB, UB) or months
(ZT, Z3N, ZF). The formula is in `conversion_factor` and reproduces published
CME factors to four decimals.
 
Because the 6% assumption is never exactly right, one bond is always cheapest
to deliver. The analytics in `treasury_future` are the standard basis toolkit:
 
- Invoice price `F × CF + accrued(delivery)`.
- Gross basis `P_clean − F × CF`.
- Carry: finance the dirty price at term repo (ACT/360) to delivery, collect
  and reinvest any coupon in between; the resulting forward clean price gives
  `carry = P_clean − P_forward`.
- Net basis `gross − carry`, the part of the basis not explained by carry.
- Implied repo: the financing rate at which buying the bond and delivering
  it into the future breaks even. Solved in closed form from
  `P_dirty (1 + r τ) = invoice + Σ c_i (1 + r τ_i)`.
The CTD is the bond with the highest implied repo. The CTD-only fair value is
`min_i P_forward,i / CF_i`, an upper bound on the futures price because it
ignores the short's option to switch bonds if the market moves.
`delivery_option` prices that switch option by revaluing the basket over
yield scenarios at delivery and taking the expectation of the minimum.
 
`sofr_future` covers CME three-month SOFR futures (SR3): settlement to
`100 − R` with `R` the daily-compounded SOFR over the IMM quarter, the
forward implied by a curve, and a convexity adjustment (an input in rate
units, since it depends on a rate-vol model) between the futures rate and the
forward rate.
 
### SOFR and the OIS swap (`index::overnight`, `swap::ois_swap`)
 
Post-LIBOR, the vanilla USD swap is a SOFR OIS: spot start T+2, annual fixed
leg vs annual *compounded-in-arrears* SOFR, both ACT/360 ModifiedFollowing on
the USGS+USNY calendar, with a 2-business-day payment lag.
 
The floating rate for a period is the daily compounding of overnight fixings,
where each fixing applies until the next business day (so a Friday fixing
carries three calendar days):
 
```
R = ( Π_i (1 + r_i n_i / 360) − 1 ) × 360 / N
```
 
The `OvernightIndex` uses stored fixings for past days and, for future days,
the simple forward implied by a discount curve between consecutive business
days. The important property is that compounding those forwards *telescopes*:
`Π (DF_i / DF_{i+1}) = DF_start / DF_end`, so the floating leg's PV is
`N × (DF_start − DF_end)` before the payment lag. That identity is what makes
single-curve OIS pricing consistent and the bootstrap exact. Lookback,
observation shift and lockout are supported for other conventions.
 
Valuation returns both legs, the *annuity* `A = Σ N τ_k DF(p_k)` (the PV of
one unit of fixed rate), and the par rate `float_pv / A`. A payer's NPV is
`float_pv − fixed_pv`.
 
### Curves and the bootstrap (`curve`)
 
`DiscountCurve` holds discount factors on dated nodes with ACT/365F time and
an implicit `DF(0) = 1`. The default interpolation is log-linear on discount
factors, equivalent to piecewise-constant instantaneous forwards, which is
what lets a bootstrap reprice its inputs exactly. Linear-zero interpolation is
available; both extrapolate flat beyond the last node.
 
The bootstrap is sequential: the overnight deposit pins the front, SR3
futures (convexity-adjusted) fill the first two years, and OIS par quotes
take over from there. Each instrument adds one node at its last relevant
date, solved with Brent so the instrument reprices at par given the nodes
already fixed. With flat forwards between nodes the problem is
one-dimensional per instrument and every input reprices to ~1e-11. The node
dates and discount factors match QuantLib's `PiecewiseLogLinearDiscount`
with `Pillar.LastRelevantDate` helpers.
 
`DiscountCurve::shifted(bp)` and `with_node_shift(i, bp)` provide the bumps
for parallel DV01 and bucketed zero risk. `curve::jacobian` converts zero
bucketed risk into par-quote risk (what a desk hedges with) by inverting the
Jacobian of instrument par rates with respect to the curve nodes.
 
### Swaptions, the vol cube and caps (`swaption`, `capfloor`)
 
A European swaption is an option to enter a swap at a fixed strike on the
expiry date. Pricing it under the *annuity measure* reduces the problem to a
vanilla option on the forward swap rate `F` with the annuity `A` as numeraire:
 
```
PV_payer = A × [ (F − K) N(d) + σ√T n(d) ],   d = (F − K) / (σ√T)     (Bachelier)
```
 
Bachelier (normal vol, quoted in basis points) is the USD market standard
because it handles low and negative rates and because rate moves are closer
to additive than multiplicative. Shifted Black is also available,
`d(F + s) = σ (F + s) dW`, for markets that quote lognormal vols with a shift.
 
`F` and `A` are not computed by a separate formula: they come from the
underlying `OisSwap::value()`, so the swaption is consistent with the swap
pricer by construction, and payer minus receiver equals the forward-starting
swap's NPV exactly. Greeks are returned in currency units (`delta = A × N(d)`,
`vega = A × √T n(d)`), with the per-unit-annuity model output attached.
Settlement is physical; cash settlement under the IRR-annuity convention is
not yet implemented.
 
`SwaptionVolCube` holds an ATM normal-vol grid over (expiry, tenor) with a
SABR smile calibrated at each node, so an off-ATM strike gets a smile vol
interpolated in expiry and tenor. `CapFloor` prices caplets and floorlets on
compounded-in-arrears SOFR with the same Bachelier formula per period.
 
### FX (`fq-fx`)
 
An `FxMarket` is spot plus the two currencies' discount curves; the forward
is covered interest parity `S × DF_quote(T) / DF_base(T)` on the pair's
spot-lag and settlement calendars. `FxForward` and `FxNdf` value against it
(the NDF fixes on the fixing date and settles the difference in the
deliverable currency). `FxVanilla` is Garman-Kohlhagen, which is Black-76 on
the forward discounted at the quote-currency rate, with the market's delta
conventions (spot or forward, premium-adjusted or not) and ATM conventions
(delta-neutral straddle, forward). `FxVolSurface` builds a smile per expiry
from ATM, risk-reversal and butterfly quotes and interpolates in delta space.
 
### Commodity primitives (`fq-commod-core`)
 
*Futures curves.* A `FuturesCurve` maps delivery months to settlement prices
for one `Hub`; interpolation is linear or previous-flat across missing
months. `strip_price` is the day-weighted average over a date range,
`average` the equal-weighted average over months, and `BasisCurve` holds
`hub − benchmark` so a locational curve is benchmark plus basis.
 
*Vol.* `SeasonalVol` is one ATM Black vol per contract month, the market's
quoting convention. `SamuelsonVol` is the term-structure model
`σ(τ) = σ_∞ + (σ_0 − σ_∞) e^{−ατ}`, integrated in closed form from option
expiry to contract expiry, which captures the rise in vol as a contract
approaches delivery.
 
*Options.* `FuturesOption` is Black-76 on one contract month. `AsianOption`
is the Turnbull-Wakeman moment-matching approximation for an average of
fixings that may sit on different contracts, with already-observed fixings
folded into an effective strike. Spread options come in three flavours:
Margrabe (exact, zero strike), Kirk (strike absorbed into the short leg) and
Bjerksund-Stensland 2014 (the default; a better approximation for
non-zero strikes), with an implied-correlation solver.
 
*Monte Carlo.* `ClewlowStrickland` is the one- or two-factor forward-curve
model `dF(t,T)/F = σ_s e^{−α(T−t)} dW_1 + σ_l dW_2`. Simulation is exact in
the mean-reverting factor (the stochastic integral `∫ e^{αs} dW` has a known
variance per step), curves are stored per path and step, and `black_vol` and
`correlation` give the model-implied Black vol of any contract and the
correlation between any two, so the same parameters price vanillas, spread
options and storage consistently.
 
### Natural gas (`fq-gas`)
 
The NYMEX Henry Hub future expires three business days before the first of
the delivery month; bidweek is the five trading days ending on expiry, when
monthly index prices are set. `FixedFloatSwap` pays fixed against the monthly
index on daily quantities; `BasisSwap` does the same for hub minus NYMEX.
 
`TransportCapacity` values firm pipeline capacity as a strip of monthly spread
options on delivery minus receipt, less fuel retention and the commodity
charge, net of the demand charge; intrinsic is the forward margin floored at
zero, option value uses the spread pricers with a receipt/delivery
correlation. `StorageFacility` is the standard inventory problem: the
intrinsic value is a dynamic program over months on an inventory grid with
injection and withdrawal limits and costs, and the rolling-intrinsic value
re-solves that DP along each simulated Clewlow-Strickland curve, locking in
the first month's decision each step, which is the lower bound a desk hedges
to. `SwingOption` nominates daily volumes within daily and total bounds; the
intrinsic is a greedy knapsack on the forward curve, the upper bound treats
every day as an independent Black-76 option, and rolling Monte Carlo sits in
between.
 
### Power (`fq-power`)
 
Power is quoted in blocks. Peak is hours ending 8 to 23 on non-holiday
weekdays (5x16; CAISO and WECC include Saturdays, 6x16), off-peak is
everything else, ATC is the hour-weighted blend, and the wraps (2x16, 7x8)
split off-peak into weekend daytime and nights. `iso` computes the hour
counts from the NERC holiday list and the DST transitions (23-hour March day,
25-hour November day). `PowerCurve` holds a peak and an off-peak strip per
hub and derives every other block.
 
A `HeatRateOption` is a strip of monthly options on the spark spread
`P − HR × G − VOM`, valued with the spread pricers against a gas curve with a
power/gas correlation; `TollingAgreement` is that call strip less the capacity
payment (in $/kW-month). `shape_month` turns a monthly peak/off-peak pair into
hourly prices with a 24-hour profile scaled to reproduce the block averages
exactly, DST-aware. `dispatch` is a unit-commitment dynamic program for one
thermal unit against hourly power and gas: start cost, minimum up and down
times and minimum load, returning the schedule alongside the frictionless
value so the cost of the constraints is visible.
 
### Oil (`fq-oil`)
 
Each contract has its own expiry rule: WTI stops trading three business days
before the 25th of the month preceding delivery (shifted back if the 25th is
not a business day), Brent on the last business day of the second month
before delivery, RBOB and heating oil on the last business day of the month
before, gasoil two business days before the 14th. Those rules decide the
front-month roll inside a calendar month, which is why a calendar-month
average of the front month (`average_forward`) is a roll-weighted blend of
two contracts. `AverageSwap` and `AveragePriceOption` are built on that
blend; the APO uses the core Asian pricer with the roll correlation between
the two contracts, and observed fixings are folded in as the month progresses.
 
`CrackOption` is a spread option on product (converted to $/bbl, 42 gallons)
minus crude. `Crack321` is the refinery margin `(2 RB + 1 HO − 3 CL)/3`,
with an option valued by matching the lognormal moments of the three-asset
basket. `CalendarSpreadOption` is an option on near minus far for one
contract, with the correlation either given or implied by a
Clewlow-Strickland model; `contango_carry` compares a spread with the cost of
storing and financing a barrel between the two months.
 
### Metals (`fq-metals`)
 
Precious metals trade like currencies: a `PreciousMarket` is spot plus a USD
curve and a lease-rate curve, the forward is `S × DF_lease / DF_usd` to the
value date (T+2 on the joint London/New York calendar), and GOFO is the
implied simple forward rate. `PreciousForward` and `PreciousVanilla`
(Black-76 on that forward, with a spot delta) follow.
 
LME base metals trade to prompt dates rather than contract months. `lme`
implements the rule set: cash is T+2, the three-month prompt rolls day by day
(with the end-of-month and non-business-day adjustments), daily prompts run
out to three months, then weekly Wednesdays to six months, then third
Wednesdays out to the metal's horizon, and options declare on the first
Wednesday for the third-Wednesday prompt. `LmeCurve` is keyed by prompt date
with linear carry between quotes; `LmeForward`, `LmeCarry` (a borrow or lend
between two prompts) and `LmeOption` value against it.
 
### Agriculturals (`fq-ags`)
 
Thirteen contracts across CBOT grains and oilseeds, ICE softs and CME
livestock, each with its listed months, lot size, quotation unit (cents per
bushel, cents per pound, dollars per short ton) and expiry rule (the business
day before the 15th for CBOT, specific day counts for coffee, sugar, cocoa,
cotton and orange juice, and the livestock rules). `AgCalendar` is the US
exchange calendar including Good Friday. `AgOption` is Black-76 with the
exchange's option expiry (the Friday before the futures expiry for CBOT) and
converts the premium to dollars through the lot size.
 
`Crush` is the board crush `0.022 × ZM + 0.11 × ZL − ZS/100` in $/bu (meal
in $/short ton, oil in cents/lb, beans in cents/bu), with the oil share and
an option on the margin by basket moment matching. `full_carry` is the
benchmark spread between two months for a storable: storage cost plus
interest on the near price, so a spread can be quoted as a percentage of full
carry. `AgCalendarSpread` is the option on the near/far spread.
 
### Numerics (`fq-core::math`)
 
- `brent` for derivative-free root finding on a bracket; `newton_safe` is
  Newton with a bisection fallback (Numerical Recipes `rtsafe`) for cases with
  a cheap analytic derivative such as bond yield.
- `norm_cdf` is Hart's rational approximation (accurate to ~1e-14), with
  `norm_inv` for Monte Carlo.
- `bachelier`, `shifted_black` and `black76` return price, delta, gamma and
  vega per unit numeraire so the same code serves swaptions, FX vanillas
  and every commodity option.
- `sabr` is Hagan's asymptotic implied-vol formula with a least-squares
  calibration to a smile.
- `interpolation` provides linear and flat 1D and bilinear 2D interpolators
  with flat extrapolation, used by the vol cube and the FX surface.
## Validation
 
Unit tests sit next to the code and check closed forms, parity relations,
finite-difference Greeks and hand-computed dates (ISDA day-count examples,
SIFMA holiday lists, Presidents' Day coupon rolls, Good Friday settlement).
 
`crates/fq-rates/tests/golden_quantlib.rs` pins outputs to QuantLib-Python
1.43: Treasury yield, accrued, duration and convexity (including short first
coupon, final-period simple yield and EOM cases); OIS par rates and every
cashflow of a 5Y payer; a 1Y×10Y swaption's NPV, annuity, vega and delta
under both models; and all ten nodes of a SOFR bootstrap. The generating
script is `tools/quantlib_golden.py`. One subtlety: QuantLib bonds discount to
business-day-adjusted payment dates by default, while the Street convention
uses unadjusted coupon dates; the golden bonds are built with `Unadjusted`
payment dates so the comparison is like for like.
 
Conversion factors are checked against published CME values (ZN Dec 2017 and
Mar 2009 examples).
 
The commodity crates are checked against exchange date rules worked by hand
(WTI CLF27 expiring 21 Dec 2026, Brent Jul-27 on 28 May, the LME three-month
roll from 30 Jul to 29 Oct 2027, the 2027 NERC holidays, CBOT and ICE
expiries around Good Friday 26 Mar 2027) and against pricing identities:
put-call parity, par swaps repricing to zero, intrinsic at or below option
value at or below the Black-76 upper bound, Margrabe equal to Kirk and
Bjerksund-Stensland at zero strike, inventory feasibility of every storage
schedule, and Monte Carlo means and European prices within standard error of
the closed forms. `python/tests/test_commod.py` repeats the date fixtures and
identities through the bindings and prices the sample book end to end.
 
## Roadmap
 
1. Rates: swaption cash settlement, more bootstrap instruments, bucketed vega
   through the cube.
2. FX: cross-currency curves and XCCY swaps, barriers.
3. Commodities: a Least-Squares Monte Carlo upper bound for storage and
   swing, multi-unit dispatch, LNG and freight, emissions.
4. Generic `Real` for AD-based risk.
5. `fq-capi` (flat C ABI) and Excel.
6. Equities last.
## License
 
BSD-3-Clause. Pricing logic ported from QuantLib carries QuantLib's modified
BSD license notice.
