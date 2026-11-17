"""``python -m fqview [TRADE_DATE]`` prints the sample Treasury blotter;
``python -m fqview commod [TRADE_DATE] [MC_PATHS]`` prints the sample
commodities book."""

import sys

from . import bonds, commod

args = sys.argv[1:]
if args and args[0] in ("commod", "commodities", "energy"):
    sys.exit(commod.main([sys.argv[0], *args[1:]]))
if args and args[0] in ("bonds", "treasury"):
    args = args[1:]
sys.exit(bonds.main([sys.argv[0], *args]))
