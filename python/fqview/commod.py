"""Commodities desk view: one energy/metals/ags book, one price, one sheet.

Usage::

    python -m fqview commod               # sample book, today's date
    python -m fqview commod 2026-11-09    # sample book, given trade date

or from Python::

    from fqview import CommodBook, sample_commod_market
    mkt = sample_commod_market("2026-11-09")
    book = CommodBook(mkt, sample_commod_positions())
    book.print()
    rows = book.rows()          # list of dicts
    df = book.to_dataframe()    # if pandas is installed

The market object carries everything the sector crates need (discount
curve, exchange calendars, futures curves per hub, seasonal vols, a
Clewlow-Strickland model) so a position only has to say what it is. Each
position prices through one ``value`` method on the Rust side and comes
back as a row with ``pv``, an ``intrinsic`` where the product has one,
and a one-line ``detail`` a trader would want next to the number.

All numbers are from ferrumquant; nothing is priced in Python.
"""

from __future__ import annotations

import datetime as dt
import math
import sys
from dataclasses import dataclass, field
from typing import Callable, Iterable, Optional, Sequence, Union

import ferrumquant as fq

DateInput = Union[str, dt.date, fq.Date]
MonthInput = Union[str, tuple, fq.ContractMonth]


def _months(start: fq.ContractMonth, n: int) -> list[fq.ContractMonth]:
    return [start.add_months(i) for i in range(n)]


# ---------------------------------------------------------------------------
# Market
# ---------------------------------------------------------------------------


@dataclass
class CommodMarket:
    """Everything a commodity position needs to price itself.

    ``curves`` is keyed by hub/contract code (``"HH"``, ``"PJMW:PK"``,
    ``"CL"``, ``"ZC"``); ``vols`` likewise, each a ``SeasonalVol`` or a
    flat number. ``power`` holds assembled ``PowerCurve`` objects by hub.
    """

    trade_date: fq.Date
    discount: fq.DiscountCurve
    curves: dict[str, fq.FuturesCurve] = field(default_factory=dict)
    vols: dict[str, object] = field(default_factory=dict)
    power: dict[str, fq.PowerCurve] = field(default_factory=dict)
    basis: dict[str, fq.BasisCurve] = field(default_factory=dict)
    lme: dict[str, fq.LmeCurve] = field(default_factory=dict)
    precious: dict[str, fq.PreciousMarket] = field(default_factory=dict)
    model: fq.ClewlowStrickland = field(default_factory=lambda: fq.ClewlowStrickland(0.9, 2.5, 0.25, 0.3))
    nymex: fq.NymexCalendar = field(default_factory=fq.NymexCalendar)
    oil: fq.OilMarket = field(default_factory=fq.OilMarket)
    lme_cal: fq.LmeCalendar = field(default_factory=fq.LmeCalendar)
    ag_cal: fq.AgCalendar = field(default_factory=fq.AgCalendar)
    # Correlations by pair code, e.g. ("PJMW:PK", "HH"); default used otherwise.
    correlations: dict[tuple[str, str], float] = field(default_factory=dict)
    default_corr: float = 0.7
    mc_paths: int = 1000

    def curve(self, code: str) -> fq.FuturesCurve:
        try:
            return self.curves[code]
        except KeyError:
            raise KeyError(f"no futures curve for '{code}' in the market") from None

    def vol(self, code: str):
        return self.vols.get(code, 0.3)

    def corr(self, a: str, b: str) -> float:
        return self.correlations.get((a, b), self.correlations.get((b, a), self.default_corr))

    def shifted(self, code: str, bump: float) -> "CommodMarket":
        """Copy with one futures curve shifted in parallel (for simple deltas)."""
        curves = dict(self.curves)
        curves[code] = self.curves[code].shifted(bump)
        power = dict(self.power)
        for hub, pc in self.power.items():
            if code == f"{hub}:PK":
                power[hub] = fq.PowerCurve(hub, pc.peak.shifted(bump), pc.offpeak)
            elif code == f"{hub}:OP":
                power[hub] = fq.PowerCurve(hub, pc.peak, pc.offpeak.shifted(bump))
        out = CommodMarket(**{**self.__dict__, "curves": curves, "power": power})
        return out


