"""Model view: Hull-White calibrated to the swaption cube, Bermudans and a path check.

Usage::

    python -m fqview.models             # sample curve, cube, calibration, Bermudan book
    python -m fqview.models 2026-10-13

or from Python::

    from fqview import CurveView, VolView, ModelView, sample_quotes, sample_cube_quotes, sample_bermudans
    cv = CurveView("2026-10-13", sample_quotes())
    vv = VolView(cv, sample_cube_quotes(cv))
    mv = ModelView(cv, vv, mean_reversion=0.05)
    mv.print_fit()                          # market vs model normal vols on the ATM grid
    mv.print_bermudans(sample_bermudans())  # Bermudan PVs against their first Europeans
    mv.print_paths()                        # simulated bank account reprices the curve

Conventions (all from the Rust side):
    * Calibration instruments are the ATM payer swaptions of the cube grid,
      quoted in normal vol; the fit minimises vega-weighted price errors.
    * ``mode="sigma"`` keeps the mean reversion fixed; ``"both"`` fits it too.
    * Bermudans are co-terminal on spot-starting USD SOFR swaps, priced on
      the trinomial tree; the first-exercise European is the lower bound.
"""

from __future__ import annotations

import datetime as dt
import sys
from dataclasses import dataclass
from typing import Sequence

import ferrumquant as fq

from .curves import CurveView, sample_quotes
from .vol import VolView, sample_cube_quotes

BP = 1e-4


@dataclass(frozen=True)
class BermudanPosition:
    label: str
    first_exercise: str   # tenor from trade date, e.g. "1Y"
    tenor: str            # swap tenor from first exercise, e.g. "5Y"
    kind: str             # "payer" or "receiver"
    notional: float       # signed: negative is short
    strike: float


