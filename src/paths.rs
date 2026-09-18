use std::env;
use std::path::{Path, PathBuf};

pub const SOCKET_NAME: &str = "stt.sock";

pub fn project_root() -> PathBuf {
    env::var_os("HERDR_STT_PROJECT_ROOT")
        .or_else(|| env::var_os("HERDR_PLUGIN_ROOT"))
        .map(PathBuf::from)
        .or_else(|| {
            let executable = env::current_exe().ok()?;
            let parent = executable.parent()?;
            let root = if parent.file_name()?.to_str()? == "bin" {
                parent.parent()?
            } else {
                parent.parent()?.parent()?
            };
            root.join("herdr-plugin.toml")
                .is_file()
                .then(|| root.to_path_buf())
        })
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")))
}

pub fn config_path() -> PathBuf {
    env::var_os("HERDR_STT_CONFIG")
        .map(PathBuf::from)
        .or_else(|| {
            env::var_os("HERDR_PLUGIN_CONFIG_DIR")
                .map(PathBuf::from)
                .map(|path| path.join("config.toml"))
        })
        .unwrap_or_else(|| plugin_config_dir().join("config.toml"))
}

pub fn legacy_config_path() -> PathBuf {
    config_home().join("herdr-stt/config.toml")
}

pub fn plugin_config_dir() -> PathBuf {
    config_home().join("herdr/plugins/config/herdr.stt")
}

pub fn state_dir() -> PathBuf {
    env::var_os("HERDR_PLUGIN_STATE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| state_home().join("herdr/plugins/herdr.stt"))
}

pub fn history_path() -> PathBuf {
    state_dir().join("transcripts.jsonl")
}

pub fn vocabulary_cache_dir() -> PathBuf {
    state_dir().join("vocabulary")
}

pub fn corrector_dir() -> PathBuf {
    state_dir().join("corrector")
}

pub fn corrector_binary() -> PathBuf {
    corrector_dir().join("runtime/llama-server")
}

pub fn corrector_model() -> PathBuf {
    corrector_dir().join("models/Qwen3-4B-Q4_K_M.gguf")
}

pub fn runtime_dir() -> PathBuf {
    env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(format!("/tmp/herdr-stt-{}", unsafe { libc::geteuid() })))
        .join("herdr-stt")
}

pub fn socket_path() -> PathBuf {
    env::var_os("HERDR_STT_SOCKET")
        .map(PathBuf::from)
        .unwrap_or_else(|| runtime_dir().join(SOCKET_NAME))
}

pub fn default_python() -> PathBuf {
    project_root().join("python/.venv/bin/python")
}

pub fn ensure_parent(path: &Path) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    Ok(())
}

fn config_home() -> PathBuf {
    env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .unwrap_or_else(|| PathBuf::from(".config"))
}

fn state_home() -> PathBuf {
    env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state")))
        .unwrap_or_else(|| PathBuf::from(".local/state"))
}
