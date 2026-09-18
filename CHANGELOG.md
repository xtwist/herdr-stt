# Changelog

## 0.3.0

- Split command routing, daemon recording, transcript processing, correction transport and edit
  validation into independent modules behind small binary entrypoints.
- Add global/profile/per-recording context, UTF-8 reference files, directory bindings, language
  selection, vocabulary hints, bounded merging, profile listing and context preview.
- Pass context to both Whisper and optional Qwen; preserve the same voice-command protections.
- Support standard Herdr GitHub installation with idempotent runtime setup, relocatable binaries,
  quoted service paths, CPU-only dependencies and opt-in CUDA packages.
- Bound Whisper context/hotwords jointly by tokenizer length; accept Qwen reasoning delimiters
  around structured JSON without accepting arbitrary prose. Reject vocabulary patches that
  swallow neighboring words or replace existing code identifiers.
- Bound model waits, keep status responsive during inference, discard failed workers and disable
  correction HTTP redirects. Correct normalized text rather than stale raw segment text.
- Add daemon integration tests, context and installation tests, CI and reproducible source releases.
- Keep 0.2 configuration/history compatibility; bump the daemon protocol to version 3.
