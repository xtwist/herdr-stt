# Publication and removal

Herdr's marketplace is an automatic index of public GitHub repositories. See the official
[plugin reference](https://herdr.dev/docs/plugins/) and
[marketplace requirements](https://herdr.dev/docs/marketplace/).

## Before publication

```bash
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
uv run --project python --locked ruff check --config python/pyproject.toml python scripts
uv run --project python --locked pytest python/tests
HERDR_STT_ACCELERATOR=cpu ./scripts/build.sh
python3 scripts/package.py
```

Use the generated source archive as the public source tree. It contains the manifest, sources,
lockfiles, license, example configuration, documentation and CI; it excludes local environments,
models, compiled output, personal config, recordings and history. The adjacent SHA-256 file
verifies the archive. The same input files and executable bits produce the same archive bytes.

Publish those files to [xtwist/herdr-stt](https://github.com/xtwist/herdr-stt) with
`herdr-plugin.toml` at its root. Keep
`Cargo.lock` and `python/uv.lock` under version control. Add the GitHub topic `herdr-plugin`.
The marketplace discovers valid manifests on the default branch automatically (documented
refresh interval: 30 minutes). No separate Herdr upload format or catalog pull request is needed.

```bash
herdr plugin install xtwist/herdr-stt
herdr plugin action invoke herdr.stt.setup
```

`setup` initializes services and config without loading a model. Configure the keyboard binding
from README. The runtime startup hook repeats setup idempotently. The build hook never modifies
user config or starts services. A source archive is for manual/local installation; the Herdr CLI
installs from GitHub shorthand, not from an archive path.

## Upgrades

Reinstall the GitHub source to update a managed plugin. Locally linked plugins must first be
unlinked to switch to a GitHub-managed installation. Configuration and state live outside the
managed source checkout and are preserved. Version 0.3 reads 0.2 configuration and historical
transcripts. Runtime API version 3 requires restarting an older daemon; `herdr-stt reload` does
this. Setup updates service paths after relocation or reinstall, restarting active services only
when their unit content changed.

## Removal

Herdr does not remove plugin-owned systemd units or data. Stop and remove the units first:

```bash
systemctl --user disable --now herdr-stt.service herdr-stt-corrector.service
rm -f "${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user/herdr-stt.service"
rm -f "${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user/herdr-stt-corrector.service"
systemctl --user daemon-reload
herdr plugin uninstall herdr.stt
```

For a local checkout use `herdr plugin unlink herdr.stt` if you want to keep its registration-free
sources. Remove the local installer's `~/.local/bin/herdr-stt` symlink and the marked keybinding
block if present. Configuration, model files and transcript history remain available for recovery;
delete them separately only if wanted. `herdr-stt history clear` clears transcript records.
