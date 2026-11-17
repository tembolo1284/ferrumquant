"""Vol view: swaption cube (ATM grid + smiles) and a cap/floor strip on the curve.

Usage::

    python -m fqview.vol                # sample cube and caps, today's date
    python -m fqview.vol 2026-10-13

or from Python::

    from fqview import CurveView, VolView, sample_quotes, sample_cube_quotes, sample_caps
    cv = CurveView("2026-10-13", sample_quotes())
    vv = VolView(cv, sample_cube_quotes(cv))
    vv.print_cube()                              # ATM grid and calibrated SABR per node
    vv.print_smile("1Y", "10Y")                  # vol by strike offset
    vv.print_caps(sample_caps())                 # cap/floor PVs, vegas, implied flat vols

Conventions (all from the Rust side):
    * Normal (Bachelier) vols in rate units; printed in bp.
    * Each node's smile is SABR with beta = 0; ATM and (rho, nu) are
      interpolated bilinearly in (expiry time, tenor years), alpha re-anchored.
    * Caplets fix on the last SOFR observation of each annual period.
"""

from __future__ import annotations

import datetime as dt
import sys
from dataclasses import dataclass
from typing import Sequence

import ferrumquant as fq

from .curves import CurveView, sample_quotes

BP = 1e-4


@dataclass(frozen=True)
class CapPosition:
    label: str
    tenor: str
    kind: str            # "cap" or "floor"
    notional: float
    strike: float
    flat_vol: float      # normal vol, rate units


class VolView:
    def __init__(self, curve_view: CurveView, cube_quotes: dict, expiries: Sequence[str] | None = None, tenors: Sequence[str] | None = None):
        self.cv = curve_view
        exp = list(expiries) if expiries else sorted({e for e, _ in cube_quotes}, key=lambda p: fq.Period(p).years)
        ten = list(tenors) if tenors else sorted({t for _, t in cube_quotes}, key=lambda p: fq.Period(p).years)
        self.expiries, self.tenors = exp, ten
        self.cube = fq.SwaptionVolCube(self.cv.trade_date, exp, ten, cube_quotes, default_smile=(-0.2, 0.4))

    # -- cube ---------------------------------------------------------------

    def forward(self, expiry: str, tenor: str) -> float:
        """Forward swap rate for an expiry/tenor off the curve."""
        exp_date = fq.Period(expiry).add_to(self.cv.trade_date)
        sw = fq.Swaption.usd_sofr(exp_date, tenor, "payer", 1.0, 0.0)
        return sw.forward_and_annuity(self.cv.index, self.cv.curve)[0]

    def smile(self, expiry: str, tenor: str, offsets_bp: Sequence[float] = (-100, -50, -25, 0, 25, 50, 100)) -> list[dict]:
        exp_date = fq.Period(expiry).add_to(self.cv.trade_date)
        f = self.forward(expiry, tenor)
        return [
            {"offset_bp": o, "strike": f + o * BP, "vol": self.cube.normal_vol(exp_date, tenor, f, f + o * BP)}
            for o in offsets_bp
        ]

    def format_cube(self) -> str:
        head = f"{'ATM bp':<8}" + "".join(f"{t:>9}" for t in self.tenors)
        lines = [f"Swaption cube  ref {self.cube.reference_date}  (normal vol, bp)", head, "-" * len(head)]
        for ei, e in enumerate(self.expiries):
            lines.append(f"{e:<8}" + "".join(f"{self.cube.node(ei, ti)['atm_vol'] / BP:9.1f}" for ti in range(len(self.tenors))))
        lines.append("")
        lines.append(f"{'SABR':<8}{'node':>9} {'fwd %':>8} {'alpha bp':>9} {'rho':>7} {'nu':>7} {'rmse bp':>8}")
        for ei, e in enumerate(self.expiries):
            for ti, t in enumerate(self.tenors):
                n = self.cube.node(ei, ti)
                lines.append(f"{'':<8}{e + 'x' + t:>9} {n['forward'] * 100:8.4f} {n['alpha'] / BP:9.2f} {n['rho']:7.3f} {n['nu']:7.3f} {n['rmse'] / BP:8.3f}")
        return "\n".join(lines)

    def format_smile(self, expiry: str, tenor: str) -> str:
        rows = self.smile(expiry, tenor)
        head = f"{'K-F bp':>7} {'Strike %':>9} {'Vol bp':>8}"
        lines = [f"{expiry}x{tenor} smile  forward {self.forward(expiry, tenor) * 100:.4f}%", head, "-" * len(head)]
        for r in rows:
            lines.append(f"{r['offset_bp']:7.0f} {r['strike'] * 100:9.4f} {r['vol'] / BP:8.2f}")
        return "\n".join(lines)

    # -- caps / floors ------------------------------------------------------

    def cap_rows(self, positions: Sequence[CapPosition]) -> list[dict]:
        out = []
        for p in positions:
            cf = fq.CapFloor.usd_sofr(self.cv.trade_date, p.tenor, p.kind, abs(p.notional), p.strike)
            sign = -1.0 if p.notional < 0 else 1.0
            pr = cf.price(self.cv.index, self.cv.curve, flat_vol=p.flat_vol)
            up = cf.npv(self.cv.index, self.cv.curve.shifted(BP), flat_vol=p.flat_vol)
            dn = cf.npv(self.cv.index, self.cv.curve.shifted(-BP), flat_vol=p.flat_vol)
            caplets = pr.caplets()
            out.append(
                {
                    "label": p.label,
                    "kind": p.kind,
                    "tenor": p.tenor,
                    "notional": p.notional,
                    "strike": p.strike,
                    "vol": p.flat_vol,
                    "npv": sign * pr.pv,
                    "dv01": sign * (up - dn) / 2,
                    "vega": sign * pr.vega * BP,
                    "caplets": len(caplets),
                    "first_forward": caplets[0]["forward"],
                    "last_forward": caplets[-1]["forward"],
                    "implied_vol": cf.implied_flat_vol(self.cv.index, self.cv.curve, pr.pv),
                }
            )
        return out

    def format_caps(self, positions: Sequence[CapPosition]) -> str:
        rows = self.cap_rows(positions)
        head = f"{'Trade':<14} {'Kind':<5} {'Tenor':<5} {'Notional(mm)':>12} {'Strike %':>8} {'Vol bp':>7} {'Fwd1 %':>8} {'FwdN %':>8} {'NPV':>12} {'DV01':>9} {'Vega/bp':>9}"
        lines = [f"Caps/floors  trade {self.cv.trade_date}  ({len(rows)} lines)", head, "-" * len(head)]
        for r in rows:
            lines.append(
                f"{r['label']:<14} {r['kind']:<5} {r['tenor']:<5} {r['notional'] / 1e6:>12.2f} {r['strike'] * 100:8.4f} {r['vol'] / BP:7.1f} "
                f"{r['first_forward'] * 100:8.4f} {r['last_forward'] * 100:8.4f} {r['npv']:>12,.0f} {r['dv01']:>9,.0f} {r['vega']:>9,.0f}"
            )
        lines.append("-" * len(head))
        t = {k: sum(r[k] for r in rows) for k in ("npv", "dv01", "vega")}
        lines.append(f"{'TOTAL':<14} {'':<5} {'':<5} {'':>12} {'':>8} {'':>7} {'':>8} {'':>8} {t['npv']:>12,.0f} {t['dv01']:>9,.0f} {t['vega']:>9,.0f}")
        return "\n".join(lines)

    def print_cube(self) -> None:
        print(self.format_cube())

    def print_smile(self, expiry: str, tenor: str) -> None:
        print(self.format_smile(expiry, tenor))

    def print_caps(self, positions: Sequence[CapPosition]) -> None:
        print(self.format_caps(positions))