class ModelView:
    def __init__(
        self,
        curve_view: CurveView,
        vol_view: VolView,
        mean_reversion: float = 0.05,
        sigma_guess: float = 0.01,
        mode: str = "sigma",
        expiries: Sequence[str] | None = None,
        tenors: Sequence[str] | None = None,
    ):
        self.cv = curve_view
        self.vv = vol_view
        self.expiries = list(expiries) if expiries else [e for e in vol_view.expiries if fq.Period(e).years >= 1.0]
        self.tenors = list(tenors) if tenors else list(vol_view.tenors)
        self.instruments = self._instruments()
        self.calibration = fq.calibrate_hull_white(self.cv.curve, [(s, v) for _, _, s, v in self.instruments], mode=mode, a=mean_reversion, sigma=sigma_guess)
        self.model = self.calibration.model

    # -- calibration ----------------------------------------------------------

    def _instruments(self) -> list[tuple[str, str, fq.Swaption, float]]:
        out = []
        for e in self.expiries:
            ei = self.vv.expiries.index(e)
            exp_date = fq.Period(e).add_to(self.cv.trade_date)
            for t in self.tenors:
                ti = self.vv.tenors.index(t)
                f = self.vv.forward(e, t)
                sw = fq.Swaption.usd_sofr(exp_date, t, "payer", 1e6, f)
                out.append((e, t, sw, self.vv.cube.node(ei, ti)["atm_vol"]))
        return out

    def fit_rows(self) -> list[dict]:
        rows = []
        for (e, t, sw, mkt), fit in zip(self.instruments, self.calibration.fits()):
            rows.append(
                {
                    "expiry": e,
                    "tenor": t,
                    "forward": sw.strike,
                    "market_vol": mkt,
                    "model_vol": fit["model_vol"],
                    "error": (fit["model_vol"] - mkt) if fit["model_vol"] is not None else None,
                    "market_pv": fit["market_pv"],
                    "model_pv": fit["model_pv"],
                }
            )
        return rows

    def format_fit(self) -> str:
        c = self.calibration
        m = c.model
        head = f"{'Model bp':<8}" + "".join(f"{t:>9}" for t in self.tenors)
        lines = [
            f"Hull-White fit  ref {self.cv.trade_date}  a={m.mean_reversion:.4f}  sigma={m.sigma * 100:.4f}%  rmse={c.rmse / BP:.2f}bp  "
            f"({len(self.instruments)} ATM swaptions, {c.iterations} iterations, {'converged' if c.converged else 'not converged'})",
            head,
            "-" * len(head),
        ]
        by = {(r["expiry"], r["tenor"]): r for r in self.fit_rows()}
        for e in self.expiries:
            lines.append(f"{e:<8}" + "".join(f"{by[(e, t)]['model_vol'] / BP:9.1f}" for t in self.tenors))
        lines.append("")
        lines.append(f"{'Err bp':<8}" + "".join(f"{t:>9}" for t in self.tenors))
        for e in self.expiries:
            lines.append(f"{e:<8}" + "".join(f"{by[(e, t)]['error'] / BP:9.1f}" for t in self.tenors))
        return "\n".join(lines)

    # -- Bermudans ------------------------------------------------------------

    def bermudan(self, p: BermudanPosition) -> fq.BermudanSwaption:
        first = fq.Period(p.first_exercise).add_to(self.cv.trade_date)
        return fq.BermudanSwaption.usd_sofr_coterminal(first, p.tenor, p.kind, abs(p.notional), p.strike)

    def bermudan_rows(self, positions: Sequence[BermudanPosition], steps_per_year: int = 100) -> list[dict]:
        rows = []
        for p in positions:
            b = self.bermudan(p)
            sign = -1.0 if p.notional < 0 else 1.0
            berm = self.model.bermudan_swaption(b, steps_per_year)
            first = fq.Swaption(b.exercise_dates[0], p.kind, b.underlying)
            eur = self.model.european_swaption(first)
            # Parallel DV01 and vega hold (a, sigma) fixed: no refit on the bump.
            up = fq.HullWhite(self.cv.curve.shifted(BP), self.model.mean_reversion, self.model.sigma).bermudan_swaption(b, steps_per_year)
            dn = fq.HullWhite(self.cv.curve.shifted(-BP), self.model.mean_reversion, self.model.sigma).bermudan_swaption(b, steps_per_year)
            vega = fq.HullWhite(self.cv.curve, self.model.mean_reversion, self.model.sigma + BP).bermudan_swaption(b, steps_per_year) - berm
            rows.append(
                {
                    "label": p.label,
                    "kind": p.kind,
                    "exercises": len(b.exercise_dates),
                    "first_exercise": b.exercise_dates[0],
                    "maturity": b.underlying.maturity_date,
                    "notional": p.notional,
                    "strike": p.strike,
                    "forward": eur.forward_rate,
                    "european_npv": sign * eur.pv,
                    "npv": sign * berm,
                    "premium": sign * (berm - eur.pv),
                    "dv01": sign * (up - dn) / 2,
                    "vega": sign * vega,
                    "european_vol": eur.implied_normal_vol,
                }
            )
        return rows

    def format_bermudans(self, positions: Sequence[BermudanPosition], steps_per_year: int = 100) -> str:
        rows = self.bermudan_rows(positions, steps_per_year)
        head = (
            f"{'Trade':<14} {'Kind':<8} {'Ex':>3} {'First':>10} {'Maturity':>10} {'Notional(mm)':>12} {'Strike %':>8} {'Fwd %':>7} "
            f"{'European':>11} {'Bermudan':>11} {'Premium':>9} {'DV01':>8} {'Vega/bp':>8}"
        )
        lines = [f"Bermudan swaptions  trade {self.cv.trade_date}  Hull-White a={self.model.mean_reversion:.3f} sigma={self.model.sigma * 1e4:.1f}bp  ({len(rows)} lines)", head, "-" * len(head)]
        for r in rows:
            lines.append(
                f"{r['label']:<14} {r['kind']:<8} {r['exercises']:>3} {str(r['first_exercise']):>10} {str(r['maturity']):>10} {r['notional'] / 1e6:>12.2f} "
                f"{r['strike'] * 100:8.4f} {r['forward'] * 100:7.4f} {r['european_npv']:>11,.0f} {r['npv']:>11,.0f} {r['premium']:>9,.0f} {r['dv01']:>8,.0f} {r['vega']:>8,.0f}"
            )
        lines.append("-" * len(head))
        t = {k: sum(r[k] for r in rows) for k in ("european_npv", "npv", "premium", "dv01", "vega")}
        lines.append(
            f"{'TOTAL':<14} {'':<8} {'':>3} {'':>10} {'':>10} {'':>12} {'':>8} {'':>7} "
            f"{t['european_npv']:>11,.0f} {t['npv']:>11,.0f} {t['premium']:>9,.0f} {t['dv01']:>8,.0f} {t['vega']:>8,.0f}"
        )
        return "\n".join(lines)

    # -- simulation -----------------------------------------------------------

    def path_rows(self, horizons: Sequence[float] = (1.0, 2.0, 5.0, 10.0), bond_tenor: float = 5.0, paths: int = 20_000, seed: int = 12345) -> list[dict]:
        sim = self.model.simulate(list(horizons), paths=paths, seed=seed)
        rows = []
        for k, t in enumerate(sim.times()):
            df, df_se = sim.estimate(lambda p, k=k: sim.discount_to(p, k))
            bond, bond_se = sim.discounted_bond(k, t + bond_tenor)
            rates = sim.short_rates(k)
            mean_r = sum(rates) / len(rates)
            rows.append(
                {
                    "t": t,
                    "curve_df": self.cv.curve.discount_t(t),
                    "mc_df": df,
                    "mc_df_se": df_se,
                    "curve_bond": self.cv.curve.discount_t(t + bond_tenor),
                    "mc_bond": bond,
                    "mc_bond_se": bond_se,
                    "mean_short_rate": mean_r,
                    "model_short_rate": self.model.short_rate_mean(t),
                    "short_rate_sd": self.model.variance(t) ** 0.5,
                }
            )
        return rows

    def format_paths(self, horizons: Sequence[float] = (1.0, 2.0, 5.0, 10.0), bond_tenor: float = 5.0, paths: int = 20_000) -> str:
        rows = self.path_rows(horizons, bond_tenor, paths)
        head = f"{'t':>5} {'DF curve':>10} {'DF MC':>10} {'se':>8} {f'P(t,t+{bond_tenor:g})':>12} {'MC':>10} {'se':>8} {'E[r] %':>7} {'MC %':>7} {'sd bp':>6}"
        lines = [f"Hull-White paths  {paths:,} antithetic paths  (bank account and bonds vs the curve)", head, "-" * len(head)]
        for r in rows:
            lines.append(
                f"{r['t']:5.1f} {r['curve_df']:10.6f} {r['mc_df']:10.6f} {r['mc_df_se']:8.1e} {r['curve_bond']:12.6f} {r['mc_bond']:10.6f} {r['mc_bond_se']:8.1e} "
                f"{r['model_short_rate'] * 100:7.3f} {r['mean_short_rate'] * 100:7.3f} {r['short_rate_sd'] / BP:6.0f}"
            )
        return "\n".join(lines)

    def print_fit(self) -> None:
        print(self.format_fit())

    def print_bermudans(self, positions: Sequence[BermudanPosition], steps_per_year: int = 100) -> None:
        print(self.format_bermudans(positions, steps_per_year))

    def print_paths(self, horizons: Sequence[float] = (1.0, 2.0, 5.0, 10.0), bond_tenor: float = 5.0, paths: int = 20_000) -> None:
        print(self.format_paths(horizons, bond_tenor, paths))

    def to_dataframe(self):
        import pandas as pd

        return pd.DataFrame(self.fit_rows())


def sample_bermudans() -> list[BermudanPosition]:
    return [
        BermudanPosition("1Yx5Y berm pay", "1Y", "5Y", "payer", 100e6, 0.040),
        BermudanPosition("1Yx10Y berm pay", "1Y", "10Y", "payer", 50e6, 0.045),
        BermudanPosition("2Yx5Y berm rec", "2Y", "5Y", "receiver", -75e6, 0.035),
        BermudanPosition("5Yx5Y berm rec", "5Y", "5Y", "receiver", 100e6, 0.042),
    ]


def main(argv: Sequence[str]) -> int:
    trade = argv[1] if len(argv) > 1 else dt.date.today().isoformat()
    cv = CurveView(trade, sample_quotes())
    vv = VolView(cv, sample_cube_quotes(cv))
    mv = ModelView(cv, vv, mean_reversion=0.05)
    mv.print_fit()
    print()
    mv.print_bermudans(sample_bermudans())
    print()
    mv.print_paths()
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
