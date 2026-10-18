"""fqview -- a trader-style view onto the ferrumquant library.

Pure Python on top of the ``ferrumquant`` extension module (crates/fq-py).
Nothing is priced here; this package only arranges inputs and formats
outputs the way a desk would look at them.
"""

from .bonds import TreasuryBlotter, TreasuryPosition, sample_book

__all__ = ["TreasuryBlotter", "TreasuryPosition", "sample_book"]
