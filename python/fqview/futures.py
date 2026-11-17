"""Treasury futures basis screen: deliverable basket, CTD, net basis, delivery option.

Usage::

    python -m fqview.futures                # sample ZN basket, today's date
    python -m fqview.futures 2027-01-04

or from Python::

    from fqview import BasisScreen, Deliverable
    screen = BasisScreen("ZN", "H27", futures_price=112.25, repo=0.0385, trade_date="2027-01-04",
                         deliverables=[Deliverable("T 4 1/4 08/36", "2026-08-15", "2036-08-15", 0.0425, "99-08"), ...])
    screen.print()

Conventions (all from the Rust side):
    * Prices per 100 face; CF per CME (6% yield, coupon rounded to 1/8).
    * Gross basis = clean - F*CF; carry = clean - forward clean (coupon income
      less repo financing to delivery); net basis = gross - carry.
    * CTD by highest implied repo; fair price = min forward clean / CF.
    * Delivery (switch) option under a parallel normal yield shift to the
      last delivery date; timing and wildcard options are not included.
"""

from __future__ import annotations

import datetime as dt
import sys
from dataclasses import dataclass
from typing import Optional, Sequence

import ferrumquant as fq

from .bonds import DateInput, PriceInput


@dataclass(frozen=True)
class Deliverable:
    label: str
    dated: DateInput
    maturity: DateInput
    coupon: float
    price: PriceInput        # clean, decimal or 32nds

    def bond(self) -> fq.FixedRateBond:
        return fq.FixedRateBond.us_treasury(self.dated, self.maturity, self.coupon)

    def clean_price(self) -> float:
        return fq.parse_32nds(self.price) if isinstance(self.price, str) else float(self.price)


class BasisScreen:
    def __init__(
        self,
        contract: str,
        delivery: str,
        futures_price: float,
        repo: float,
        trade_date: DateInput,
        deliverables: Sequence[Deliverable],
        yield_vol: float = 0.009,
        delivery_date: Optional[DateInput] = None,
    ):
        self.contract, self.delivery, self.futures_price, self.repo = contract, delivery, futures_price, repo
        self.trade_date = fq.Date.from_any(trade_date)
        self.future = fq.TreasuryFuture(contract, delivery, [d.bond() for d in deliverables])
        # Keep only the deliverables that survived the eligibility filter, in order.
        kept = {str(b.maturity): b for b, _ in self.future.basket()}
        self.deliverables = [d for d in deliverables if str(fq.Date.from_any(d.maturity)) in kept]
        self.settle = self.future.basket()[0][0].settlement_date(self.trade_date)
        self.delivery_date = fq.Date.from_any(delivery_date) if delivery_date is not None else self.future.last_delivery_date()
        self.yield_vol = yield_vol

    def prices(self) -> list[float]:
        return [d.clean_price() for d in self.deliverables]

    def rows(self) -> list[dict]:
        prices = self.prices()
        ctd, basis = self.future.cheapest_to_deliver(prices, self.futures_price, self.settle, self.delivery_date, self.repo)
        opt = self.future.delivery_option(prices, self.settle, self.delivery_date, self.repo, yield_vol=self.yield_vol)
        out = []
        for i, (d, (bond, cf), b) in enumerate(zip(self.deliverables, self.future.basket(), basis)):
            a = bond.analytics_from_price(b["clean_price"], self.settle)
            out.append(
                {
                    "label": d.label,
                    "coupon": d.coupon,
                    "maturity": bond.maturity,
                    "price": b["clean_price"],
                    "price_32": fq.format_32nds(b["clean_price"]),
                    "yield": a.yield_rate,
                    "cf": cf,
                    "gross_basis_32": b["gross_basis"] * 32,
                    "carry_32": b["carry"] * 32,
                    "net_basis_32": b["net_basis"] * 32,
                    "implied_repo": b["implied_repo"],
                    "forward_clean": b["forward_clean"],
                    "converted_forward": b["converted_forward"],
                    "forward_yield": opt.forward_yields[i],
                    "ctd_probability": opt.ctd_probability[i],
                    "is_ctd": i == ctd,
                }
            )
        return out

    def summary(self) -> dict:
        prices = self.prices()
        ctd, fair = self.future.fair_price(prices, self.settle, self.delivery_date, self.repo)
        opt = self.future.delivery_option(prices, self.settle, self.delivery_date, self.repo, yield_vol=self.yield_vol)
        return {
            "futures_price": self.futures_price,
            "fair_price_ctd_only": fair,
            "fair_price_with_option": opt.fair_price,
            "switch_option_32": opt.switch_option * 32,
            "rich_cheap_32": (self.futures_price - opt.fair_price) * 32,
            "ctd": self.deliverables[ctd].label,
            "most_likely_ctd": self.deliverables[opt.most_likely_ctd()].label,
            "settle": self.settle,
            "delivery_date": self.delivery_date,
        }

    def format(self) -> str:
        rows, s = self.rows(), self.summary()
        head = f"{'Deliverable':<16} {'Price':>9} {'Yield %':>8} {'CF':>7} {'Gross':>7} {'Carry':>7} {'Net':>7} {'Impl Repo %':>11} {'Fwd Yld %':>9} {'P(CTD)':>7}"
        lines = [
            f"{self.contract} {self.delivery} basis  trade {self.trade_date}  settle {s['settle']}  delivery {s['delivery_date']}  "
            f"F={self.futures_price:.4f}  repo={self.repo * 100:.3f}%  (basis in 32nds)",
            head,
            "-" * len(head),
        ]
        for r in rows:
            mark = "*" if r["is_ctd"] else " "
            lines.append(
                f"{r['label']:<15}{mark} {r['price_32']:>9} {r['yield'] * 100:8.4f} {r['cf']:7.4f} {r['gross_basis_32']:7.2f} "
                f"{r['carry_32']:7.2f} {r['net_basis_32']:7.2f} {r['implied_repo'] * 100:11.3f} {r['forward_yield'] * 100:9.4f} {r['ctd_probability']:7.1%}"
            )
        lines.append("-" * len(head))
        lines.append(
            f"CTD {s['ctd']} (most likely at delivery: {s['most_likely_ctd']})   fair {s['fair_price_ctd_only']:.4f} ctd-only, "
            f"{s['fair_price_with_option']:.4f} with switch option ({s['switch_option_32']:.2f}/32)   futures rich/cheap {s['rich_cheap_32']:+.2f}/32"
        )
        return "\n".join(lines)

    def print(self) -> None:
        print(self.format())

    def to_dataframe(self):
        import pandas as pd  # optional dependency

        return pd.DataFrame(self.rows()).set_index("label")


def sample_basket() -> list[Deliverable]:
    """ZN H27 candidates (prices made up); the 30Y drops out as non-deliverable."""
    return [
        Deliverable("T 3 1/2 02/34", "2024-02-15", "2034-02-15", 0.0350, "96-24"),
        Deliverable("T 3 7/8 08/34", "2024-08-15", "2034-08-15", 0.03875, "99-04"),
        Deliverable("T 4 1/4 02/35", "2025-02-15", "2035-02-15", 0.0425, "101-20"),
        Deliverable("T 4 1/4 08/36", "2026-08-15", "2036-08-15", 0.0425, "99-08"),
        Deliverable("T 4 1/2 08/56", "2026-08-15", "2056-08-15", 0.0450, "98-00"),
    ]


def main(argv: Sequence[str]) -> int:
    trade = argv[1] if len(argv) > 1 else dt.date.today().isoformat()
    BasisScreen("ZN", "H27", futures_price=112.0, repo=0.0385, trade_date=trade, deliverables=sample_basket()).print()
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
