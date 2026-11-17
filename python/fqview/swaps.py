"""Swap and swaption blotter on a bootstrapped SOFR curve.

Usage::

    python -m fqview.swaps                 # sample book, today's date
    python -m fqview.swaps 2026-10-13

or from Python::

    from fqview import CurveView, RatesBlotter, SwapPosition, SwaptionPosition, sample_quotes
    cv = CurveView("2026-10-13", sample_quotes())
    book = RatesBlotter(cv, [SwapPosition("5Y payer", "5Y", "pay", 100e6, 0.0378), ...])
    book.print()            # per-line PV and risk
    book.print_ladder()     # book DV01 by curve quote (Jacobian-based par ladder)

Conventions (all from the Rust side):
    * NPV is from the holder's side: a payer swap gains when rates rise.
    * PV01 is the annuity per bp: PV of 1bp of fixed coupon on the notional.
    * DV01 is the NPV change for +1bp (parallel zero shift for the column,
      per-quote re-bootstrap for the ladder). Swaption delta is per 1bp of
      forward, vega per 1bp of normal vol.
"""

from __future__ import annotations

import datetime as dt
import sys
from dataclasses import dataclass
from typing import Optional, Sequence, Union

import ferrumquant as fq

from .bonds import DateInput
from .curves import CurveView, sample_quotes

BP = 1e-4


@dataclass(frozen=True)
class SwapPosition:
    """Spot-starting OIS by tenor, or dated with ``effective``/``maturity``."""

    label: str
    tenor: Optional[str]
    side: str                    # "pay" or "receive" (fixed)
    notional: float
    fixed_rate: float
    effective: Optional[DateInput] = None
    maturity: Optional[DateInput] = None

    def build(self, trade_date: fq.Date) -> fq.OisSwap:
        if self.effective is not None and self.maturity is not None:
            return fq.OisSwap.usd_sofr(self.effective, self.maturity, self.side, self.notional, self.fixed_rate)
        if self.tenor is None:
            raise ValueError(f"{self.label}: give a tenor or effective/maturity dates")
        return fq.OisSwap.usd_sofr_spot(trade_date, self.tenor, self.side, self.notional, self.fixed_rate)


@dataclass(frozen=True)
class SwaptionPosition:
    """European swaption; ``normal_vol`` in rate units (0.0080 = 80bp)."""

    label: str
    expiry: DateInput
    tenor: str
    kind: str                    # "payer" or "receiver"
    notional: float              # negative for a short position
    strike: float
    normal_vol: float

    def build(self) -> fq.Swaption:
        return fq.Swaption.usd_sofr(self.expiry, self.tenor, self.kind, abs(self.notional), self.strike)

    @property
    def sign(self) -> float:
        return -1.0 if self.notional < 0 else 1.0


Position = Union[SwapPosition, SwaptionPosition]


