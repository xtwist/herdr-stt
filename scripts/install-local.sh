#!/usr/bin/env bash
set -euo pipefail
PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
HERDR_COMMAND="${HERDR_BIN_PATH:-herdr}"
"$PROJECT_ROOT/scripts/build.sh"
"$HERDR_COMMAND" plugin link "$PROJECT_ROOT"
export HERDR_PLUGIN_CONFIG_DIR
HERDR_PLUGIN_CONFIG_DIR="$("$HERDR_COMMAND" plugin config-dir herdr.stt)"
"$PROJECT_ROOT/bin/herdr-stt" setup --keybinding
install -d "$HOME/.local/bin"
ln -sfn "$PROJECT_ROOT/bin/herdr-stt" "$HOME/.local/bin/herdr-stt"
echo "Herdr STT installed. The model loads on first dictation. Run: herdr-stt doctor"
