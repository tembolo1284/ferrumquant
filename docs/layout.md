# Layout
 
```
ferrumquant/
├── Cargo.toml                 workspace root (members, shared deps, lints, profiles)
├── build.sh                   build/test driver with per-product targets and the Python venv
├── README.md                  overview, quick start, links into docs/
├── docs/                      building.md, layout.md, design.md, finance.md, validation.md
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
 
## Core and rates
 
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
 
## Commodities
 
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
 
## Python
 
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
    ├── fx.rs         CurrencyPair (dates, conventions), FxMarket, FxForward, FxNdf, FxSlice (delta <-> strike,
    │                 ATM strikes), FxVanilla (Garman-Kohlhagen, Greeks in market conventions), FxVolSurface
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
│   ├── __main__.py   `python -m fqview [GROUP] [DATE] [PATHS]`; GROUP = all | bonds | rates | fx | commod
│   ├── bonds.py      TreasuryBlotter, TreasuryPosition, sample_book
│   ├── curves.py     CurveView: bootstrap with Jacobian, node table, par-quote DV01 ladders for any pricer
│   ├── swaps.py      RatesBlotter, SwapPosition, SwaptionPosition: NPV, PV01, DV01, vega, book ladder
│   ├── futures.py    BasisScreen, Deliverable: CF, gross/net basis, carry, implied repo, CTD, switch option
│   ├── vol.py        VolView, CapPosition: ATM grid, SABR per node, smiles, cap/floor strip
│   ├── fx.py         FxView, FxOptionPosition: forward points ladder, smile pillars, option book
│   └── commod.py     CommodMarket (curves, vols, calendars, CS model), CommodPosition builders
│                     (gas_swap, gas_basis_swap, transport, storage, swing, block_swap, heat_rate_option,
│                     tolling, average_swap, apo, crack_option, precious_forward, precious_vanilla,
│                     lme_forward, lme_option, ag_option, crush, ag_spread),
│                     CommodBook (PV, intrinsic/extrinsic, 1c outright delta, sector totals)
└── tests/
    ├── test_bonds.py    QuantLib sweep over coupons x maturities x settlements
    ├── test_curves.py   bootstrap nodes and interpolated DFs vs QuantLib, Jacobian ladders, error paths
    ├── test_swaps.py    OIS legs and par rates, swaption goldens, implied vols, blotter
    ├── test_futures.py  conversion factors, basis, CTD, delivery option, SR3
    ├── test_vol.py      vol cube calibration, caps/floors vs QuantLib OvernightLeg + Bachelier engine
    ├── test_fx.py       dates, parity, Greeks; Garman-Kohlhagen and BlackDeltaCalculator vs QuantLib
    └── test_commod.py   date rules vs the Rust fixtures, pricing identities, the sample book
```
