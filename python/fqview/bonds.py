"""Treasury bond blotter: price/yield/risk per line, the way a desk reads it.

Usage::

    python -m fqview                 # sample book, today's date
    python -m fqview 2026-10-16      # sample book, given trade date

or from Python::

    from fqview import TreasuryBlotter, TreasuryPosition
    blotter = TreasuryBlotter("2026-10-16", [TreasuryPosition("T 4 3/8 08/36", "2026-08-15", "2036-08-15", 0.04375, "99-16+", 25e6)])
    blotter.print()
    rows = blotter.rows()        # list of dicts
    df = blotter.to_dataframe()  # if pandas is installed

Conventions (all from the Rust side):
    * Street yield: semiannual bond basis, simple interest in the final period.
    * Prices per 100 face; 32nds accepted as "99-16", "99-16+", "99-162".
    * DV01 per 100 is the dirty-price change for a 1bp *fall* in yield
      (positive for a long). Position DV01 scales by notional / 100.
"""

from __future__ import annotations

import datetime as dt
import sys
from dataclasses import dataclass
from typing import Iterable, Optional, Sequence, Union

import ferrumquant as fq

DateInput = Union[str, dt.date, fq.Date]
PriceInput = Union[str, float]


@dataclass(frozen=True)
class TreasuryPosition:
    """One line of a Treasury book.

    ``price`` is the clean price: a decimal or a 32nds string. ``notional``
    is face amount in currency (positive long, negative short).
    """

    label: str
    dated: DateInput
    maturity: DateInput
    coupon: float
    price: PriceInput
    notional: float = 1_000_000.0
    first_coupon: Optional[DateInput] = None

    def bond(self) -> fq.FixedRateBond:
        return fq.FixedRateBond.us_treasury(self.dated, self.maturity, self.coupon, self.first_coupon)

    def clean_price(self) -> float:
        if isinstance(self.price, str):
            return fq.parse_32nds(self.price)
        return float(self.price)


class TreasuryBlotter:
    """Prices every position on one trade date (T+1 settlement)."""

    COLUMNS = (
        ("label", "Security", 18, "{}"),
        ("notional_mm", "Face(mm)", 9, "{:>9.2f}"),
        ("price_32", "Price", 10, "{:>10}"),
        ("yield_pct", "Yield %", 9, "{:>9.4f}"),
        ("accrued", "Accrued", 9, "{:>9.6f}"),
        ("dirty_price", "Dirty", 11, "{:>11.6f}"),
        ("mod_duration", "ModDur", 8, "{:>8.4f}"),
        ("convexity", "Convex", 8, "{:>8.3f}"),
        ("dv01_per_100", "DV01/100", 9, "{:>9.6f}"),
        ("position_dv01", "Pos DV01", 12, "{:>12,.0f}"),
        ("market_value", "Mkt Value", 16, "{:>16,.2f}"),
    )

    def __init__(self, trade_date: DateInput, positions: Iterable[TreasuryPosition]):
        self.trade_date = fq.Date.from_any(trade_date)
        self.positions: Sequence[TreasuryPosition] = tuple(positions)

    def rows(self) -> list[dict]:
        out = []
        for p in self.positions:
            bond = p.bond()
            settle = bond.settlement_date(self.trade_date)
            clean = p.clean_price()
            a = bond.analytics_from_price(clean, settle)
            scale = p.notional / 100.0
            out.append(
                {
                    "label": p.label,
                    "coupon": p.coupon,
                    "maturity": bond.maturity,
                    "settle": settle,
                    "notional": p.notional,
                    "notional_mm": p.notional / 1e6,
                    "clean_price": a.clean_price,
                    "price_32": fq.format_32nds(a.clean_price),
                    "yield": a.yield_rate,
                    "yield_pct": a.yield_rate * 100.0,
                    "accrued": a.accrued,
                    "dirty_price": a.dirty_price,
                    "mac_duration": a.macaulay_duration,
                    "mod_duration": a.modified_duration,
                    "convexity": a.convexity,
                    "dv01_per_100": a.dv01,
                    "position_dv01": a.dv01 * scale,
                    "market_value": a.dirty_price * scale,
                }
            )
        return out

    def totals(self, rows: Optional[list[dict]] = None) -> dict:
        rows = self.rows() if rows is None else rows
        dv01 = sum(r["position_dv01"] for r in rows)
        mv = sum(r["market_value"] for r in rows)
        face = sum(r["notional"] for r in rows)
        # Dollar-duration-weighted book duration (what the risk sheet shows).
        wdur = sum(r["mod_duration"] * r["market_value"] for r in rows) / mv if mv else float("nan")
        return {"notional": face, "market_value": mv, "position_dv01": dv01, "mod_duration": wdur}

    def format(self) -> str:
        rows = self.rows()
        t = self.totals(rows)
        head = " ".join(f"{title:>{w}}" if key != "label" else f"{title:<{w}}" for key, title, w, _ in self.COLUMNS)
        lines = [
            f"Treasury blotter  trade {self.trade_date}  settle {rows[0]['settle'] if rows else '-'}  ({len(rows)} lines)",
            head,
            "-" * len(head),
        ]
        for r in rows:
            cells = []
            for key, _, w, fmt in self.COLUMNS:
                s = fmt.format(r[key])
                cells.append(f"{s:<{w}}" if key == "label" else f"{s:>{w}}")
            lines.append(" ".join(cells))
        lines.append("-" * len(head))
        lines.append(
            f"{'TOTAL':<18} {t['notional'] / 1e6:>9.2f} {'':>10} {'':>9} {'':>9} {'':>11} "
            f"{t['mod_duration']:>8.4f} {'':>8} {'':>9} {t['position_dv01']:>12,.0f} {t['market_value']:>16,.2f}"
        )
        return "\n".join(lines)

    def print(self) -> None:
        print(self.format())

    def to_dataframe(self):
        import pandas as pd  # optional dependency

        return pd.DataFrame(self.rows()).set_index("label")


def sample_book() -> list[TreasuryPosition]:
    """A small on-the-run-ish ladder with made-up prices (32nds)."""
    return [
        TreasuryPosition("T 3 7/8 09/28", "2026-09-30", "2028-09-30", 0.03875, "99-27", 50e6),
        TreasuryPosition("T 3 3/4 09/30", "2026-09-30", "2030-09-30", 0.03750, "99-05+", 25e6),
        TreasuryPosition("T 3 7/8 09/32", "2026-09-30", "2032-09-30", 0.03875, "99-01", 25e6),
        TreasuryPosition("T 4 3/8 08/36", "2026-08-15", "2036-08-15", 0.04375, "99-16+", 25e6),
        TreasuryPosition("T 4 7/8 08/46", "2026-08-15", "2046-08-15", 0.04875, "98-20", -10e6),
        TreasuryPosition("T 4 7/8 08/56", "2026-08-15", "2056-08-15", 0.04875, "97-12", 10e6),
    ]


def main(argv: Sequence[str]) -> int:
    trade = argv[1] if len(argv) > 1 else dt.date.today().isoformat()
    TreasuryBlotter(trade, sample_book()).print()
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
