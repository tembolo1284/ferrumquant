"""FX view: forward points ladder, smile pillars from ATM/RR/BF quotes, and
an option book with market-convention Greeks.

Usage::

    python -m fqview.fx                 # sample EURUSD and USDJPY, today's date
    python -m fqview.fx 2026-10-13

or from Python::

    from fqview import FxView, FxOptionPosition, sample_fx_market, sample_fx_quotes, sample_fx_book
    view = FxView(sample_fx_market("EURUSD", "2026-10-13"), sample_fx_quotes("EURUSD"))
    view.print_forwards()               # outrights and points by tenor
    view.print_smiles()                 # 10d/25d/ATM strikes and vols per pillar
    view.print_book(sample_fx_book("EURUSD"))

Conventions (all from the Rust side):
    * Pairs in market order; forwards from covered interest parity on the
      two discount curves, dated with the pair's spot lag, joint calendars
      and the end-of-month rule.
    * Premium in USD when the pair has it (EURUSD: quote, USDJPY: base),
      otherwise base. Base-currency premium means premium-adjusted deltas.
    * Deltas are spot delta to 1Y and forward delta beyond; ATM is the
      delta-neutral straddle (premium-adjusted where deltas are).
    * Butterflies are read as smile strangles; each pillar is lognormal
      SABR; time interpolation is in total variance.
"""

from __future__ import annotations

import datetime as dt
import sys
from dataclasses import dataclass
from typing import Optional, Sequence, Union

import ferrumquant as fq

from .bonds import DateInput

PairInput = Union[str, fq.CurrencyPair]

TENORS = ("1W", "2W", "1M", "2M", "3M", "6M", "9M", "1Y", "18M", "2Y")


@dataclass(frozen=True)
class FxOptionPosition:
    """Vanilla by tenor, struck at a level or at a delta such as ``"25dP"`` /
    ``"25dC"`` / ``"ATM"``. ``notional`` is in base currency, negative for a short."""

    label: str
    tenor: str
    kind: str                     # "call" or "put" on the base
    strike: Union[float, str]
    notional: float

    @property
    def sign(self) -> float:
        return -1.0 if self.notional < 0 else 1.0


