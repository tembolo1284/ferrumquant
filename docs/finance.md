# The finance, module by module
 
## Day counts (`fq-core::time::daycount`)
 
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
 
## Calendars and schedules (`calendar`, `schedule`)
 
Two US calendars are built in. *USGS* follows SIFMA's recommendations for the
government securities market and is used for Treasury settlement and SOFR
fixings. *USNY* follows the Federal Reserve. They differ on Good Friday (USGS
closed, USNY open) and on Saturday holidays (USNY moves New Year's and
Veterans Day to the Friday, USGS does not). One deliberate divergence from
QuantLib: Good Friday is always a USGS holiday here, even in payroll-release
years when SIFMA recommends only an early close, because SIFMA still says those
days are not good settlement days. TARGET, UK settlement and Japan calendars
(diffed against QuantLib over 2000 to 2060) back the FX spot and forward
date rules, the LME and the precious metals T+2 rule; `Calendar::joint`
combines any set of them.
 
`Schedule` mirrors QuantLib's Backward/Forward generation with explicit stub
dates, the end-of-month rule, and a regularity flag per period.
`Schedule::ref_period(i)` returns the notional coupon period that ACT/ACT ICMA
needs for a stub, which is how the bond gets accrued interest right on odd
coupons without any special casing.
 
## Treasury bonds (`fq-rates::bond::fixed_rate`)
 
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
 
## Treasury futures (`fq-rates::future`)
 
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
 
## SOFR and the OIS swap (`index::overnight`, `swap::ois_swap`)
 
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
 
## Curves and the bootstrap (`curve`)
 
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
 
## Swaptions, the vol cube and caps (`swaption`, `capfloor`)
 
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
 
## Hull-White (`fq-models`)
 
The one-factor Hull-White model `dr = (theta(t) - a r) dt + sigma dW` fits
any `DiscountCurve` by construction. The implementation never evaluates
`theta` or the instantaneous forward curve: writing `r = x + alpha(t)` with
`x` the zero-mean Ornstein-Uhlenbeck factor, every bond price is
 
```
P(t,T | x) = P(0,T)/P(0,t) exp(-B x - B y(t) - B^2 v(t)/2)
B = (1 - e^{-a(T-t)})/a,  v(t) = sigma^2 (1 - e^{-2at})/(2a),  y(t) = sigma^2 (1 - e^{-at})^2/(2a^2)
```
 
which depends on the curve only through discount factors. Under the
`T`-forward measure `x(T)` is Gaussian with mean `-y(T)` and variance `v(T)`,
which prices anything paid at `T`. ZCB options are Jamshidian's closed form.
A caplet on daily-compounded SOFR over `[s, e]` is the exchange of two
lognormals, `exp(-int_0^s r)` against `(1 + K tau) exp(-int_0^e r)`, whose
variance is the forward-looking `B(tau)^2 v(s)` plus the variance accrued
inside the period; the forward-looking term-rate caplet is kept for
comparison and is strictly cheaper.
 
A European swaption's underlying, seen at expiry, is a sum of
exponential-affine terms `c_k exp(-beta_k x)`: fixed coupons discount to
their lagged payment dates, each floating payment is `N (P(T,s) - P(T,e))`
times the deterministic lag factor `P(0,p)/P(0,e)` (the same treatment as
`fq-rates`, so payer minus receiver equals the forward swap NPV exactly).
Locating the exercise boundary `x*` with Brent turns the option into a sum
of truncated-lognormal expectations: Jamshidian's decomposition on the
swap's real dates rather than on a bullet bond. The result carries the
implied Bachelier vol, so the model can be read against the cube.
 
Calibration minimises vega-weighted price errors against swaption normal
vols with Nelder-Mead in log coordinates, either `sigma` alone with `a`
fixed (the usual desk choice) or both. A single `(a, sigma)` cannot reproduce
a surface whose vols fall with tenor as fast as the sample cube does; the
fit lands in the middle with a few bp of error, which is the model's known
limitation rather than the fit's.
 
Bermudans use the Hull-White trinomial tree on `x`: the branch from each
node is centred on the node nearest the mean `x e^{-a dt}`, which truncates
the lattice at `|j| = 1/(2 a dt)`, probabilities match the exact mean and
variance per step, time steps are non-uniform so exercise dates land on
slices, and the one-period discounts `exp(-(alpha_i + x_j) dt)` are fitted
slice by slice through Arrow-Debreu prices so the tree reprices the curve.
At an exercise node the swap value is the analytic affine sum for that
node's factor, so a single-exercise Bermudan converges to the closed form.
 
Simulation is exact: per step the pair `(x, int x)` is jointly Gaussian with
closed-form covariance, so the bank account `P(0,t) exp(-int x - int y)` has
no discretisation error and `E[D(t)] = P(0,t)`, `E[D(t) P(t,T|x)] = P(0,T)`
hold in expectation. Bond prices along a path come from the same formula,
which is what a prepayment model or a Least-Squares Monte Carlo callable
needs.
 
## FX (`fq-fx`)
 
An `FxMarket` is spot plus the two currencies' discount curves; the forward
is covered interest parity `S × DF_quote(T) / DF_base(T)` on the pair's
spot-lag and settlement calendars. `FxForward` and `FxNdf` value against it
(the NDF fixes on the fixing date and settles the difference in the
deliverable currency). `FxVanilla` is Garman-Kohlhagen, which is Black-76 on
the forward discounted at the quote-currency rate, with the market's delta
conventions (spot or forward, premium-adjusted or not) and ATM conventions
(spot, forward, delta-neutral straddle, premium-adjusted straddle), with the
market defaults by pair: USD premium when the pair has USD, so EURUSD deltas
are plain and USDJPY deltas premium-adjusted; spot delta to one year, forward
delta beyond. `FxVolSurface` reads each expiry's ATM, risk-reversal and
butterfly quotes (smile-strangle convention), converts them to strikes under
those conventions, fits lognormal SABR per pillar and interpolates total
variance in time at fixed strike. Prices, Greeks, strike-from-delta and ATM
strikes are checked against QuantLib's `AnalyticEuropeanEngine` and
`BlackDeltaCalculator`.
 
## Commodity primitives (`fq-commod-core`)
 
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
 
## Natural gas (`fq-gas`)
 
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
 
## Power (`fq-power`)
 
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
 
## Oil (`fq-oil`)
 
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
 
## Metals (`fq-metals`)
 
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
 
## Agriculturals (`fq-ags`)
 
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
 
## Numerics (`fq-core::math`)
 
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
- `optim::nelder_mead` is a simplex minimiser in any dimension, used by the
  Hull-White calibration; `rng` is a self-contained xoshiro256** with
  Marsaglia polar Gaussians, so Monte Carlo paths are reproducible across
  platforms with no external dependency.
