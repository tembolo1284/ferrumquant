"""fqview -- a trader-style view onto the ferrumquant library.

Pure Python on top of the ``ferrumquant`` extension module (crates/fq-py).
Nothing is priced here; this package only arranges inputs and formats
outputs the way a desk would look at them.

    python -m fqview            # Treasury blotter
    python -m fqview commod     # commodities book
"""

from .bonds import TreasuryBlotter, TreasuryPosition, sample_book
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
    "TreasuryBlotter",
    "TreasuryPosition",
    "sample_book",
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