class RatesBlotter:
    """Values a swap/swaption book on a ``CurveView``'s curve."""

    def __init__(self, curve_view: CurveView, positions: Sequence[Position]):
        self.cv = curve_view
        self.index = curve_view.index
        self.positions = list(positions)
        self._swaps = [(p, p.build(self.cv.trade_date)) for p in self.positions if isinstance(p, SwapPosition)]
        self._swaptions = [(p, p.build()) for p in self.positions if isinstance(p, SwaptionPosition)]

    # -- pricing ------------------------------------------------------------

    def npv(self, curve: Optional[fq.DiscountCurve] = None) -> float:
        """Book NPV on `curve` (default: the view's curve). Used for ladders."""
        c = curve if curve is not None else self.cv.curve
        total = 0.0
        for _, s in self._swaps:
            total += s.npv(self.index, c)
        for p, sw in self._swaptions:
            total += p.sign * sw.price(self.index, c, normal_vol=p.normal_vol).pv
        return total

    def rows(self) -> list[dict]:
        c = self.cv.curve
        out = []
        for p, s in self._swaps:
            v = s.value(self.index, c)
            out.append(
                {
                    "label": p.label,
                    "type": "OIS",
                    "side": p.side,
                    "notional": p.notional,
                    "start": s.effective_date,
                    "end": s.maturity_date,
                    "rate": p.fixed_rate,
                    "par_rate": v.par_rate,
                    "npv": v.npv,
                    "pv01": v.annuity * BP,
                    "dv01": s.dv01(self.index, c),
                    "vega": 0.0,
                    "gamma": 0.0,
                }
            )
        for p, sw in self._swaptions:
            pr = sw.price(self.index, c, normal_vol=p.normal_vol)
            # Parallel DV01 of the option by bumping the curve, consistent with the swaps.
            up = sw.price(self.index, c.shifted(BP), normal_vol=p.normal_vol).pv
            dn = sw.price(self.index, c.shifted(-BP), normal_vol=p.normal_vol).pv
            out.append(
                {
                    "label": p.label,
                    "type": "SWPN",
                    "side": p.kind,
                    "notional": p.notional,
                    "start": sw.underlying.effective_date,
                    "end": sw.underlying.maturity_date,
                    "rate": p.strike,
                    "par_rate": pr.forward_rate,
                    "npv": p.sign * pr.pv,
                    "pv01": p.sign * pr.annuity * BP,
                    "dv01": p.sign * (up - dn) / 2,
                    "delta_bp": p.sign * pr.delta * BP,
                    "gamma": p.sign * pr.gamma * BP * BP,
                    "vega": p.sign * pr.vega * BP,
                    "expiry": sw.expiry,
                    "vol": p.normal_vol,
                }
            )
        return out

    def ladder(self) -> list[dict]:
        return self.cv.par_ladder(self.npv)

    def totals(self, rows: Optional[list[dict]] = None) -> dict:
        rows = self.rows() if rows is None else rows
        return {k: sum(r[k] for r in rows) for k in ("npv", "pv01", "dv01", "vega", "gamma")}

    # -- presentation -------------------------------------------------------

    def format(self) -> str:
        rows = self.rows()
        t = self.totals(rows)
        head = (
            f"{'Trade':<16} {'Type':<4} {'Side':<8} {'Notional(mm)':>12} {'Start':>11} {'End':>11} "
            f"{'Rate %':>8} {'Fwd/Par %':>9} {'NPV':>14} {'PV01':>10} {'DV01':>10} {'Vega/bp':>10}"
        )
        lines = [f"Rates blotter  trade {self.cv.trade_date}  ({len(rows)} lines)", head, "-" * len(head)]
        for r in rows:
            lines.append(
                f"{r['label']:<16} {r['type']:<4} {r['side']:<8} {r['notional'] / 1e6:>12.2f} {str(r['start']):>11} {str(r['end']):>11} "
                f"{r['rate'] * 100:>8.4f} {r['par_rate'] * 100:>9.4f} {r['npv']:>14,.0f} {r['pv01']:>10,.0f} {r['dv01']:>10,.0f} {r['vega']:>10,.0f}"
            )
        lines.append("-" * len(head))
        lines.append(f"{'TOTAL':<16} {'':<4} {'':<8} {'':>12} {'':>11} {'':>11} {'':>8} {'':>9} {t['npv']:>14,.0f} {t['pv01']:>10,.0f} {t['dv01']:>10,.0f} {t['vega']:>10,.0f}")
        return "\n".join(lines)

    def print(self) -> None:
        print(self.format())

    def print_ladder(self) -> None:
        self.cv.print_ladder(self.ladder(), "book NPV")

    def to_dataframe(self):
        import pandas as pd  # optional dependency

        return pd.DataFrame(self.rows()).set_index("label")


def sample_rates_book() -> list[Position]:
    """A 2s5s10s butterfly, a receiver, and a long 1Yx10Y payer hedged with a short 5Yx5Y."""
    return [
        SwapPosition("2Y pay", "2Y", "pay", 100e6, 0.0380),
        SwapPosition("5Y rec", "5Y", "receive", 200e6, 0.0378),
        SwapPosition("10Y pay", "10Y", "pay", 100e6, 0.0395),
        SwapPosition("30Y rec", "30Y", "receive", 25e6, 0.0425),
        SwapPosition("fwd 1Y5Y pay", None, "pay", 50e6, 0.0390, effective="2027-10-15", maturity="2032-10-15"),
        SwaptionPosition("1Yx10Y payer", "2027-10-13", "10Y", "payer", 100e6, 0.0400, 0.0080),
        SwaptionPosition("5Yx5Y payer", "2031-10-13", "5Y", "payer", -50e6, 0.0425, 0.0075),
        SwaptionPosition("2Yx2Y rec", "2028-10-13", "2Y", "receiver", 100e6, 0.0350, 0.0085),
    ]


def main(argv: Sequence[str]) -> int:
    trade = argv[1] if len(argv) > 1 else dt.date.today().isoformat()
    cv = CurveView(trade, sample_quotes())
    book = RatesBlotter(cv, sample_rates_book())
    book.print()
    print()
    book.print_ladder()
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