# ---------------------------------------------------------------------------
# Positions
# ---------------------------------------------------------------------------


@dataclass(frozen=True)
class CommodPosition:
    """One line of the book.

    ``pricer`` takes the market and returns ``(pv, intrinsic_or_None,
    detail_str)``. The helper constructors below build the common ones;
    anything else can be wrapped the same way.
    """

    label: str
    sector: str
    product: str
    hub: str
    period: str
    quantity: float
    unit: str
    pricer: Callable[[CommodMarket], tuple[float, Optional[float], str]]
    delta_code: Optional[str] = None  # curve to bump for the delta column


def _period(start: fq.ContractMonth, end: fq.ContractMonth) -> str:
    return start.code if start == end else f"{start.code}-{end.code}"


def gas_swap(label: str, hub: str, start: MonthInput, end: MonthInput, daily_qty: float, fixed: float, pay_fixed: bool = True) -> CommodPosition:
    s, e = fq.ContractMonth.from_any(start), fq.ContractMonth.from_any(end)
    inst = fq.GasSwap(hub, s, e, daily_qty, fixed, pay_fixed)

    def price(m: CommodMarket):
        v = inst.value(m.curve(hub), m.discount, m.nymex)
        return v["pv"], None, f"par {v['par_fixed']:.4f}  fixed {fixed:.4f}  {'pay' if pay_fixed else 'rec'} fixed"

    return CommodPosition(label, "gas", "swap", hub, _period(s, e), daily_qty, "MMBtu/d", price, hub)


def gas_basis_swap(label: str, hub: str, start: MonthInput, end: MonthInput, daily_qty: float, fixed_basis: float, receive_basis: bool = True) -> CommodPosition:
    s, e = fq.ContractMonth.from_any(start), fq.ContractMonth.from_any(end)
    inst = fq.GasBasisSwap(hub, s, e, daily_qty, fixed_basis, receive_basis)

    def price(m: CommodMarket):
        v = inst.value(m.basis[hub], m.discount, m.nymex)
        return v["pv"], None, f"par basis {v['par_fixed']:+.4f}  fixed {fixed_basis:+.4f}"

    # Basis curves are inputs in their own right, so no outright delta here.
    return CommodPosition(label, "gas", "basis", hub, _period(s, e), daily_qty, "MMBtu/d", price)


def transport(label: str, receipt: str, delivery: str, start: MonthInput, end: MonthInput, mdq: float, fuel: float, commodity: float, demand: float) -> CommodPosition:
    s, e = fq.ContractMonth.from_any(start), fq.ContractMonth.from_any(end)
    inst = fq.TransportCapacity(receipt, delivery, s, e, mdq, fuel, commodity, demand)

    def price(m: CommodMarket):
        v = inst.value(m.curve(receipt), m.curve(delivery), m.vol(receipt), m.vol(delivery), m.corr(receipt, delivery), m.discount, m.nymex)
        return v["net_pv"], v["intrinsic_pv"] - v["demand_pv"], f"extrinsic {v['extrinsic_pv']:,.0f}  demand {v['demand_pv']:,.0f}"

    return CommodPosition(label, "gas", "transport", f"{receipt}>{delivery}", _period(s, e), mdq, "MMBtu/d", price, delivery)


def storage(label: str, hub: str, start: MonthInput, end: MonthInput, capacity: float, max_inj: float, max_wd: float, inj_cost: float, wd_cost: float, fuel: float = 0.0, mc: bool = True) -> CommodPosition:
    s, e = fq.ContractMonth.from_any(start), fq.ContractMonth.from_any(end)
    inst = fq.StorageFacility(hub, s, e, capacity, max_inj, max_wd, inj_cost, wd_cost, fuel)

    def price(m: CommodMarket):
        if mc:
            v = inst.rolling_intrinsic(m.curve(hub), m.model, m.discount, m.nymex, paths=m.mc_paths)
            return v["rolling_pv"], v["intrinsic_pv"], f"extrinsic {v['extrinsic_pv']:,.0f}  se {v['rolling_std_err']:,.0f}  {m.mc_paths} paths"
        v = inst.intrinsic(m.curve(hub), m.discount, m.nymex)
        inj = sum(x["change"] for x in v["schedule"] if x["change"] > 0)
        return v["intrinsic_pv"], v["intrinsic_pv"], f"intrinsic only  cycles {inj / capacity:.2f}"

    return CommodPosition(label, "gas", "storage", hub, _period(s, e), capacity, "MMBtu", price, hub)


