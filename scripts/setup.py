#!/usr/bin/env python3
"""Idempotent per-user setup shared by local and marketplace installations."""
from __future__ import annotations

import argparse
import os
import subprocess
import sys
import tempfile
from pathlib import Path


def systemd_quote(value: str, *, command: bool = False) -> str:
    if any(char in value for char in "\n\r\0"):
        raise ValueError("paths cannot contain newlines or NUL")
    value = value.replace("\\", "\\\\").replace('"', '\\"').replace("%", "%%")
    # The ':' ExecStart prefix disables environment expansion, including literal dollars.
    return (":" if command else "") + f'"{value}"'


def write_changed(path: Path, content: str) -> bool:
    if path.is_file() and path.read_text() == content:
        return False
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = None
    try:
        with tempfile.NamedTemporaryFile("w", dir=path.parent, delete=False) as handle:
            temporary = Path(handle.name)
            handle.write(content)
        temporary.replace(path)
    finally:
        if temporary is not None:
            temporary.unlink(missing_ok=True)
    return True


def render_units(root: Path, config: Path, state: Path) -> dict[str, str]:
    replacements = {
        "@ENV_PROJECT_ROOT@": systemd_quote(f"HERDR_STT_PROJECT_ROOT={root}"),
        "@ENV_CONFIG_PATH@": systemd_quote(f"HERDR_STT_CONFIG={config}"),
        "@ENV_STATE_DIR@": systemd_quote(f"HERDR_PLUGIN_STATE_DIR={state}"),
        "@STT_BINARY@": systemd_quote(str(root / "bin/herdr-stt"), command=True),
        "@DAEMON_BINARY@": systemd_quote(str(root / "bin/herdr-sttd"), command=True),
    }
    units = {}
    for name in ("herdr-stt.service", "herdr-stt-corrector.service"):
        content = (root / "systemd" / f"{name}.in").read_text()
        for marker, value in replacements.items():
            content = content.replace(marker, value)
        units[name] = content
    return units


def setup(root: Path, config: Path, state: Path, unit_dir: Path, *, activate: bool) -> None:
    config.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    state.mkdir(parents=True, exist_ok=True, mode=0o700)
    legacy = Path(os.environ.get("XDG_CONFIG_HOME", Path.home() / ".config")) / "herdr-stt/config.toml"
    if not config.exists():
        source = legacy if legacy.is_file() else root / "config.example.toml"
        content = source.read_text()
        # Exclusive creation preserves settings if two Herdr sessions initialize together.
        try:
            descriptor = os.open(config, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        except FileExistsError:
            pass
        else:
            with os.fdopen(descriptor, "w") as handle:
                handle.write(content)
    changed = False
    for name, content in render_units(root, config, state).items():
        changed = write_changed(unit_dir / name, content) or changed
    if activate and changed:
        subprocess.run(["systemctl", "--user", "daemon-reload"], check=True)
        # Refresh an existing installation, but do not load models on first setup.
        subprocess.run(
            ["systemctl", "--user", "try-restart", "herdr-stt.service", "herdr-stt-corrector.service"],
            check=True,
        )


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, required=True)
    parser.add_argument("--config", type=Path, required=True)
    parser.add_argument("--state", type=Path, required=True)
    parser.add_argument("--unit-dir", type=Path)
    parser.add_argument("--no-systemctl", action="store_true", help="render only, for packaging tests")
    parser.add_argument("--keybinding", action="store_true")
    args = parser.parse_args()
    root = args.root.resolve()
    config_home = Path(os.environ.get("XDG_CONFIG_HOME", Path.home() / ".config"))
    setup(
        root, args.config.resolve(), args.state.resolve(),
        args.unit_dir or config_home / "systemd/user", activate=not args.no_systemctl,
    )
    if args.keybinding:
        subprocess.run([sys.executable, str(root / "scripts/update_herdr_config.py")], check=True)
        subprocess.run(
            [os.environ.get("HERDR_BIN_PATH", "herdr"), "server", "reload-config"], check=True,
        )


if __name__ == "__main__":
    main()
