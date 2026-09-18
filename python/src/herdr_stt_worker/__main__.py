from __future__ import annotations

import os
import sys

os.environ.setdefault("ORT_DISABLE_TELEMETRY", "1")


def _reexec_with_nvidia_libraries() -> None:
    """Expose pip-installed cuBLAS/cuDNN before CTranslate2 is imported."""
    if os.environ.get("HERDR_STT_NVIDIA_LIBS_READY") == "1":
        return
    try:
        import nvidia.cublas.lib
        import nvidia.cudnn.lib
    except ImportError:
        return

    paths = [
        next(iter(nvidia.cublas.lib.__path__)),
        next(iter(nvidia.cudnn.lib.__path__)),
    ]
    existing = os.environ.get("LD_LIBRARY_PATH")
    if existing:
        paths.append(existing)
    environment = os.environ.copy()
    environment["LD_LIBRARY_PATH"] = os.pathsep.join(paths)
    environment["HERDR_STT_NVIDIA_LIBS_READY"] = "1"
    os.execve(
        sys.executable, [sys.executable, "-m", "herdr_stt_worker", *sys.argv[1:]], environment
    )


_reexec_with_nvidia_libraries()

from .worker import main

if __name__ == "__main__":
    raise SystemExit(main())