class FxView:
    """Forwards, smiles and an option book for one pair on one ``FxMarket``."""

    def __init__(self, market: fq.FxMarket, quotes: Sequence, tenors: Sequence[str] = TENORS):
        self.market = market
        self.pair = market.pair
        self.tenors = list(tenors)
        self.surface = fq.FxVolSurface(market, list(quotes))

    # -- forwards -----------------------------------------------------------

    def forward_rows(self) -> list[dict]:
        out = []
        for t in self.tenors:
            vd, fwd = self.market.forward_for_tenor(t)
            out.append(
                {
                    "tenor": t,
                    "value_date": vd,
                    "days": vd - self.market.spot_date,
                    "forward": fwd,
                    "points": self.market.forward_points(vd),
                    "implied_differential": self.market.implied_rate_differential(vd),
                }
            )
        return out

    def format_forwards(self) -> str:
        pip = self.pair.pip
        dp = 2 if pip == 0.01 else 4
        head = f"{'Tenor':<6} {'Value':>11} {'Days':>5} {'Outright':>11} {'Points':>9} {'Impl diff %':>11}"
        lines = [
            f"{self.pair} forwards  trade {self.market.trade_date}  spot {self.market.spot:.{dp}f} ({self.market.spot_date})  "
            f"base {self.pair.base} / quote {self.pair.quote}  pip {pip}",
            head,
            "-" * len(head),
        ]
        for r in self.forward_rows():
            lines.append(
                f"{r['tenor']:<6} {str(r['value_date']):>11} {r['days']:>5} {r['forward']:>11.{dp}f} {r['points']:>9.2f} {r['implied_differential'] * 100:>11.4f}"
            )
        return "\n".join(lines)

    # -- smiles -------------------------------------------------------------

    def smile_rows(self) -> list[dict]:
        out = []
        for s in self.surface.smiles():
            pts = {round(d, 4): (k, v) for d, k, v in s["points"]}
            row = {
                "tenor": str(s["tenor"]),
                "expiry": s["expiry"],
                "delivery": s["delivery"],
                "forward": s["forward"],
                "convention": s["delta_convention"],
                "atm_strike": s["atm_strike"],
                "atm_vol": s["atm_vol"],
                "rmse": s["rmse"],
                "rho": s["sabr"]["rho"],
                "nu": s["sabr"]["nu"],
            }
            for label, d in (("10p", -0.10), ("25p", -0.25), ("25c", 0.25), ("10c", 0.10)):
                k, v = pts.get(d, (float("nan"), float("nan")))
                row[f"k_{label}"], row[f"v_{label}"] = k, v
            row["rr25"] = row["v_25c"] - row["v_25p"]
            row["bf25"] = 0.5 * (row["v_25c"] + row["v_25p"]) - row["atm_vol"]
            out.append(row)
        return out

    def format_smiles(self) -> str:
        dp = 2 if self.pair.pip == 0.01 else 4
        head = (
            f"{'Tenor':<6} {'Expiry':>11} {'Fwd':>9} {'Conv':<10} {'10dP':>8} {'25dP':>8} {'ATM':>8} {'25dC':>8} {'10dC':>8} "
            f"{'RR25':>6} {'BF25':>6} {'rho':>7} {'nu':>6} {'fit bp':>7}"
        )
        lines = [f"{self.pair} smiles  (strike on top, vol % below)", head, "-" * len(head)]
        for r in self.smile_rows():
            ks = "".join(f"{r[f'k_{w}']:>8.{dp}f}" if r[f"k_{w}"] == r[f"k_{w}"] else f"{'':>8}" for w in ("10p", "25p"))
            ks += f"{r['atm_strike']:>8.{dp}f}" + "".join(f"{r[f'k_{w}']:>8.{dp}f}" if r[f"k_{w}"] == r[f"k_{w}"] else f"{'':>8}" for w in ("25c", "10c"))
            vs = "".join(f"{r[f'v_{w}'] * 100:>8.3f}" if r[f"v_{w}"] == r[f"v_{w}"] else f"{'':>8}" for w in ("10p", "25p"))
            vs += f"{r['atm_vol'] * 100:>8.3f}" + "".join(f"{r[f'v_{w}'] * 100:>8.3f}" if r[f"v_{w}"] == r[f"v_{w}"] else f"{'':>8}" for w in ("25c", "10c"))
            lines.append(
                f"{r['tenor']:<6} {str(r['expiry']):>11} {r['forward']:>9.{dp}f} {r['convention']:<10}{ks} "
                f"{r['rr25'] * 100:>6.2f} {r['bf25'] * 100:>6.2f} {r['rho']:>7.3f} {r['nu']:>6.3f} {r['rmse'] * 1e4:>7.2f}"
            )
            lines.append(f"{'':<6} {'':>11} {'':>9} {'':<10}{vs}")
        return "\n".join(lines)

    # -- book ---------------------------------------------------------------

    def build(self, p: FxOptionPosition) -> fq.FxVanilla:
        expiry, delivery = self.pair.expiry_and_delivery(self.market.trade_date, p.tenor)
        if isinstance(p.strike, str):
            spec = p.strike.strip().lower()
            if spec == "atm":
                s = self.surface.slice_at(expiry)
                conv = self.surface.config()["atm"]
                strike = s.forward
                for _ in range(8):  # fixed point: ATM strike depends on the vol at the ATM strike
                    strike = s.atm_strike(conv, self.surface.vol(expiry, strike))
            else:
                n, kind = spec[:-2], spec[-1]
                delta = float(n[:-1] if n.endswith("d") else n) / 100.0
                strike, _ = self.surface.vol_for_delta(expiry, "call" if kind == "c" else "put", delta if kind == "c" else -delta)
        else:
            strike = float(p.strike)
        return fq.FxVanilla(self.pair, p.kind, strike, expiry, delivery, abs(p.notional))

    def book_rows(self, positions: Sequence[FxOptionPosition]) -> list[dict]:
        out = []
        for p in positions:
            o = self.build(p)
            v = self.surface.value(o)
            n = abs(p.notional)
            out.append(
                {
                    "label": p.label,
                    "tenor": p.tenor,
                    "kind": p.kind,
                    "expiry": o.expiry,
                    "strike": o.strike,
                    "notional": p.notional,
                    "vol": v["vol"],
                    "pv_quote": p.sign * v["pv_quote"],
                    "pv_base": p.sign * v["pv_base"],
                    "pct_base": v["pct_base"],
                    "quote_pips": v["quote_pips"],
                    "spot_delta": p.sign * v["spot_delta"] * n,
                    "forward_delta": p.sign * v["forward_delta"] * n,
                    "spot_delta_pa": p.sign * v["spot_delta_pa"] * n,
                    "gamma_1pct": p.sign * v["gamma"] * (0.01 * self.market.spot) ** 2,
                    "vega_1pct": p.sign * v["vega"] * 0.01,
                    "vanna": p.sign * v["vanna"] * 0.01 * 0.01 * self.market.spot,
                    "volga": p.sign * v["volga"] * 0.01 * 0.01,
                }
            )
        return out

    def format_book(self, positions: Sequence[FxOptionPosition]) -> str:
        rows = self.book_rows(positions)
        dp = 2 if self.pair.pip == 0.01 else 4
        q, b = self.pair.quote, self.pair.base
        head = (
            f"{'Trade':<18} {'Tenor':<5} {'Kind':<4} {'Expiry':>11} {'Strike':>9} {'Notl(mm)':>9} {'Vol %':>7} {'%' + b:>7} "
            f"{'PV ' + q:>13} {'Spot d ' + b:>12} {'Fwd d ' + b:>12} {'Gamma 1%':>11} {'Vega 1%':>11} {'Vanna':>10} {'Volga':>10}"
        )
        lines = [f"{self.pair} option book  trade {self.market.trade_date}  ({len(rows)} lines; deltas in {b}, PV/gamma/vega in {q})", head, "-" * len(head)]
        for r in rows:
            lines.append(
                f"{r['label']:<18} {r['tenor']:<5} {r['kind']:<4} {str(r['expiry']):>11} {r['strike']:>9.{dp}f} {r['notional'] / 1e6:>9.2f} {r['vol'] * 100:>7.3f} "
                f"{r['pct_base'] * 100:>7.3f} {r['pv_quote']:>13,.0f} {r['spot_delta']:>12,.0f} {r['forward_delta']:>12,.0f} "
                f"{r['gamma_1pct']:>11,.0f} {r['vega_1pct']:>11,.0f} {r['vanna']:>10,.0f} {r['volga']:>10,.0f}"
            )
        lines.append("-" * len(head))
        t = {k: sum(r[k] for r in rows) for k in ("pv_quote", "spot_delta", "forward_delta", "gamma_1pct", "vega_1pct", "vanna", "volga")}
        lines.append(
            f"{'TOTAL':<18} {'':<5} {'':<4} {'':>11} {'':>9} {'':>9} {'':>7} {'':>7} {t['pv_quote']:>13,.0f} {t['spot_delta']:>12,.0f} "
            f"{t['forward_delta']:>12,.0f} {t['gamma_1pct']:>11,.0f} {t['vega_1pct']:>11,.0f} {t['vanna']:>10,.0f} {t['volga']:>10,.0f}"
        )
        return "\n".join(lines)

    def print_forwards(self) -> None:
        print(self.format_forwards())

    def print_smiles(self) -> None:
        print(self.format_smiles())

    def print_book(self, positions: Sequence[FxOptionPosition]) -> None:
        print(self.format_book(positions))

    def to_dataframe(self, positions: Sequence[FxOptionPosition]):
        import pandas as pd  # optional dependency

        return pd.DataFrame(self.book_rows(positions)).set_index("label")