def swing(label: str, hub: str, start: MonthInput, end: MonthInput, strike: float, min_d: float, max_d: float, min_t: float, max_t: float) -> CommodPosition:
    s, e = fq.ContractMonth.from_any(start), fq.ContractMonth.from_any(end)
    inst = fq.SwingOption(hub, s, e, strike, min_d, max_d, min_t, max_t)

    def price(m: CommodMarket):
        v = inst.value_with_model(m.curve(hub), m.vol(hub), m.model, m.discount, m.nymex, paths=m.mc_paths)
        return v["rolling_pv"], v["intrinsic_pv"], f"upper bound {v['upper_bound_pv']:,.0f}  se {v['rolling_std_err']:,.0f}"

    return CommodPosition(label, "gas", "swing", hub, _period(s, e), max_t, "MMBtu", price, hub)


def block_swap(label: str, hub: str, block: str, start: MonthInput, end: MonthInput, mw: float, fixed: float, pay_fixed: bool = True) -> CommodPosition:
    s, e = fq.ContractMonth.from_any(start), fq.ContractMonth.from_any(end)
    inst = fq.BlockSwap(hub, block, s, e, mw, fixed, pay_fixed)

    def price(m: CommodMarket):
        v = inst.value(m.power[hub], m.discount)
        return v["pv"], None, f"par {v['par_fixed']:.2f}  fixed {fixed:.2f}  {v['total_mwh']:,.0f} MWh"

    return CommodPosition(label, "power", f"{block} swap", hub, _period(s, e), mw, "MW", price, f"{hub}:{block.upper()}")


def heat_rate_option(label: str, hub: str, block: str, gas_hub: str, start: MonthInput, end: MonthInput, mw: float, hr: float, vom: float) -> CommodPosition:
    s, e = fq.ContractMonth.from_any(start), fq.ContractMonth.from_any(end)
    inst = fq.HeatRateOption("call", hub, block, s, e, mw, hr, vom)
    pk = f"{hub}:{block.upper()}"

    def price(m: CommodMarket):
        v = inst.value(m.power[hub], m.curve(gas_hub), m.vol(pk), m.vol(gas_hub), m.corr(pk, gas_hub), m.discount)
        ihr = sum(x["implied_heat_rate"] for x in v["months"]) / len(v["months"])
        return v["option_pv"], v["intrinsic_pv"], f"extrinsic {v['extrinsic_pv']:,.0f}  mkt HR {ihr:.2f} vs {hr:.2f}"

    return CommodPosition(label, "power", "spark", hub, _period(s, e), mw, "MW", price, pk)


def tolling(label: str, hub: str, block: str, gas_hub: str, start: MonthInput, end: MonthInput, mw: float, hr: float, vom: float, capacity_kw_month: float) -> CommodPosition:
    s, e = fq.ContractMonth.from_any(start), fq.ContractMonth.from_any(end)
    inst = fq.TollingAgreement(hub, block, s, e, mw, hr, vom, capacity_kw_month)
    pk = f"{hub}:{block.upper()}"

    def price(m: CommodMarket):
        v = inst.value(m.power[hub], m.curve(gas_hub), m.vol(pk), m.vol(gas_hub), m.corr(pk, gas_hub), m.discount)
        return v["net_pv"], v["spark"]["intrinsic_pv"] - v["capacity_pv"], f"spark {v['spark']['option_pv']:,.0f}  capacity {v['capacity_pv']:,.0f}"

    return CommodPosition(label, "power", "tolling", hub, _period(s, e), mw, "MW", price, pk)


