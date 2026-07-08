"""Make the appliance validator importable from its sibling tests directory."""

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
