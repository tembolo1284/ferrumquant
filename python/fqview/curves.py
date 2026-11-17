"""SOFR curve view: quotes in, bootstrapped nodes and par-quote risk out.

Usage::

    python -m fqview.curves                 # sample quote sheet, today's date
    python -m fqview.curves 2026-10-13      # sample quote sheet, given trade date

or from Python::

    from fqview import CurveView, sample_quotes
    cv = CurveView("2026-10-13", sample_quotes())
    cv.print()                                   # node table
    ladder = cv.par_ladder(lambda c: c.discount("2031-10-17") * 1e6)
    cv.print_ladder(ladder, "DF(5Y) x 1mm")      # DV01 per quote

Quotes are tuples, the same shape the Rust bootstrap takes:
    ("DEPO", "1D", 0.0432)            cash deposit, simple ACT/360
    ("SR3",  "Z26", 95.78, 0.0002)    SOFR future: IMM code, price, convexity (rate units)
    ("OIS",  "5Y", 0.0378)            spot-starting USD SOFR OIS par rate

Conventions (all from the Rust side):
    * Log-linear discount factors (piecewise-flat forwards) by default.
    * Zero rates are continuously compounded on ACT/365F from the trade date.
    * Par-quote DV01 is dPV for a +1bp move in that quote with the curve
      re-bootstrapped, via the Jacobian (not a bump-and-rebuild per quote).
"""

from __future__ import annotations

import datetime as dt
import sys
from typing import Callable, Optional, Sequence

import ferrumquant as fq

from .bonds import DateInput

Quote = tuple
PriceFn = Callable[[fq.DiscountCurve], float]


class CurveView:
    """Bootstraps once (with Jacobian) and presents the curve."""

    def __init__(self, trade_date: DateInput, quotes: Sequence[Quote], interpolation: str = "log_linear", bump: float = 1e-4):
        self.trade_date = fq.Date.from_any(trade_date)
        self.quotes = list(quotes)
        self.index = fq.OvernightIndex.sofr()
        self.result, self.jacobian = fq.bootstrap_sofr_with_jacobian(self.trade_date, self.index, self.quotes, bump=bump, interpolation=interpolation)
        self.curve = self.result.curve

    # -- data ---------------------------------------------------------------

    @staticmethod
    def _quote_value(q: Quote) -> float:
        return float(q[2])

    def rows(self) -> list[dict]:
        rows = self.result.table()
        for q, r in zip(self.quotes, rows):
            r["quote"] = self._quote_value(q)
            r["kind"] = q[0].upper()
            # 3M simple ACT/360 forward starting at the pillar: what the next
            # futures strip or the money market would quote off this curve.
            start = r["pillar"]
            end = fq.Period("3M").add_to(start)
            r["fwd_3m"] = self.curve.forward_rate(start, end)
        return rows

    def par_ladder(self, price_fn: PriceFn, bp: float = 1e-4) -> list[dict]:
        """DV01 of `price_fn(curve)` to each input quote, in currency per `bp`."""
        zero = fq.zero_bucket_ladder(self.curve, price_fn)
        par = self.jacobian.to_par_ladder(zero)
        return [
            {"key": k, "zero_dv01": z * bp, "par_dv01": p * bp}
            for k, z, p in zip(self.result.keys, zero, par)
        ]

    def parallel_dv01(self, price_fn: PriceFn, bp: float = 1e-4) -> float:
        """Central-difference PV change for a +1bp parallel zero shift."""
        up, dn = price_fn(self.curve.shifted(bp)), price_fn(self.curve.shifted(-bp))
        return (up - dn) / 2

    # -- presentation -------------------------------------------------------

    def format(self) -> str:
        rows = self.rows()
        head = f"{'Instrument':<10} {'Quote':>9} {'Pillar':>11} {'T':>7} {'DF':>12} {'Zero %':>9} {'3M Fwd %':>9} {'Reprice':>10}"
        lines = [f"SOFR curve  trade {self.trade_date}  {self.curve.interpolation}  ({len(rows)} nodes)", head, "-" * len(head)]
        for r in rows:
            quote = f"{r['quote']:9.4f}" if r["kind"] == "SR3" else f"{r['quote'] * 100:9.4f}"
            lines.append(
                f"{r['key']:<10} {quote} {str(r['pillar']):>11} {r['time']:7.3f} {r['df']:12.9f} "
                f"{r['zero'] * 100:9.5f} {r['fwd_3m'] * 100:9.5f} {r['repricing_error']:10.1e}"
            )
        return "\n".join(lines)

    def format_ladder(self, ladder: list[dict], title: str = "") -> str:
        head = f"{'Bucket':<10} {'Zero DV01':>14} {'Par DV01':>14}"
        lines = [f"Bucketed risk{': ' + title if title else ''}  (per 1bp)", head, "-" * len(head)]
        for r in ladder:
            lines.append(f"{r['key']:<10} {r['zero_dv01']:14,.2f} {r['par_dv01']:14,.2f}")
        lines.append("-" * len(head))
        lines.append(f"{'TOTAL':<10} {sum(r['zero_dv01'] for r in ladder):14,.2f} {sum(r['par_dv01'] for r in ladder):14,.2f}")
        return "\n".join(lines)

    def print(self) -> None:
        print(self.format())

    def print_ladder(self, ladder: list[dict], title: str = "") -> None:
        print(self.format_ladder(ladder, title))

    def to_dataframe(self):
        import pandas as pd  # optional dependency

        return pd.DataFrame(self.rows()).set_index("key")


def sample_quotes() -> list[Quote]:
    """Deposit + first four SR3 contracts + OIS out to 30Y (made-up levels)."""
    return [
        ("DEPO", "1D", 0.0432),
        ("SR3", "Z26", 95.78, 0.0002),
        ("SR3", "H27", 95.95, 0.0002),
        ("SR3", "M27", 96.10, 0.0002),
        ("SR3", "U27", 96.20, 0.0002),
        ("OIS", "2Y", 0.0380),
        ("OIS", "3Y", 0.0375),
        ("OIS", "5Y", 0.0378),
        ("OIS", "7Y", 0.0385),
        ("OIS", "10Y", 0.0395),
        ("OIS", "20Y", 0.0405),
        ("OIS", "30Y", 0.0410),
    ]


def main(argv: Sequence[str]) -> int:
    trade = argv[1] if len(argv) > 1 else dt.date.today().isoformat()
    cv = CurveView(trade, sample_quotes())
    cv.print()
    print()
    # Risk of a 5Y zero-coupon claim of 1mm: all of it should sit in the 5Y bucket.
    pillar_5y = next(r["pillar"] for r in cv.rows() if r["key"] == "OIS 5Y")
    ladder = cv.par_ladder(lambda c: c.discount(pillar_5y) * 1_000_000)
    cv.print_ladder(ladder, f"1mm zero-coupon {pillar_5y}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