def average_swap(label: str, contract: str, start: MonthInput, end: MonthInput, qty: float, fixed: float, pay_fixed: bool = True) -> CommodPosition:
    s, e = fq.ContractMonth.from_any(start), fq.ContractMonth.from_any(end)
    inst = fq.AverageSwap(contract, s, e, qty, fixed, pay_fixed)

    def price(m: CommodMarket):
        v = inst.value(m.curve(contract), m.discount, m.oil)
        return v["pv"], None, f"par {v['par_fixed']:.3f}  fixed {fixed:.3f}  {'pay' if pay_fixed else 'rec'} fixed"

    return CommodPosition(label, "oil", "CMA swap", contract, _period(s, e), qty, fq.oil_contracts()[contract]["unit"] + "/mo", price, contract)


def apo(label: str, kind: str, contract: str, month: MonthInput, strike: float, qty: float) -> CommodPosition:
    mo = fq.ContractMonth.from_any(month)
    inst = fq.AveragePriceOption(kind, contract, mo, strike, qty)

    def price(m: CommodMarket):
        v = inst.value(m.curve(contract), m.vol(contract), m.discount, m.oil)
        intrinsic = max(0.0, (v["forward_average"] - strike) * (1 if kind == "call" else -1)) * qty * v["discount"]
        return v["pv"], intrinsic, f"avg fwd {v['forward_average']:.3f}  eff vol {v['effective_vol']:.3f}  {v['fixings']} fixings ({v['observed']} seen)"

    return CommodPosition(label, "oil", f"APO {kind}", contract, mo.code, qty, fq.oil_contracts()[contract]["unit"], price, contract)


def crack_option(label: str, kind: str, product: str, crude: str, month: MonthInput, strike: float, barrels: float) -> CommodPosition:
    mo = fq.ContractMonth.from_any(month)
    inst = fq.CrackOption(kind, product, crude, mo, strike, barrels)

    def price(m: CommodMarket):
        v = inst.value(m.curve(product), m.curve(crude), m.vol(product), m.vol(crude), m.corr(product, crude), m.discount, m.oil)
        intrinsic = max(0.0, (v["forward_crack"] - strike) * (1 if kind == "call" else -1)) * barrels * v["discount"]
        return v["pv"], intrinsic, f"fwd crack {v['forward_crack']:.2f}  K {strike:.2f}"

    return CommodPosition(label, "oil", f"crack {kind}", f"{product}/{crude}", mo.code, barrels, "bbl", price, product)


def precious_forward(label: str, metal: str, ounces: float, strike: float, value_date: DateInput) -> CommodPosition:
    inst = fq.PreciousForward(metal, ounces, strike, value_date)
    vd = fq.Date.from_any(value_date)

    def price(m: CommodMarket):
        v = inst.value(m.precious[metal])
        return v["pv"], None, f"fwd {v['forward']:.2f}  strike {strike:.2f}"

    return CommodPosition(label, "metals", "forward", metal, str(vd), ounces, "oz", price)


def precious_vanilla(label: str, kind: str, metal: str, ounces: float, strike: float, expiry: DateInput, vol: float) -> CommodPosition:
    inst = fq.PreciousVanilla(kind, metal, ounces, strike, expiry)
    ex = fq.Date.from_any(expiry)

    def price(m: CommodMarket):
        v = inst.value(m.precious[metal], vol)
        intrinsic = max(0.0, (v["forward"] - strike) * (1 if kind == "call" else -1)) * ounces * v["discount"]
        return v["pv"], intrinsic, f"fwd {v['forward']:.2f}  vol {vol:.1%}  spot delta {v['spot_delta']:,.0f} oz"

    return CommodPosition(label, "metals", f"vanilla {kind}", metal, str(ex), ounces, "oz", price)


def lme_forward(label: str, metal: str, tonnes: float, price_: float, prompt: DateInput) -> CommodPosition:
    inst = fq.LmeForward(metal, tonnes, price_, prompt)
    pr = fq.Date.from_any(prompt)

    def price(m: CommodMarket):
        v = inst.value(m.lme[metal], m.discount, m.lme_cal)
        return v["pv"], None, f"fwd {v['forward']:.2f}  traded {price_:.2f}"

    return CommodPosition(label, "metals", "LME fwd", metal, str(pr), tonnes, "t", price)


