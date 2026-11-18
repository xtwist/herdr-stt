from __future__ import annotations

import importlib.util
import shutil
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("plugin_setup", ROOT / "scripts/setup.py")
setup = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(setup)


def test_setup_preserves_configuration_and_quotes_relocated_paths(tmp_path, monkeypatch):
    monkeypatch.setenv("XDG_CONFIG_HOME", str(tmp_path / "xdg"))
    root = tmp_path / 'plugin with spaces % and $'
    root.mkdir()
    shutil.copytree(ROOT / "systemd", root / "systemd")
    shutil.copy(ROOT / "config.example.toml", root)
    config = tmp_path / "settings/config.toml"
    state = tmp_path / "state"
    units = tmp_path / "units"
    setup.setup(root, config, state, units, activate=False)
    tomllib.loads(config.read_text())
    assert config.stat().st_mode & 0o777 == 0o600
    service = (units / "herdr-stt.service").read_text()
    assert 'ExecStart=:"' in service
    assert '%% and $' in service
    assert 'Environment="HERDR_STT_CONFIG=' in service
    assert "@PROJECT_ROOT@" not in service
    tray = (units / "herdr-stt-tray.service").read_text()
    assert 'bin/herdr-stt" tray' in tray
    assert "WantedBy=graphical-session.target" in tray
    config.write_text("[context]\ntext = 'Keep my settings'\n")
    old_mtime = (units / "herdr-stt.service").stat().st_mtime_ns
    setup.setup(root, config, state, units, activate=False)
    assert "Keep my settings" in config.read_text()
    assert (units / "herdr-stt.service").stat().st_mtime_ns == old_mtime


def test_manifest_versions_and_commands_are_publishable():
    manifest = tomllib.loads((ROOT / "herdr-plugin.toml").read_text())
    cargo = tomllib.loads((ROOT / "Cargo.toml").read_text())
    python = tomllib.loads((ROOT / "python/pyproject.toml").read_text())
    assert manifest["version"] == cargo["package"]["version"] == python["project"]["version"]
    assert manifest["platforms"] == ["linux"]
    assert manifest["id"] == "herdr.stt"
    assert manifest["startup"][0]["command"] == ["bin/herdr-stt", "setup"]
    actions = manifest["actions"]
    assert len({action["id"] for action in actions}) == len(actions)
    assert {"toggle", "cancel", "status", "setup", "reload"} <= {a["id"] for a in actions}
