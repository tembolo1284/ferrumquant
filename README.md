# ferrumquant
 
A QuantLib-inspired Rust library for pricing and risk across rates, FX and
commodities, with Python bindings (PyO3) and a trader-style Python view on
top. Excel bindings are planned. The name is Latin *ferrum* (iron) plus
quant; rust is iron oxide.
 
Everything is a Rust 2024 workspace of `fq-*` crates. Market data is passed
by value, errors are typed, and nothing is priced in Python that is not
priced in Rust. The rates and FX slices are validated against QuantLib-Python
(1.43 and 1.44);
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
| Python | `fq-py`, `python/fqview` | The whole library as the `ferrumquant` module; desk views on top: Treasury blotter, SOFR curve and par-quote risk, swap/swaption book, vol cube and caps, Treasury futures basis, FX forwards/smiles/option book, cross-commodity book |

## Quick start

````
git clone https://github.com/tembolo1284/ferrumquant && cd ferrumquant
./build.sh --clippy --fmt                  # fmt check, clippy -D warnings, cargo test --workspace
./build.sh --py all --with-quantlib        # venv, ferrumquant module, pytest, sample views
./build.sh --py view -- rates 2026-11-09   # or: bonds | fx | commod | all
````

Requirements: Rust 1.85+ (edition 2024); Python 3.9+ with `venv` for the
bindings. `build.sh` runs on the bash 3.2 that ships with macOS. Current
counts: 300 Rust tests across the nine library crates and 294 Python tests
(QuantLib-Python cross-checks included; they skip when QuantLib is absent).

````python
import ferrumquant as fq

today = "2026-11-09"
sofr = fq.OvernightIndex.sofr()
curve = fq.bootstrap_sofr(today, sofr, [("DEPO", "1D", 0.0432), ("SR3", "Z26", 95.78, 0.0002), ("OIS", "2Y", 0.0380), ("OIS", "10Y", 0.0395)]).curve
swap = fq.OisSwap.usd_sofr_spot(today, "5Y", "pay", 100e6, 0.0378)
print(swap.value(sofr, curve).npv, swap.dv01(sofr, curve))

eur, usd = fq.DiscountCurve.flat(today, 0.02, "2040-01-01"), fq.DiscountCurve.flat(today, 0.04, "2040-01-01")
mkt = fq.FxMarket("EURUSD", today, 1.1750, eur, usd)
surf = fq.FxVolSurface(mkt, [("1M", 0.072, -0.004, 0.0015), ("1Y", 0.080, -0.009, 0.0025, -0.0165, 0.008)])
call = fq.FxVanilla.from_tenor("EURUSD", today, "6M", "call", 1.20, 10e6)
print(surf.value(call)["pv_quote"], surf.vol_for_delta(call.expiry, "put", -0.25))
````

## Documentation

| Document | Contents |
|---|---|
| [docs/building.md](docs/building.md) | `build.sh` options for Rust and Python, the venv, test selection by product, using the module from Python, the `fqview` desk layer |
| [docs/layout.md](docs/layout.md) | Every crate and module with a one-line description |
| [docs/design.md](docs/design.md) | Why there is no observer graph, how market data and risk are passed, error handling, thread safety |
| [docs/finance.md](docs/finance.md) | The finance, module by module: day counts and calendars, Treasuries and futures, SOFR and OIS, curves and bootstrap, swaptions and caps, FX, each commodity sector, numerics |
| [docs/validation.md](docs/validation.md) | What is checked against QuantLib, CME and exchange rules, and the roadmap |

## License
 
BSD-3-Clause. Pricing logic ported from QuantLib carries QuantLib's modified
BSD license notice.