def lme_option(label: str, kind: str, metal: str, tonnes: float, strike: float, year: int, month: int, vol: float) -> CommodPosition:
    inst = fq.LmeOption(kind, metal, tonnes, strike, year, month)

    def price(m: CommodMarket):
        v = inst.value(m.lme[metal], vol, m.discount, m.lme_cal)
        intrinsic = max(0.0, (v["forward"] - strike) * (1 if kind == "call" else -1)) * tonnes * v["discount"]
        return v["pv"], intrinsic, f"fwd {v['forward']:.2f}  declares {v['expiry']}  prompt {v['prompt']}"

    return CommodPosition(label, "metals", f"LME {kind}", metal, f"{year}-{month:02d}", tonnes, "t", price)


def ag_option(label: str, kind: str, contract: str, month: MonthInput, strike: float, lots: float) -> CommodPosition:
    mo = fq.ContractMonth.from_any(month)
    inst = fq.AgOption(kind, contract, mo, strike, lots)
    lot = fq.ag_contracts()[contract]["lot_size"]
    scale = fq.ag_lot_value(contract, 1.0)

    def price(m: CommodMarket):
        v = inst.value(m.curve(contract), m.vol(contract), m.discount, m.ag_cal)
        intrinsic = max(0.0, (v["forward"] - strike) * (1 if kind == "call" else -1)) * scale * lots * v["discount"]
        return v["pv"], intrinsic, f"fwd {v['forward']:.2f}  vol {v['vol']:.1%}  delta ${v['delta_dollars']:,.0f}/pt  exp {v['expiry']}"

    return CommodPosition(label, "ags", f"option {kind}", contract, mo.code, lots * lot, fq.ag_contracts()[contract]["unit"], price, contract)


def crush(label: str, month: MonthInput, bushels: float) -> CommodPosition:
    mo = fq.ContractMonth.from_any(month)
    inst = fq.Crush(mo, bushels)

    def price(m: CommodMarket):
        v = inst.value(m.curve("ZS"), m.curve("ZM"), m.curve("ZL"))
        return v["gross_margin"], v["gross_margin"], f"crush {v['crush_margin']:.3f} $/bu  oil share {v['oil_share']:.1%}"

    return CommodPosition(label, "ags", "crush", "ZS/ZM/ZL", mo.code, bushels, "bu", price, "ZS")


def ag_spread(label: str, kind: str, contract: str, near: MonthInput, far: MonthInput, strike: float, lots: float) -> CommodPosition:
    n, f = fq.ContractMonth.from_any(near), fq.ContractMonth.from_any(far)
    inst = fq.AgCalendarSpread(kind, contract, n, f, strike, lots)

    def price(m: CommodMarket):
        v = inst.value(m.curve(contract), m.vol(contract), m.corr(f"{contract}:{n.code}", f"{contract}:{f.code}"), m.discount, m.ag_cal)
        return v["pv"], None, f"spread {v['forward_spread']:+.2f}  K {strike:.2f}"

    return CommodPosition(label, "ags", f"CSO {kind}", contract, f"{n.code}/{f.code}", lots, "lots", price, contract)


# ---------------------------------------------------------------------------
# Book
# ---------------------------------------------------------------------------


