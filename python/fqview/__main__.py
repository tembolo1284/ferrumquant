"""``python -m fqview [TRADE_DATE]`` prints the sample Treasury blotter."""

import sys

from .bonds import main

sys.exit(main(sys.argv))