def sample_cube_quotes(cv: CurveView, expiries: Sequence[str] = ("3M", "1Y", "2Y", "5Y", "10Y"), tenors: Sequence[str] = ("1Y", "2Y", "5Y", "10Y", "30Y")) -> dict:
    """ATM grid with a mild hump at 1Y-2Y expiries, forwards from the curve, and
    a 25-delta-ish RR/BF smile on each node (made-up levels)."""
    atm = {
        "3M": [78, 82, 86, 84, 76],
        "1Y": [88, 92, 94, 90, 80],
        "2Y": [90, 93, 94, 89, 78],
        "5Y": [84, 86, 86, 82, 72],
        "10Y": [74, 76, 76, 72, 64],
    }
    quotes = {}
    vv = VolView.__new__(VolView)
    vv.cv = cv
    for e in expiries:
        for ti, t in enumerate(tenors):
            f = vv.forward(e, t)
            v = atm[e][ti] * BP
            # Skew: low strikes richer; curvature symmetric.
            smile = [(-0.01, v * 1.10), (-0.005, v * 1.04), (0.005, v * 0.99), (0.01, v * 1.02)]
            quotes[(e, t)] = {"forward": f, "atm_vol": v, "smile": smile}
    return quotes


def sample_caps() -> list[CapPosition]:
    return [
        CapPosition("2Y cap 4%", "2Y", "cap", 100e6, 0.040, 0.0085),
        CapPosition("5Y cap 4.5%", "5Y", "cap", 100e6, 0.045, 0.0080),
        CapPosition("5Y floor 3%", "5Y", "floor", -100e6, 0.030, 0.0080),
        CapPosition("10Y cap 5%", "10Y", "cap", 50e6, 0.050, 0.0072),
    ]


def main(argv: Sequence[str]) -> int:
    trade = argv[1] if len(argv) > 1 else dt.date.today().isoformat()
    cv = CurveView(trade, sample_quotes())
    vv = VolView(cv, sample_cube_quotes(cv))
    vv.print_cube()
    print()
    vv.print_smile("1Y", "10Y")
    print()
    vv.print_caps(sample_caps())
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