class CommodBook:
    """Prices every position against one market and lays it out by sector."""

    COLUMNS = (
        ("label", "Position", 26, "{}"),
        ("sector", "Sector", 7, "{}"),
        ("product", "Product", 12, "{}"),
        ("hub", "Hub", 11, "{}"),
        ("period", "Period", 11, "{}"),
        ("quantity", "Qty", 12, "{:>12,.0f}"),
        ("unit", "Unit", 8, "{}"),
        ("pv", "PV", 15, "{:>15,.0f}"),
        ("intrinsic", "Intrinsic", 13, "{}"),
        ("delta", "Delta/0.01", 11, "{}"),
    )

    def __init__(self, market: CommodMarket, positions: Iterable[CommodPosition], deltas: bool = True):
        self.market = market
        self.positions: Sequence[CommodPosition] = tuple(positions)
        self.deltas = deltas

    def rows(self) -> list[dict]:
        out = []
        bumped: dict[str, CommodMarket] = {}
        for p in self.positions:
            pv, intrinsic, detail = p.pricer(self.market)
            delta = None
            if self.deltas and p.delta_code and p.delta_code in self.market.curves:
                if p.delta_code not in bumped:
                    bumped[p.delta_code] = self.market.shifted(p.delta_code, 0.01)
                delta = p.pricer(bumped[p.delta_code])[0] - pv
            out.append(
                {
                    "label": p.label,
                    "sector": p.sector,
                    "product": p.product,
                    "hub": p.hub,
                    "period": p.period,
                    "quantity": p.quantity,
                    "unit": p.unit,
                    "pv": pv,
                    "intrinsic": intrinsic,
                    "extrinsic": None if intrinsic is None else pv - intrinsic,
                    "delta": delta,
                    "detail": detail,
                }
            )
        return out

    def totals(self, rows: Optional[list[dict]] = None) -> dict:
        rows = self.rows() if rows is None else rows
        by_sector: dict[str, float] = {}
        for r in rows:
            by_sector[r["sector"]] = by_sector.get(r["sector"], 0.0) + r["pv"]
        return {"pv": sum(r["pv"] for r in rows), "by_sector": by_sector, "lines": len(rows)}

    @staticmethod
    def _cell(key: str, fmt: str, value) -> str:
        if value is None:
            return "-"
        if key in ("intrinsic", "delta"):
            return f"{value:,.0f}"
        return fmt.format(value)

    def format(self, details: bool = True) -> str:
        rows = self.rows()
        t = self.totals(rows)
        left = {"label", "sector", "product", "hub", "period", "unit"}
        head = " ".join(f"{title:<{w}}" if key in left else f"{title:>{w}}" for key, title, w, _ in self.COLUMNS)
        lines = [f"Commodities book  trade {self.market.trade_date}  ({len(rows)} lines, {self.market.mc_paths} MC paths)", head, "-" * len(head)]
        sector = None
        for r in rows:
            if r["sector"] != sector:
                sector = r["sector"]
                if r is not rows[0]:
                    lines.append("")
            cells = []
            for key, _, w, fmt in self.COLUMNS:
                s = self._cell(key, fmt, r[key])
                cells.append(f"{s:<{w}}" if key in left else f"{s:>{w}}")
            lines.append(" ".join(cells))
            if details:
                lines.append(f"{'':<27}{r['detail']}")
        lines.append("-" * len(head))
        for s, v in t["by_sector"].items():
            lines.append(f"{s.upper():<26} {'':>7} {'':>12} {'':>11} {'':>11} {'':>12} {'':>8} {v:>15,.0f}")
        lines.append(f"{'TOTAL':<26} {'':>7} {'':>12} {'':>11} {'':>11} {'':>12} {'':>8} {t['pv']:>15,.0f}")
        return "\n".join(lines)

    def print(self, details: bool = True) -> None:
        print(self.format(details))

    def to_dataframe(self):
        import pandas as pd  # optional dependency

        return pd.DataFrame(self.rows()).set_index("label")


# ---------------------------------------------------------------------------
# Sample market and book
# ---------------------------------------------------------------------------


