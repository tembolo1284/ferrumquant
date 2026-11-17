"""``python -m fqview [GROUP] [TRADE_DATE] [MC_PATHS]`` prints sample views.

GROUP is ``bonds`` (Treasury blotter), ``rates`` (SOFR curve, swap and
swaption book, vol cube and caps, Treasury futures basis), ``fx`` (forwards,
smiles, option book), ``commod`` (commodities book; MC_PATHS applies), or
``all`` (default: everything in that order)."""

import sys

from . import bonds, commod, curves, futures, fx, swaps, vol

GROUPS = {
    "bonds": [bonds],
    "treasury": [bonds],
    "rates": [curves, swaps, vol, futures],
    "curve": [curves],
    "swaps": [swaps],
    "vol": [vol],
    "futures": [futures],
    "fx": [fx],
    "currencies": [fx],
    "commod": [commod],
    "commodities": [commod],
    "energy": [commod],
}

args = sys.argv[1:]
group = "all"
if args and args[0].lower() in GROUPS or (args and args[0].lower() == "all"):
    group, args = args[0].lower(), args[1:]
modules = [curves, swaps, vol, futures, fx, bonds, commod] if group == "all" else GROUPS[group]
for mod in modules:
    rc = mod.main([sys.argv[0], *args])
    if rc:
        sys.exit(rc)
    print()
