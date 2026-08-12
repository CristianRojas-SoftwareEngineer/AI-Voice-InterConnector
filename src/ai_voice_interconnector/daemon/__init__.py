"""
ai-voice-interconnector daemon module.

Provides a persistent TTS server that keeps the model in memory
between invocations, eliminating model load overhead on each call.
"""

from .daemon import DaemonManager
from .ipc import DaemonIPCClient, DaemonIPCError, is_daemon_running
from .protocol import (
    SynthesizeRequest,
    HealthResponse,
    VoicesResponse,
    ProgressEvent,
    ResultEvent,
    ErrorEvent,
)
from .server import app

__all__ = [
    "DaemonManager",
    "DaemonIPCClient",
    "DaemonIPCError",
    "is_daemon_running",
    "SynthesizeRequest",
    "HealthResponse",
    "VoicesResponse",
    "ProgressEvent",
    "ResultEvent",
    "ErrorEvent",
    "app",
]