def sample_commod_market(trade_date: DateInput, mc_paths: int = 1000) -> CommodMarket:
    """Made-up but shaped curves for every hub the sample book touches."""
    ref = fq.Date.from_any(trade_date)
    first = fq.ContractMonth.from_any(ref).next()
    months = _months(first, 48)
    disc = fq.DiscountCurve.flat(ref, 0.04, ref.add_years(12))

    def curve(code: str, hub: fq.Hub, f: Callable[[fq.ContractMonth, int], float]) -> fq.FuturesCurve:
        return fq.FuturesCurve(hub, ref, [(m, f(m, i)) for i, m in enumerate(months)])

    def winter(m: fq.ContractMonth) -> float:
        return 1.0 if m.month in (12, 1, 2) else (0.5 if m.month in (11, 3) else 0.0)

    def summer(m: fq.ContractMonth) -> float:
        return 1.0 if m.month in (7, 8) else (0.5 if m.month in (6, 9) else 0.0)

    curves = {
        "HH": curve("HH", fq.gas_hub("HH"), lambda m, i: 3.20 + 1.10 * winter(m) + 0.012 * i),
        "TETM3": curve("TETM3", fq.gas_hub("TETM3"), lambda m, i: 3.20 + 1.10 * winter(m) + 0.012 * i + 0.60 + 3.5 * winter(m)),
        "WAHA": curve("WAHA", fq.gas_hub("WAHA"), lambda m, i: 3.20 + 1.10 * winter(m) + 0.012 * i - 1.40 + 0.02 * i),
        "PJMW:PK": curve("PJMW:PK", fq.power_hub("PJMW"), lambda m, i: 48 + 22 * winter(m) + 18 * summer(m) + 0.1 * i),
        "PJMW:OP": curve("PJMW:OP", fq.power_hub("PJMW"), lambda m, i: 30 + 9 * winter(m) + 3 * summer(m) + 0.05 * i),
        "ERN:PK": curve("ERN:PK", fq.power_hub("ERN"), lambda m, i: 42 + 8 * winter(m) + 45 * summer(m) + 0.1 * i),
        "ERN:OP": curve("ERN:OP", fq.power_hub("ERN"), lambda m, i: 26 + 4 * winter(m) + 10 * summer(m) + 0.05 * i),
        "CL": curve("CL", fq.oil_hub("CL"), lambda m, i: 68.0 - 0.25 * i),
        "BRN": curve("BRN", fq.oil_hub("BRN"), lambda m, i: 71.5 - 0.25 * i),
        "RB": curve("RB", fq.oil_hub("RB"), lambda m, i: 2.05 + 0.25 * summer(m) + 0.10 * (m.month in (4, 5)) - 0.004 * i),
        "HO": curve("HO", fq.oil_hub("HO"), lambda m, i: 2.30 + 0.12 * winter(m) - 0.004 * i),
        "ZC": curve("ZC", fq.ag_hub("ZC"), lambda m, i: 435.0 + 1.6 * i),
        "ZS": curve("ZS", fq.ag_hub("ZS"), lambda m, i: 1040.0 + 1.2 * i),
        "ZM": curve("ZM", fq.ag_hub("ZM"), lambda m, i: 305.0 + 0.3 * i),
        "ZL": curve("ZL", fq.ag_hub("ZL"), lambda m, i: 44.0 + 0.05 * i),
        "KC": curve("KC", fq.ag_hub("KC"), lambda m, i: 285.0 - 1.5 * i),
    }
    sv = lambda base, w, s=0.0: fq.SeasonalVol([(m, base + w * winter(m) + s * summer(m)) for m in months])  # noqa: E731
    vols = {
        "HH": sv(0.42, 0.25),
        "TETM3": sv(0.55, 0.45),
        "WAHA": sv(0.60, 0.20),
        "PJMW:PK": sv(0.45, 0.25, 0.30),
        "PJMW:OP": sv(0.35, 0.15, 0.10),
        "ERN:PK": sv(0.55, 0.20, 0.60),
        "ERN:OP": sv(0.40, 0.10, 0.25),
        "CL": sv(0.32, 0.0),
        "BRN": sv(0.30, 0.0),
        "RB": sv(0.36, 0.0, 0.08),
        "HO": sv(0.34, 0.06),
        "ZC": sv(0.24, 0.0, 0.10),
        "ZS": sv(0.20, 0.0, 0.08),
        "ZM": sv(0.26, 0.0, 0.08),
        "ZL": sv(0.28, 0.0),
        "KC": sv(0.38, 0.0),
    }
    power = {
        "PJMW": fq.PowerCurve("PJMW", curves["PJMW:PK"], curves["PJMW:OP"]),
        "ERN": fq.PowerCurve("ERN", curves["ERN:PK"], curves["ERN:OP"]),
    }
    basis = {
        "TETM3": fq.BasisCurve.from_outrights(curves["TETM3"], curves["HH"]),
        "WAHA": fq.BasisCurve.from_outrights(curves["WAHA"], curves["HH"]),
    }
    lme_cal = fq.LmeCalendar()
    lme_quotes = lambda code, spot, per_month: [(d, spot + per_month * (d.serial - ref.serial) / 30.0) for d, k in lme_cal.prompts(code, ref, 15) if k in ("cash", "3m", "monthly")]  # noqa: E731
    cu = fq.LmeCurve("CA", ref, lme_quotes("CA", 9400.0, 9.0), lme_cal)  # contango, about 1.1% a year
    al = fq.LmeCurve("AH", ref, lme_quotes("AH", 2600.0, 5.0), lme_cal)
    usd = fq.DiscountCurve.flat(ref, 0.045, ref.add_years(12))
    precious = {
        "XAU": fq.PreciousMarket("XAU", ref, 2650.0, usd, fq.flat_lease_curve(ref, 0.005)),
        "XAG": fq.PreciousMarket("XAG", ref, 31.5, usd, fq.flat_lease_curve(ref, 0.012)),
    }
    corr = {
        ("HH", "TETM3"): 0.90,
        ("HH", "WAHA"): 0.85,
        ("PJMW:PK", "HH"): 0.65,
        ("ERN:PK", "HH"): 0.45,
        ("RB", "CL"): 0.75,
        ("HO", "CL"): 0.80,
    }
    return CommodMarket(ref, disc, curves, vols, power, basis, {"CA": cu, "AH": al}, precious, correlations=corr, mc_paths=mc_paths)


