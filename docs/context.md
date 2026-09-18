# Recognition context

Configure `config.toml` under `herdr plugin config-dir herdr.stt`. Existing 0.2 configuration
continues to work. `transcription.initial_prompt` and `transcription.hotwords` remain supported.

| Setting | Meaning | Default |
| --- | --- | --- |
| `context.text` | Shared background prose | Empty |
| `context.files` | Explicit UTF-8 reference files | Empty |
| `context.hotwords` | Names and preferred spellings | Empty |
| `context.language` | Whisper code (`ru`, `uk`, `en`, etc.) or `auto` | Auto |
| `context.default_profile` | Profile used when no binding or explicit choice applies | None |
| `context.profiles.NAME` | `text`, `files`, `hotwords`, `language` for a named profile | Empty |
| `context.bindings` | Array of `{ path, profile }` directory bindings | Empty |
| `context.max_chars` | Reference text budget, Unicode characters | 4000 |
| `context.max_prompt_chars` | Whisper prompt budget, Unicode characters | 800 |
| `context.max_file_bytes` | Maximum size of each reference file; larger files are rejected | 65536 |
| `context.max_hotwords` | Maximum distinct recognition hotwords | 128 |

The configured source of context is user-owned. The plugin never reads a repository's arbitrary
instructions as configuration and does not automatically include selected text or entire files.
Repository vocabulary remains controlled separately by `[vocabulary]`.

## Selection and merging

1. `--profile NAME` selects that profile, otherwise the deepest matching directory in
   `context.bindings` wins, otherwise `default_profile` applies.
2. Reference text is ordered from most specific to broadest: `--context` and `--context-file`,
   profile text/files, global text/files. Bounded truncation preserves the beginning.
3. Whisper receives that text followed by the existing `transcription.initial_prompt`, within
   `max_prompt_chars`. The worker then shares a 223-token hint budget between hotwords and the beginning of the
   prompt, reserving the rest of the decoder window for speech. This extra truncation also sets
   `context_truncated`; keep useful hints short.
4. Hotwords are combined in this order: command line, profile, global context, legacy
   transcription terms, workspace/repository terms. Duplicates are removed without regard to
   letter case; the first spelling wins. Each term is bounded to 120 characters.
5. Language precedence is command line, profile, global. Use `auto` to undo a forced language.
   An explicit language disables per-segment multilingual detection for that request.

Bindings match canonical directory ancestors (including symlinks), never textual prefixes.
Missing bound directories do not match. Relative binding and configured file paths resolve
against the loaded config file. `~` expands to the user's home. CLI `--context-file` paths are
relative to the shell working directory. Missing, oversized and non-UTF-8 files produce a clear
error before recording starts. A misspelled profile fails instead of silently choosing a default.

A profile and its file contents are captured at recording start. A stop action uses that snapshot,
even after switching panes or changing the files. Config changes need `herdr-stt reload`; file
contents are reread for the next recording. `context_profile` and `context_truncated` appear in
JSON transcripts/history; background text itself is not written to history.

## Example

```toml
[context]
text = "Диктуем рабочие задачи на русском, названия технологий на английском."
default_profile = "engineering"

[context.profiles.engineering]
text = "Сервис маршрутизирует SOCKS5-соединения через Rust и Tokio."
hotwords = ["SessionRouter", "SOCKS5", "Tokio"]
files = ["contexts/glossary.md"]

[context.profiles.meeting]
text = "Участники встречи: Юрий, Олена, Michael. Обсуждаем план релиза."
hotwords = ["Олена", "Michael"]
language = "ru"

[[context.bindings]]
path = "~/Projects/router"
profile = "engineering"
```

Create the referenced file before enabling the profile. Useful content is terminology, names,
subject matter and examples of expected spelling. Context is a recognition hint; it cannot
ensure verbatim transcription and should not contain unrelated long documents.

```bash
herdr-stt context --profile meeting
herdr-stt transcribe meeting.wav --profile meeting --context "Тема: релиз версии 3"
```

## Correction and privacy

Qwen gets a JSON data payload with `reference_context`, `vocabulary` and `transcript`. It can
suggest minimal patches only; the same deterministic validation applies with or without context.
Voice commands are routed from deterministic text. Correction failures return that text.

With the default local setup, audio and context stay on the machine. Model downloads use the
network. If a remote correction endpoint is explicitly enabled with `allow_remote = true`,
transcript chunks, vocabulary and reference context are sent there. HTTP redirects are disabled.
Audio recorded by the plugin is temporary; audio passed to `transcribe` is never deleted.
The optional local history retains transcripts, not recordings or background documents.
