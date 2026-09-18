# Verification for 0.3.0

Validated on Linux with Herdr 0.9.1, Rust 1.95 and Python 3.12.

- Rust formatting and Clippy with warnings denied.
- 33 Rust tests: 26 library tests, 6 context-resolution tests and 1 full daemon IPC test.
- 9 Python tests for transcription options, shared hint-token budgeting and installation.
- The daemon integration test exercises a real subprocess worker, errors, timeout/restart,
  responsive status, context capture at recording start, original pane identity, cancellation,
  temporary-audio cleanup and graceful shutdown, using fixture recording/model adapters.
- Actual cached Whisper `small` on CPU INT8 recognized the ALSA Front Center sample. A second
  run used the relocated package and deliberately oversized context; it recognized the sample
  and reported context truncation.
- Actual Qwen3-4B-Q4_K_M through the pinned llama.cpp runtime, a 2048-token context and CPU
  inference exercised both managed and OpenAI-compatible request formats. Live verification
  retains the original action verb, negation and port number, including when no patch is accepted.
- A clean source archive was extracted, its worker installed without editable source references,
  then moved to a different directory containing spaces, `%` and `$`. Both binaries and the Python
  worker resolved their new locations. Setup generated valid units, checked by
  `systemd-analyze --user verify` without installing them into the user's service registry.
- Herdr parsed the versioned manifest and exposed its actions/startup hook through `plugin list`.
- Source packaging was checked for excluded environments/audio/binaries and reproducible hashes.

The normal suite needs no models or microphone. To run the optional live corrector test against
an already running local Qwen server:

```bash
HERDR_STT_TEST_QWEN_ENDPOINT=http://127.0.0.1:18118/v1 \
  cargo test --test live_corrector -- --ignored --nocapture
```

These are functional checks, not a claim of transcription accuracy on arbitrary recordings.
Microphone capture and insertion into a live user pane were not exercised; the IPC test uses a
fixture recorder. Use `herdr-stt benchmark` with a representative recorded corpus to measure WER,
CER and whether optional corrections improve your own dictation.