# ---------------------------------------------------------------------------
# Samples (made-up levels)
# ---------------------------------------------------------------------------

_SAMPLE = {
    "EURUSD": {"spot": 1.1750, "base_rate": 0.020, "quote_rate": 0.040},
    "USDJPY": {"spot": 152.30, "base_rate": 0.040, "quote_rate": 0.005},
    "GBPUSD": {"spot": 1.3150, "base_rate": 0.040, "quote_rate": 0.040},
}

_SAMPLE_QUOTES = {
    "EURUSD": [
        ("1W", 0.0700, -0.0030, 0.0012),
        ("1M", 0.0720, -0.0040, 0.0015),
        ("3M", 0.0750, -0.0060, 0.0020, -0.0110, 0.0065),
        ("6M", 0.0775, -0.0075, 0.0022, -0.0140, 0.0072),
        ("1Y", 0.0800, -0.0090, 0.0025, -0.0165, 0.0080),
        ("2Y", 0.0820, -0.0100, 0.0028),
    ],
    "USDJPY": [
        ("1W", 0.0900, 0.0050, 0.0025),
        ("1M", 0.0950, 0.0080, 0.0030),
        ("3M", 0.0980, 0.0100, 0.0035, 0.0180, 0.0100),
        ("6M", 0.0990, 0.0110, 0.0038),
        ("1Y", 0.1000, 0.0120, 0.0040, 0.0220, 0.0120),
        ("2Y", 0.1010, 0.0125, 0.0042),
    ],
    "GBPUSD": [
        ("1M", 0.0780, -0.0050, 0.0018),
        ("3M", 0.0800, -0.0070, 0.0022),
        ("1Y", 0.0840, -0.0095, 0.0028),
    ],
}


