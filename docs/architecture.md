# Architecture

The package has two small binary entrypoints and a reusable Rust library. Python is isolated
behind a JSON-lines worker protocol; application routing and configuration do not depend on
`faster-whisper` internals.

```text
Herdr action / CLI
  -> context hints + pane identity + repository vocabulary
  -> daemon RPC (Unix socket, version 3)
     -> context resolver (snapshot per recording)
     -> PipeWire recording / existing audio file
     -> supervised Whisper worker (JSON lines)
     -> shared transcript pipeline
        -> deterministic normalization
        -> optional corrector backend
        -> validated minimal edits
     -> local history
  -> deterministic intent routing to original Herdr pane
```

| Module | Responsibility |
| --- | --- |
| `src/cli/` | CLI parsing, dictation commands, routing, diagnostics, evaluation and managed model installation |
| `src/daemon/mod.rs` | RPC validation, idle/recording/transcribing state, worker lifetime |
| `src/daemon/recording.rs` | PipeWire process group, recording timeout and cleanup |
| `src/context.rs` | User context, profiles, bindings, bounded file reads and hint merging |
| `src/pipeline.rs` | Common normalization/correction path for microphone and file requests |
| `src/worker.rs` | Worker startup, timeout, one restart, request/response identity |
| `python/src/herdr_stt_worker/` | Model loading and `faster-whisper` adapter |
| `src/corrector/mod.rs` | Confidence triggers, correction orchestration, deadline and fallback |
| `src/corrector/backend.rs` | Managed/OpenAI-compatible HTTP transport and patch schema |
| `src/corrector/validation.rs` | Protected numbers, negations, identifiers and intent checks |
| `src/vocabulary.rs` | Bounded cached repository terms and deterministic aliases |
| `src/herdr.rs` | Calls through `HERDR_BIN_PATH` using the original pane/session context |
| `src/protocol.rs` | Versioned serializable socket contract |
| `src/config.rs`, `src/paths.rs` | Defaults, validation and relocatable config/runtime paths |
| `scripts/setup.py` | Idempotent config migration and quoted systemd unit generation |
| `scripts/package.py` | Reproducible allowlisted source archive |

## Extending the plugin

A new recognition engine can implement the worker ready/response messages and accept the same
`audio_path`, `hotwords`, `initial_prompt` and `language` fields. Set `[worker].python` to an
executable adapter accepting the current startup arguments, or extend the worker supervisor's
launcher for a different protocol. No change to Herdr routing is needed.

Correction transports belong in `corrector/backend.rs`; preserve the structured edit contract
and always pass output through `validation.rs`. Keep profile selection in `context.rs`, so audio
file evaluation and interactive recording use identical rules. Add deterministic routing rules
in `intent.rs` and host operations in `herdr.rs`.

## Lifetime and failure behavior

The socket is bound before loading the model to prevent duplicate model startups. The daemon
owns at most one recording or transcription at a time. Status does not wait on the model lock.
Whisper calls time out and retry once with a fresh worker; a second failure discards that worker.
The next request can recover. Last successful text survives a failed request. Qwen has one total
request budget and falls back to deterministic text. Audio cleanup runs on stop, cancel and
shutdown; the recording duration is capped even if the invoking client disappears.

The supported recording platform is Linux with PipeWire and systemd. Managed Qwen downloads
are currently Linux x86_64 Vulkan binaries. Other architectures may use an existing compatible
server or custom managed binary/model. This package does not claim macOS/Windows support.
