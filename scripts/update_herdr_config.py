#!/usr/bin/env python3
from __future__ import annotations

import argparse
import os
import shutil
import tempfile
import time
import tomllib
from pathlib import Path

MARKER_START = "# herdr-stt:start"
MARKER_END = "# herdr-stt:end"
BLOCK = f"""

{MARKER_START}
[[keys.command]]
key = "ctrl+alt+v"
type = "plugin_action"
command = "herdr.stt.toggle"
description = "Voice input"
{MARKER_END}
"""


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "path",
        nargs="?",
        type=Path,
        default=Path(os.environ.get("XDG_CONFIG_HOME", Path.home() / ".config"))
        / "herdr/config.toml",
    )
    args = parser.parse_args()
    path: Path = args.path.expanduser()
    text = path.read_text() if path.exists() else ""
    parsed = tomllib.loads(text) if text.strip() else {}
    commands = parsed.get("keys", {}).get("command", [])
    for command in commands:
        if command.get("key") == "ctrl+alt+v" and command.get("command") != "herdr.stt.toggle":
            raise SystemExit(
                "ctrl+alt+v is already bound to " + repr(command.get("command") or command.get("type"))
            )
        if command.get("command") == "herdr.stt.toggle":
            print(f"Herdr STT keybinding already present in {path}")
            return 0
    if MARKER_START in text or MARKER_END in text:
        raise SystemExit("found an incomplete herdr-stt marker block; fix it before installing")

    path.parent.mkdir(parents=True, exist_ok=True)
    if path.exists():
        backup = path.with_name(f"{path.name}.herdr-stt-backup-{int(time.time())}")
        shutil.copy2(path, backup)
        print(f"Backed up Herdr config to {backup}")
    updated = text.rstrip() + BLOCK + "\n"
    tomllib.loads(updated)
    with tempfile.NamedTemporaryFile("w", dir=path.parent, delete=False) as handle:
        handle.write(updated)
        temporary = Path(handle.name)
    temporary.replace(path)
    print(f"Added Ctrl+Alt+V binding to {path}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
