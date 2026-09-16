//! The agent the front ends talk to, whichever backend it runs on: Claude Code directly, or
//! any agent over the Agent Client Protocol. One type, one set of events.

use std::path::Path;

use anyhow::Result;
use serde_json::Value;

pub use crate::acp::{Event, PermissionOption, Role, prompts};

/// Keeps this server's own secrets out of the agent's environment. The agent runs commands
/// of its own, and anything it inherits it can pass on.
pub(crate) fn strip_secrets(command: &mut std::process::Command) {
    for name in [
        "CODEREVIEW_TOKEN",
        "CODEREVIEW_PASSWORD_HASH",
        "CODEREVIEW_CONFIG",
    ] {
        command.env_remove(name);
    }
}
use crate::config::AgentConfig;

pub enum Agent {
    Acp(crate::acp::Agent),
    Claude(crate::claude::Agent),
}

/// Starts the backend `config` selects: `claude` runs Claude Code, `acp` runs the configured
/// ACP command, and `auto` (the default) prefers Claude Code when it is on PATH.
pub fn spawn(config: &AgentConfig, root: &Path) -> Result<Agent> {
    match config.resolved_kind() {
        #[cfg(test)]
        "fake-acp" => Ok(Agent::Acp(crate::acp::Agent::start(
            crate::fakes::acp(),
            "fake",
            root,
        )?)),
        #[cfg(test)]
        "fake-claude" => Ok(Agent::Claude(crate::claude::Agent::start(
            crate::fakes::claude(),
            root,
        )?)),
        "claude" => Ok(Agent::Claude(crate::claude::Agent::spawn(
            &config.claude_command,
            &config.claude_args,
            root,
        )?)),
        "acp" => Ok(Agent::Acp(crate::acp::Agent::spawn(
            &config.command,
            &config.args,
            root,
        )?)),
        other => {
            anyhow::bail!("unknown [agent] kind {other:?}; use \"auto\", \"claude\" or \"acp\"")
        }
    }
}

impl Agent {
    pub fn name(&self) -> &str {
        match self {
            Agent::Acp(a) => &a.name,
            Agent::Claude(a) => a.name(),
        }
    }

    pub fn busy(&self) -> bool {
        match self {
            Agent::Acp(a) => a.busy(),
            Agent::Claude(a) => a.busy(),
        }
    }

    pub fn is_alive(&self) -> bool {
        match self {
            Agent::Acp(a) => a.is_alive(),
            Agent::Claude(a) => a.is_alive(),
        }
    }

    pub fn prompt(&mut self, text: &str, context: &[(String, String)]) -> Result<()> {
        match self {
            Agent::Acp(a) => a.prompt(text, context),
            Agent::Claude(a) => a.prompt(text, context),
        }
    }

    pub fn cancel(&mut self) -> Result<()> {
        match self {
            Agent::Acp(a) => a.cancel(),
            Agent::Claude(a) => a.cancel(),
        }
    }

    pub fn respond_permission(
        &mut self,
        request_id: &Value,
        option_id: Option<&str>,
    ) -> Result<()> {
        match self {
            Agent::Acp(a) => a.respond_permission(request_id, option_id),
            Agent::Claude(a) => a.respond_permission(request_id, option_id),
        }
    }

    pub fn poll(&mut self) -> Vec<Event> {
        match self {
            Agent::Acp(a) => a.poll(),
            Agent::Claude(a) => a.poll(),
        }
    }
}
