"""fqview -- a trader-style view onto the ferrumquant library.

Pure Python on top of the ``ferrumquant`` extension module (crates/fq-py).
Nothing is priced here; this package only arranges inputs and formats
outputs the way a desk would look at them.

    python -m fqview            # every sample view
    python -m fqview bonds      # Treasury blotter
    python -m fqview rates      # SOFR curve, swap/swaption book, vol cube, futures basis
    python -m fqview fx         # FX forwards, smiles and option book
    python -m fqview commod     # commodities book
"""

from .bonds import TreasuryBlotter, TreasuryPosition, sample_book
from .curves import CurveView, sample_quotes
from .swaps import RatesBlotter, SwapPosition, SwaptionPosition, sample_rates_book
from .futures import BasisScreen, Deliverable, sample_basket
from .vol import CapPosition, VolView, sample_caps, sample_cube_quotes
from .fx import FxOptionPosition, FxView, sample_fx_book, sample_fx_market, sample_fx_quotes
from .commod import (
    CommodBook,
    CommodMarket,
    CommodPosition,
    ag_option,
    ag_spread,
    apo,
    average_swap,
    block_swap,
    crack_option,
    crush,
    gas_basis_swap,
    gas_swap,
    heat_rate_option,
    lme_forward,
    lme_option,
    precious_forward,
    precious_vanilla,
    sample_commod_market,
    sample_commod_positions,
    storage,
    swing,
    tolling,
    transport,
)

__all__ = [
    # bonds
    "TreasuryBlotter",
    "TreasuryPosition",
    "sample_book",
    # rates
    "CurveView",
    "sample_quotes",
    "RatesBlotter",
    "SwapPosition",
    "SwaptionPosition",
    "sample_rates_book",
    "BasisScreen",
    "Deliverable",
    "sample_basket",
    "VolView",
    "CapPosition",
    "sample_cube_quotes",
    "sample_caps",
    # fx
    "FxView",
    "FxOptionPosition",
    "sample_fx_market",
    "sample_fx_quotes",
    "sample_fx_book",
    # commodities
    "CommodBook",
    "CommodMarket",
    "CommodPosition",
    "sample_commod_market",
    "sample_commod_positions",
    "gas_swap",
    "gas_basis_swap",
    "transport",
    "storage",
    "swing",
    "block_swap",
    "heat_rate_option",
    "tolling",
    "average_swap",
    "apo",
    "crack_option",
    "precious_forward",
    "precious_vanilla",
    "lme_forward",
    "lme_option",
    "ag_option",
    "crush",
    "ag_spread",
]