def sample_commod_positions(trade_date: DateInput) -> list[CommodPosition]:
    """A cross-commodity book: gas optionality, power tolling, oil swaps and
    options, metals forwards and options, and the ag crush."""
    ref = fq.Date.from_any(trade_date)
    first = fq.ContractMonth.from_any(ref).next()
    y = ref.year + 1
    cal = fq.LmeCalendar()
    jun3w = cal.third_wednesday(y, 6)
    return [
        gas_swap("HH Q1 swap (pay fixed)", "HH", (y, 1), (y, 3), 20_000, 4.10),
        gas_basis_swap("TETCO M3 winter basis", "TETM3", (y, 1), (y, 2), 10_000, 3.00),
        transport("Waha>HH transport", "WAHA", "HH", (y, 1), (y, 12), 25_000, 0.015, 0.03, 0.25),
        storage("HH salt dome lease", "HH", first, first.add_months(11), 2_000_000, 40_000, 80_000, 0.015, 0.015, 0.01),
        swing("HH swing Jan-Mar", "HH", (y, 1), (y, 3), 4.00, 5_000, 20_000, 600_000, 1_200_000),
        block_swap("PJM West PK cal swap", "PJMW", "PK", (y, 1), (y, 12), 25, 58.0),
        heat_rate_option("PJM West 7.5HR strip", "PJMW", "PK", "HH", (y, 1), (y, 12), 50, 7.5, 2.5),
        tolling("ERCOT North peaker toll", "ERN", "PK", "HH", (y, 6), (y, 9), 100, 9.5, 3.0, 4.0),
        average_swap("Brent H1 CMA (rec fixed)", "BRN", (y, 1), (y, 6), 50_000, 70.0, pay_fixed=False),
        apo("WTI Mar APO call", "call", "CL", (y, 3), 70.0, 100_000),
        crack_option("RBOB/WTI Jun crack call", "call", "RB", "CL", (y, 6), 22.0, 200_000),
        precious_forward("Gold 1y forward", "XAU", 5_000, 2_740.0, ref.add_years(1)),
        precious_vanilla("Gold 1y 2800 call", "call", "XAU", 5_000, 2_800.0, ref.add_years(1), 0.16),
        lme_forward("Copper Jun 3W forward", "CA", 500, 9_480.0, jun3w),
        lme_option("Copper Jun 9600 call", "call", "CA", 500, 9_600.0, y, 6, 0.22),
        ag_option("Corn Jul 460 call", "call", "ZC", (y, 7), 460.0, 100),
        crush("Jul board crush", (y, 7), 2_000_000),
        ag_spread("Corn H/N CSO put", "put", "ZC", (y, 3), (y, 7), 0.0, 50),
    ]


def main(argv: Sequence[str]) -> int:
    trade = argv[1] if len(argv) > 1 else dt.date.today().isoformat()
    paths = int(argv[2]) if len(argv) > 2 else 1000
    mkt = sample_commod_market(trade, mc_paths=paths)
    CommodBook(mkt, sample_commod_positions(trade)).print()
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
