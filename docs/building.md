# Building and testing
 
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
./build.sh --py test -p py-fx      # one suite (py-bond, py-curve, py-swap, py-future, py-vol, py-models, py-fx, py-commod)
./build.sh --py view -- 2026-11-09 # every sample view
./build.sh --py view -- rates 2026-11-09          # SOFR curve, swap book, vol cube, futures basis
./build.sh --py view -- models 2026-11-09         # Hull-White fit to the cube, Bermudan book, path check
./build.sh --py view -- fx 2026-11-09             # FX forwards, smiles and option book
./build.sh --py view -- commod 2026-11-09 2000   # the commodities book with 2000 MC paths
./build.sh --py all --with-quantlib              # venv, module, pytest (incl. QuantLib sweeps), blotter
./build.sh --py shell              # shell with the venv active and python/ on PYTHONPATH
```
 
Products are module-path filters, so a test is "labelled" by where it lives.
Adding a product is one line in `build.sh`. Plain `cargo build` and
`cargo test` skip `fq-py` (it is not a default member); `--workspace`
includes it.
 
Current counts: 337 Rust tests across the ten library crates and 316 Python
tests (parametrized QuantLib-Python cross-checks included; they skip when
QuantLib is not installed).

# Using it from Python
 
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

`fqview` is the desk layer, one module per screen: `CurveView` bootstraps
once (with the Jacobian) and turns any Python pricer into a par-quote DV01
ladder; `RatesBlotter` values swaps and swaptions and ladders the book;
`BasisScreen` is the Treasury futures basis table; `VolView` shows the
swaption cube, SABR smiles and a cap/floor strip; `ModelView` calibrates
Hull-White to the cube's ATM grid, prices a Bermudan book against the
first-exercise Europeans and checks simulated paths against the curve;
`FxView` prints forward points, smile pillars and an FX option book with
Greeks in market conventions. On the commodity side `CommodMarket` carries
curves, vols, calendars and the Monte Carlo model; a `CommodPosition` is a
label plus a pricer closure; `CommodBook` prices the lot, bumps each
position's outright curve by one cent for a delta column and totals by
sector. `sample_commod_market` and `sample_commod_positions` build an
18-line book across gas, power, oil, metals and ags.
