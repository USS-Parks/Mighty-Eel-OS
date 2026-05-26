"""Adapter runner startup and loading helpers."""

from __future__ import annotations

import contextlib
import importlib
import json
import logging
import sys
from typing import Any

from adapters.base import AdapterBase, get_adapter

logger = logging.getLogger("mai.adapters.runner")


def load_adapter(module_path: str, class_name: str) -> AdapterBase:
    try:
        module = importlib.import_module(module_path)
    except ImportError as e:
        logger.error(f"Failed to import adapter module '{module_path}': {e}")
        sys.exit(1)

    cls = getattr(module, class_name, None)
    if cls is None:
        logger.error(f"Class '{class_name}' not found in module '{module_path}'")
        sys.exit(1)
    if not issubclass(cls, AdapterBase):
        logger.error(f"'{class_name}' does not inherit from AdapterBase")
        sys.exit(1)
    return cls()


def load_registered_adapter(adapter_name: str) -> tuple[AdapterBase, str]:
    with contextlib.suppress(ImportError):
        importlib.import_module(f"adapters.{adapter_name}.adapter")
    cls = get_adapter(adapter_name)
    if cls is None:
        logger.error(
            f"Adapter '{adapter_name}' not found in registry and no "
            f"module_path/entry_class provided in startup config",
        )
        sys.exit(1)
    return cls(), getattr(cls, "_mai_adapter_version", "1.0.0")


def read_startup_config() -> dict[str, Any]:
    line = sys.stdin.readline()
    if not line:
        logger.error("No startup config received on stdin (EOF)")
        sys.exit(1)
    try:
        return json.loads(line.strip())
    except json.JSONDecodeError as e:
        logger.error(f"Invalid startup config JSON: {e}")
        sys.exit(1)
