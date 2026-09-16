//! The user's preferences, shared by both front ends: `$XDG_CONFIG_HOME/codereview/config.toml`
//! (`~/.config/codereview/config.toml`), or the file `$CODEREVIEW_CONFIG` names.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// A built-in theme name; see `codereview::theme::all`.
    pub theme: String,
    /// `auto`, `side-by-side` or `unified`.
    pub layout: String,
    /// Whether new comments carry a timestamp.
    pub timestamps: bool,
    pub agent: AgentConfig,
}

/// The agent both front ends can talk to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentConfig {
    /// `auto`, `claude` or `acp`. `auto` runs Claude Code when `claude` is on PATH and the
    /// ACP command otherwise.
    pub kind: String,
    /// The Claude Code binary, and extra arguments for it (`--model`, `--permission-mode`).
    pub claude_command: String,
    pub claude_args: Vec<String>,
    /// The ACP agent: a program that speaks ACP over stdio, with its arguments.
    pub command: String,
    pub args: Vec<String>,
}

impl Default for AgentConfig {
    /// Claude Code directly. The ACP fallback is Claude Code through Zed's adapter, which
    /// needs Node 20 or newer; it is only reached when `claude` itself is missing.
    fn default() -> Self {
        Self {
            kind: "auto".into(),
            claude_command: "claude".into(),
            claude_args: Vec::new(),
            command: "npx".into(),
            args: vec!["-y".into(), "@zed-industries/claude-code-acp".into()],
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            theme: crate::theme::Theme::default().name,
            layout: "auto".into(),
            timestamps: true,
            agent: AgentConfig::default(),
        }
    }
}

impl Config {
    /// The user's config file location.
    pub fn path() -> Option<PathBuf> {
        if let Ok(p) = std::env::var("CODEREVIEW_CONFIG") {
            return Some(PathBuf::from(p));
        }
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
        Some(base.join("codereview").join("config.toml"))
    }

    /// The saved configuration, or the defaults when there is none. A malformed file is an
    /// error: silently using defaults would hide a typo in a hand-edited value.
    pub fn load() -> Result<Self> {
        match Self::path() {
            Some(path) => Self::load_from(&path),
            None => Ok(Self::default()),
        }
    }

    pub fn load_from(path: &Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                toml::from_str(&text).with_context(|| format!("cannot parse {}", path.display()))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e).with_context(|| format!("cannot read {}", path.display())),
        }
    }

    pub fn save(&self) -> Result<()> {
        let Some(path) = Self::path() else {
            anyhow::bail!("no config directory: set HOME, XDG_CONFIG_HOME or CODEREVIEW_CONFIG");
        };
        self.save_to(&path)
    }

    pub fn save_to(&self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .with_context(|| format!("cannot create {}", dir.display()))?;
        }
        let text = toml::to_string_pretty(self).context("serialise config")?;
        std::fs::write(path, text).with_context(|| format!("cannot write {}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_keys_take_defaults() {
        let c: Config = toml::from_str("theme = \"Nord\"\n").unwrap();
        assert_eq!(c.theme, "Nord");
        assert_eq!(c.layout, "auto");
        assert!(c.timestamps);
        assert_eq!(c.agent.command, "npx");
        let c: Config =
            toml::from_str("[agent]\ncommand = \"gemini\"\nargs = [\"--experimental-acp\"]\n")
                .unwrap();
        assert_eq!(c.agent.args, vec!["--experimental-acp"]);
        let back: Config = toml::from_str(&toml::to_string(&c).unwrap()).unwrap();
        assert_eq!(back, c);
    }
}
