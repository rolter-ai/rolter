"""black-box e2e harness for rolter.

drives a freshly booted docker-compose stack (postgres/redis/clickhouse/control/
gateway + fake-vLLM engines) through the real HTTP APIs. no in-process shortcuts.
"""

from .client import ApiError, ControlClient, GatewayClient
from .stack import Stack

__all__ = ["ApiError", "ControlClient", "GatewayClient", "Stack"]
