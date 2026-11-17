# Validation
 
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

The Python suites cross-check the bindings against QuantLib-Python directly:
`test_bonds.py` sweeps coupons, maturities and settlement dates;
`test_curves.py` compares bootstrap nodes and interpolated discount factors;
`test_swaps.py` compares OIS legs, par rates and swaption prices;
`test_vol.py` prices caps and floors against an `OvernightLeg` under the
Bachelier cap/floor engine; `test_fx.py` compares Garman-Kohlhagen prices,
deltas, gammas and vegas with `AnalyticEuropeanEngine` and strike-from-delta
and ATM strikes in all four delta conventions with `BlackDeltaCalculator`.

# Roadmap
 
1. `fq-models`: Hull-White one-factor (analytic bond options, calibration to
   the swaption cube, trinomial tree and Monte Carlo), the base for the next two.
2. `fq-mbs`: agency pool cashflows, CPR/PSA and a refinancing-incentive
   prepayment model, OAS and option-adjusted duration/convexity.
3. `fq-credit`: survival curves from CDS quotes (ISDA standard model
   conventions), CDS and index pricing, bond-CDS basis.
4. Rates: separate projection and discount curves end to end, Bermudan
   swaptions and CMS convexity on Hull-White, swaption cash settlement.
5. FX: cross-currency curves and XCCY swaps, barriers.
6. Commodities: a Least-Squares Monte Carlo upper bound for storage and
   swing, multi-unit dispatch, LNG and freight, emissions.
7. Generic `Real` for AD-based risk; criterion benches.
8. `fq-capi` (flat C ABI) and Excel.
9. Equities last.
