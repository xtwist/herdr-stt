#!/usr/bin/env bash
set -euo pipefail
PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
for tool in cargo uv; do
    command -v "$tool" >/dev/null || { echo "Required tool is missing: $tool" >&2; exit 1; }
done
ACCELERATOR="${HERDR_STT_ACCELERATOR:-auto}"
if [[ "$ACCELERATOR" == auto ]]; then
    ACCELERATOR=cpu
    if command -v nvidia-smi >/dev/null && nvidia-smi -L >/dev/null 2>&1; then
        ACCELERATOR=cuda
    fi
fi
case "$ACCELERATOR" in
    cuda) uv sync --project "$PROJECT_ROOT/python" --locked --no-dev --no-editable --extra cuda ;;
    cpu) uv sync --project "$PROJECT_ROOT/python" --locked --no-dev --no-editable ;;
    *) echo "HERDR_STT_ACCELERATOR must be auto, cpu or cuda" >&2; exit 1 ;;
esac
cargo build --manifest-path "$PROJECT_ROOT/Cargo.toml" --release --locked
install -d "$PROJECT_ROOT/bin"
install -m 0755 "$PROJECT_ROOT/target/release/herdr-stt" "$PROJECT_ROOT/bin/herdr-stt"
install -m 0755 "$PROJECT_ROOT/target/release/herdr-sttd" "$PROJECT_ROOT/bin/herdr-sttd"