def sample_fx_market(pair: PairInput, trade_date: DateInput, horizon: DateInput = "2040-01-01") -> fq.FxMarket:
    """Flat continuously-compounded curves at sample rates for a pair."""
    p = pair if isinstance(pair, fq.CurrencyPair) else fq.CurrencyPair(pair)
    s = _SAMPLE[str(p)]
    base = fq.DiscountCurve.flat(trade_date, s["base_rate"], horizon)
    quote = fq.DiscountCurve.flat(trade_date, s["quote_rate"], horizon)
    return fq.FxMarket(p, trade_date, s["spot"], base, quote)


def sample_fx_quotes(pair: PairInput) -> list[tuple]:
    return list(_SAMPLE_QUOTES[str(pair)])


def sample_fx_book(pair: PairInput) -> list[FxOptionPosition]:
    """A risk reversal, a strangle, a long ATM straddle and a short wing."""
    if str(pair) == "USDJPY":
        return [
            FxOptionPosition("3M 25d RR (buy C)", "3M", "call", "25dC", 20e6),
            FxOptionPosition("3M 25d RR (sell P)", "3M", "put", "25dP", -20e6),
            FxOptionPosition("1Y ATM straddle C", "1Y", "call", "ATM", 10e6),
            FxOptionPosition("1Y ATM straddle P", "1Y", "put", "ATM", 10e6),
            FxOptionPosition("6M 150 put", "6M", "put", 150.0, -25e6),
        ]
    return [
        FxOptionPosition("1M 25d RR (buy P)", "1M", "put", "25dP", 25e6),
        FxOptionPosition("1M 25d RR (sell C)", "1M", "call", "25dC", -25e6),
        FxOptionPosition("3M 10d strangle P", "3M", "put", "10dP", 10e6),
        FxOptionPosition("3M 10d strangle C", "3M", "call", "10dC", 10e6),
        FxOptionPosition("1Y ATM straddle C", "1Y", "call", "ATM", 10e6),
        FxOptionPosition("1Y ATM straddle P", "1Y", "put", "ATM", 10e6),
        FxOptionPosition("6M 1.25 call", "6M", "call", 1.25, -50e6),
    ]


def main(argv: Sequence[str]) -> int:
    trade = argv[1] if len(argv) > 1 else dt.date.today().isoformat()
    for pair in ("EURUSD", "USDJPY"):
        view = FxView(sample_fx_market(pair, trade), sample_fx_quotes(pair))
        view.print_forwards()
        print()
        view.print_smiles()
        print()
        view.print_book(sample_fx_book(pair))
        print()
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
