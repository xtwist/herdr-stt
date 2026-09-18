"""Long-lived faster-whisper worker used by herdr-sttd."""

from .worker import TranscriptionSettings, make_result

__all__ = ["TranscriptionSettings", "make_result"]
