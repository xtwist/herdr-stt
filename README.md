# Herdr STT

Local speech-to-text for Herdr: a Rust daemon records the PipeWire default microphone, a
long-lived `faster-whisper` worker transcribes it, and a Herdr plugin routes the result to the pane
where recording started. An optional repo-aware Qwen corrector can repair uncertain ASR text
without being allowed to change voice-command routing.

## Install

Linux, Herdr 0.8.2+, PipeWire (`pw-record`), a systemd user session, Rust 1.88+,
and `uv` are required. `uv` provisions Python 3.12. CPU INT8 works without an NVIDIA GPU;
the build script selects CUDA dependencies when `nvidia-smi` detects a GPU.
Override detection with `HERDR_STT_ACCELERATOR=cpu` or `cuda`.

Install from GitHub:

```bash
herdr plugin install xtwist/herdr-stt
herdr plugin action invoke herdr.stt.setup
```

For a local checkout:

```bash
./scripts/install-local.sh
herdr-stt doctor
```

The standard plugin build installs dependencies and compiles the binaries. Setup creates a
private configuration and user services, preserving existing settings. New configurations use
Whisper `large-v3-turbo` by default. You can choose another model with `[model].name` in
`config.toml`. The selected Whisper model downloads on first dictation, and Qwen downloads
only when you run `corrector install`.
Setup also runs on Herdr startup and before dictation, so a freshly installed plugin works
without running the local installer. Services are started on demand.

For marketplace installs, add this binding to Herdr's `config.toml`, then run
`herdr server reload-config`. The local installer adds it after checking for conflicts.

```toml
[[keys.command]]
key = "ctrl+alt+v"
type = "plugin_action"
command = "herdr.stt.toggle"
description = "Voice input"
```

The CLI is also available at `<plugin-root>/bin/herdr-stt`; the local installer links it into
`~/.local/bin`. Use this path for the CLI examples below when installed from the marketplace.

## Use

Press `Ctrl+Alt+V` once to start recording and again to stop. Ordinary speech is inserted into the
original Herdr pane without Enter.

The Herdr STT icon stays visible on desktops with a StatusNotifierItem tray, including Ubuntu
with AppIndicators. Its icon changes as recording starts, stops and transcribes. Open the tray
menu for the latest result or error. If the duration limit stops the microphone, the icon shows
that the recording is saved and ready to transcribe with `Ctrl+Alt+V`. Herdr STT uses the tray
for feedback without popups or sounds. The tray runs independently of the speech daemon.

Strict voice commands:

- `отправь промпт <текст>` / `send prompt <text>` — submit a new prompt immediately.
- `запусти промпт` / `run prompt` — press Enter for the prompt already in the agent input.
- `режим планирования` / `включи режим планирования` / `plan mode` — enable Codex Plan mode.
- `планирование <текст>` — enable Plan mode, remove the prefix, and insert the remaining text
  without Enter.
- `переключись на воркспейс <имя>` / `switch to workspace <name>` — focus a workspace.
- `переключи workspace <имя>` / `переключи воркспейс <имя>` — shorter switch commands.
- `открой workspace <имя>` / `workspace <имя>` — shortest local switch commands.
- Workspace commands ignore letter case and surrounding punctuation; common STT `workspice` is
  corrected to `Workspace` before routing.
- `переключись на воркспейс номер 14` — resolve duplicate workspace labels by displayed number.

Useful commands:

```bash
herdr-stt status
herdr-stt cancel
herdr-stt last
herdr-stt transcribe recording.wav
journalctl --user -u herdr-stt.service -f
```

Audio files are temporary and deleted after transcription. Configuration lives in Herdr's plugin
config directory; print the exact location with:

```bash
herdr plugin config-dir herdr.stt
```

The installer copies a legacy `~/.config/herdr-stt/config.toml` when the new config does not yet
exist. After editing the file, apply it with `herdr-stt reload`.

## Recognition context

Use `[context]` for shared background and `[context.profiles.NAME]` for named sets of
terms, background files and language preferences. For example, in the plugin's `config.toml`:

```toml
[context]
default_profile = "backend"
text = "Работаем над сервисом обработки запросов."

[context.profiles.backend]
text = "Обсуждаем Redis, ClickHouse и обработчик UsernameParser."
hotwords = ["UsernameParser", "ClickHouse", "Tokio"]
language = "auto"
# files = ["contexts/backend.md"]

[[context.bindings]]
path = "~/Projects/backend"
profile = "backend"
```

