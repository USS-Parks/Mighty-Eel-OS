"""MAI TGI backend adapter."""
from .adapter import TgiAdapter
from .client import TgiClient
from .config import TgiConfig

__all__ = ["TgiAdapter", "TgiClient", "TgiConfig"]
