#!/usr/bin/env python3
"""Build a reproducible source release using an explicit public-file allowlist."""
from __future__ import annotations

import gzip
import hashlib
import io
import tarfile
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
FILES = [
    "Cargo.toml", "Cargo.lock", "LICENSE", "README.md", "CHANGELOG.md", ".gitignore",
    "config.example.toml", "herdr-plugin.toml", "python/pyproject.toml", "python/uv.lock",
]
DIRECTORIES = ["src", "tests", "scripts", "systemd", "docs", ".github", "python/src", "python/tests"]


def main() -> None:
    manifest = tomllib.loads((ROOT / "herdr-plugin.toml").read_text())
    version = manifest["version"]
    assert version == tomllib.loads((ROOT / "Cargo.toml").read_text())["package"]["version"]
    assert version == tomllib.loads((ROOT / "python/pyproject.toml").read_text())["project"]["version"]
    paths = [ROOT / name for name in FILES]
    for directory in DIRECTORIES:
        paths.extend(path for path in (ROOT / directory).rglob("*") if path.is_file()
                     and "__pycache__" not in path.parts and path.suffix != ".pyc")
    prefix = f"herdr-stt-{version}"
    output = ROOT / "dist" / f"{prefix}.tar.gz"
    output.parent.mkdir(exist_ok=True)
    with (
        output.open("wb") as handle,
        gzip.GzipFile(fileobj=handle, filename="", mode="wb", mtime=0) as compressed,
        tarfile.open(fileobj=compressed, mode="w") as archive,
    ):
        for path in sorted(set(paths)):
            if path.is_symlink():
                raise ValueError(f"Refusing to package a symlink: {path}")
            data = path.read_bytes()
            info = tarfile.TarInfo(f"{prefix}/{path.relative_to(ROOT).as_posix()}")
            info.size = len(data)
            info.mode = 0o755 if path.stat().st_mode & 0o111 else 0o644
            archive.addfile(info, io.BytesIO(data))
    digest = hashlib.sha256(output.read_bytes()).hexdigest()
    output.with_suffix(output.suffix + ".sha256").write_text(f"{digest}  {output.name}\n")
    print(output)


if __name__ == "__main__":
    main()
