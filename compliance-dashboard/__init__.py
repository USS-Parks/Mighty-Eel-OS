"""MAI Compliance Dashboard.

The exception to the "no L6 UI" rule: this small FastAPI app gives
compliance officers, regulators, and acquirers a visual way to verify
the platform's compliance posture. It is a thin shell over the
:mod:`mai` Python SDK — every piece of data shown is fetched live
from the mai-api server's ``/v1/compliance/*`` and ``/v1/trust/*``
endpoints.
"""

__version__ = "0.1.0"