Apply config changes with `herdr-stt reload`. Background files are read at the start of each
recording. Their paths are relative to the config file, not the source repository.

```bash
herdr-stt profiles
herdr-stt context --profile backend --cwd ~/Projects/backend
herdr-stt toggle --profile backend --context "Сейчас обсуждаем миграцию пользователей"
herdr-stt transcribe recording.wav --language ru --hotword RequestHandler \
  --context-file meeting-notes.txt --cwd ~/Projects/backend
```

`context` prints the effective hints without loading a model. Whisper receives a bounded initial
prompt and hotwords; Qwen receives background as reference data plus the same vocabulary.
Explicit profile selection takes precedence over the deepest matching directory binding and the
default profile. Per-recording language overrides profile and global settings. `auto` restores
multilingual detection. The recording retains its original context even if pane focus changes.
See [context reference](docs/context.md) for limits, privacy and examples.

## Repo-aware vocabulary

At recording start the plugin resolves the original pane's working directory and extracts a
bounded vocabulary from the active git repository: manifest dependencies, tracked filenames, and
frequent CamelCase/snake_case identifiers. The result is cached and supplied both as Whisper
hotwords and as constrained correction vocabulary. Disable or limit this under `[vocabulary]`.

## Optional Qwen corrector

The corrector is disabled by default and the normal installer does not download it. For the
managed local backend:

```bash
herdr-stt corrector install
```

Then edit `config.toml`:

```toml
[correction]
enabled = true
backend = "managed"
trigger = "uncertain"
endpoint = "http://127.0.0.1:18118/v1"
model = "qwen3-4b"

[correction.managed]
idle_seconds = 120
context_tokens = 2048
gpu_layers = "all"
```

The managed backend uses pinned `llama.cpp` and `Qwen3-4B-Q4_K_M`. Its service starts lazily and
unloads model/KV memory after the configured idle period. Inspect it with:

```bash
herdr-stt corrector status
journalctl --user -u herdr-stt-corrector.service -f
```

To use an existing Ollama or `llama-server`, set `backend = "openai"`, plus its OpenAI-compatible
`endpoint` and `model`. Non-loopback external endpoints are rejected unless `allow_remote = true`
is set explicitly, because the request contains transcript text. The managed backend is always
restricted to a loopback endpoint.

Qwen returns exact JSON patches rather than a rewritten paragraph. Patches that alter numbers,
negations, existing code tokens, or the detected intent are rejected. It is only used for ordinary
text insertion; `send prompt`, workspace switching, and Plan mode always use deterministic ASR
text. Context does not authorize extra edits or change this routing rule. Any timeout, invalid response, or unavailable model falls back to deterministic Whisper text.

## History and evaluation

When `[history].enabled = true`, raw, deterministic, and final transcripts plus confidence and
correction metadata are retained locally for seven days or 200 records. Audio is never retained.
The state directory and JSONL file are user-only (`0700`/`0600`).

```bash
herdr-stt last --raw
herdr-stt last --json
herdr-stt history list --limit 20
herdr-stt history export transcripts.jsonl
herdr-stt history clear
```

For a repeatable quality check, prepare JSONL records such as:

```json
{"audio":"samples/001.wav","reference":"Проверь UsernameParser и Redis lookup.","cwd":"/path/to/repo"}
```

Then run:

```bash
herdr-stt benchmark corpus.jsonl --output report.json
```

The report compares raw Whisper, deterministic normalization, and the configured corrector using
CER/WER, technical identifier accuracy, protected-token errors, correction counts, and latency.

## Development

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
uv run --project python --locked ruff check --config python/pyproject.toml python scripts
uv run --project python --locked pytest python/tests
```


The [architecture guide](docs/architecture.md) describes module boundaries and extension points.
Tests cover context resolution, actual daemon/worker IPC, recovery, command protection, and
installation paths without needing a microphone or downloading models. See the
[verification record](docs/verification.md) for live model and package checks.

## Package and publish

```bash
python3 scripts/package.py
```

This creates `dist/herdr-stt-0.3.0.tar.gz` and a SHA-256 file from an explicit source allowlist.
It excludes binaries, environments, models, user configuration, audio and history. The archive is a
source release: extract it and use `scripts/install-local.sh`; Herdr's GitHub installer reads the
same manifest directly from the repository. See [publication and removal](docs/publishing.md).
