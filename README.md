# ferrumquant

A QuantLib-inspired Rust library for pricing and risk across rates, FX and
commodities, with Python and Excel bindings planned. The name is Latin
*ferrum* (iron) plus quant; rust is iron oxide.

The first slice covers four USD products end to end, each validated against
QuantLib-Python 1.43:

| Product | Module | What it does |
|---|---|---|
| US Treasury note/bond | `fq-rates::bond` | Cashflows, accrued, Street yield, DV01/duration/convexity, 32nds quotes |
| Treasury bond future | `fq-rates::future` | CME conversion factors, deliverable basket, invoice, basis, carry, implied repo, CTD |
| SOFR OIS swap | `fq-rates::swap`, `index`, `curve` | Compounded-in-arrears leg, NPV, par rate, annuity, DV01, OIS bootstrap |
| European swaption | `fq-rates::swaption` | Bachelier and shifted-Black on the OIS forward, Greeks, implied vol |

## Layout

```
ferrumquant/
├── Cargo.toml                 workspace root
├── build.sh                   build/test driver with per-product targets
├── tools/quantlib_golden.py   generates the QuantLib reference values
└── crates/
    ├── fq-core/               time, math, no market knowledge
    │   └── src/
    │       ├── time/          date, daycount, calendar, schedule
    │       └── math/          solver (Brent, safeguarded Newton), black (normal CDF, Bachelier, shifted Black)
    └── fq-rates/              USD rates products
        ├── src/
        │   ├── bond/          fixed_rate
        │   ├── future/        conversion_factor, treasury_future
        │   ├── index/         overnight (SOFR)
        │   ├── curve/         discount_curve, bootstrap
        │   ├── swap/          ois_swap
        │   └── swaption/      european
        └── tests/
            └── golden_quantlib.rs
```

`fq-core` knows nothing about products; `fq-rates` depends on it. Planned
crates follow the same pattern: `fq-fx`, `fq-cmdty`, `fq-py` (PyO3) and
`fq-capi` (flat C ABI for Excel).

## Building and testing

```
./build.sh                       # build + test everything (dev profile)
./build.sh -r -b                 # release build only
./build.sh -p bond -p swaption   # tests for selected products
./build.sh -p bond -t yield      # bond tests whose names contain "yield"
./build.sh -p golden             # QuantLib golden tests only
./build.sh --clippy --fmt        # lint pass
./build.sh --list                # products and the test filters they map to
```

Products are module-path filters, so a test is "labelled" by where it lives.
Adding a product is one line in `build.sh`.

## Design choices

**No observer graph.** QuantLib's `Handle`/`Observer` machinery lets a curve
update propagate to every instrument. That design fights Rust's ownership
model, so here market data is passed explicitly: a pricer takes `&DiscountCurve`
and `&OvernightIndex` as arguments and returns a value. Risk is bump-and-reprice
over cloned curves (`DiscountCurve::shifted`, `with_node_shift`), which also
makes everything trivially parallel. Algorithmic differentiation is planned
via a generic `Real` type.

**Dates are serial numbers.** `Date` is an Excel-compatible serial (days since
1899-12-30), range-checked to QuantLib's 1901–2199. Calendar lookups and date
arithmetic are integer operations, and golden values from Excel or
QuantLib-Python line up day for day.

**Calendars are values.** A `Calendar` is a set of markets plus a join rule,
so the USD swap calendar is literally `"USGS+USNY".parse()`. No trait objects,
cheap to clone, easy to pass through FFI.

**Errors are typed.** Every module has a `thiserror` enum; higher-level errors
wrap lower-level ones with `#[from]`, so a bad date inside a swaption surfaces
as `SwaptionError::Swap(SwapError::Schedule(...))` with the original message.

**Conventions are explicit and USD-first.** `FixedRateBond::us_treasury`,
`OisSwap::usd_sofr_spot`, `Swaption::usd_sofr` and `OvernightIndex::sofr`
bake in the market conventions below. The general constructors take every
convention as a parameter so other markets can be added without touching the
pricing code.

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
days are not good settlement days.

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
`min_i P_forward,i / CF_i`, which is an upper bound on the futures price: it
ignores the short's option to switch bonds or delivery day if the market
moves. Modelling that delivery option (via yield scenarios over the basket) is
the natural next step.

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

The bootstrap is sequential: OIS par quotes are sorted by maturity, and each
adds one node at the swap's final payment date, solved with Brent so the swap
reprices at par given the nodes already fixed. With flat forwards between
nodes the problem is one-dimensional per instrument and every input reprices
to ~1e-11. The node dates and discount factors match QuantLib's
`PiecewiseLogLinearDiscount` with `Pillar.LastRelevantDate` helpers.

`DiscountCurve::shifted(bp)` and `with_node_shift(i, bp)` provide the bumps
for parallel DV01 and bucketed risk.

### Swaptions (`swaption::european`)

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

### Numerics (`fq-core::math`)

- `brent` for derivative-free root finding on a bracket; `newton_safe` is
  Newton with a bisection fallback (Numerical Recipes `rtsafe`) for cases with
  a cheap analytic derivative such as bond yield.
- `norm_cdf` is Hart's rational approximation (accurate to ~1e-14), with
  `norm_inv` for Monte Carlo later.
- `bachelier`, `shifted_black` and `black76` return price, delta, gamma and
  vega per unit numeraire so the same code serves swaptions, FX vanillas
  (Garman–Kohlhagen is Black-76 on the FX forward) and commodity futures
  options.

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

## Roadmap

1. `Period` with days and weeks; deposits and SR3 futures (with convexity) in
   the bootstrap; bucketed DV01 and a par-rate Jacobian.
2. Curve-based bond analytics (Z-spread, OIS-discounted USTs), the futures
   delivery option, swaption cash settlement, SABR, caps/floors.
3. Generic `Real` for AD-based risk; `fq-py` and `fq-capi` bindings, Excel.
4. `fq-fx`: FX conventions, cross-currency curves, FX swaps/NDFs/XCCY,
   vanillas and the delta-quoted vol surface, barriers.
5. `fq-cmdty`: contract-based forward curves, average-price swaps and APOs,
   Black-76 futures options, spread options.
6. Equities last.

## License

BSD-3-Clause. Pricing logic ported from QuantLib carries QuantLib's modified
BSD license notice.
