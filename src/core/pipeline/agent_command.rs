/// Agent command parsing and recognition for per-user agent switching.
use std::collections::HashMap;

/// Represents a recognized agent command in the incoming message.
#[derive(Debug, Clone, PartialEq)]
pub enum AgentCommand {
    /// Switch to an agent (command only, no content to process).
    /// Contains the target agent name (e.g., "claude_code").
    Switch(String),

    /// Query current agent preference (e.g., "当前 agent", "/agent").
    Query,

    /// Switch to an agent and process the remaining text as the message.
    /// (agent_name, remaining_text)
    SwitchAndProcess(String, String),

    /// Not a command; process as normal message.
    NotCommand,
}

/// Parser for agent commands, driven by configurable aliases.
pub struct AgentCommandParser {
    /// Mapping from agent name to list of aliases.
    /// Example: "claude_code" -> ["cc", "claude", "claude code"]
    pub aliases: HashMap<String, Vec<String>>,
}

impl AgentCommandParser {
    pub fn new(aliases: HashMap<String, Vec<String>>) -> Self {
        Self { aliases }
    }

    /// Parse a text message to detect agent commands.
    /// Returns AgentCommand::NotCommand if it's not a recognized command.
    pub fn parse(&self, text: &str) -> AgentCommand {
        let trimmed = text.trim();

        // Check for query commands: "当前 agent", "/agent", "当前agent"
        if matches!(
            trimmed.to_lowercase().as_str(),
            "当前 agent" | "当前agent" | "/agent" | "agent"
        ) {
            return AgentCommand::Query;
        }

        // Collect all (agent_name, alias, alias_len) tuples and sort by alias length descending.
        // This ensures longest aliases are checked first, so "claude code" is checked before "claude".
        let mut candidates: Vec<(String, String, usize)> = Vec::new();
        for (agent_name, agent_aliases) in &self.aliases {
            for alias in agent_aliases {
                candidates.push((agent_name.clone(), alias.clone(), alias.len()));
            }
        }
        candidates.sort_by_key(|b| std::cmp::Reverse(b.2)); // Sort by length descending

        // Try to match against sorted aliases.
        for (agent_name, alias, _) in candidates {
            // Check standalone command (just the alias, optionally with /)
            if trimmed == alias || trimmed == format!("/{}", alias) {
                return AgentCommand::Switch(agent_name);
            }

            // Check command with space separator or slash prefix
            // e.g., "cc " or "cc content", "/cc content"
            let prefix_with_space = format!("{} ", alias);
            let prefix_with_slash = format!("/{} ", alias);

            if let Some(rest) = trimmed.strip_prefix(&prefix_with_space) {
                let content = rest.trim();
                if !content.is_empty() {
                    return AgentCommand::SwitchAndProcess(agent_name, content.to_string());
                }
            }

            if let Some(rest) = trimmed.strip_prefix(&prefix_with_slash) {
                let content = rest.trim();
                if !content.is_empty() {
                    return AgentCommand::SwitchAndProcess(agent_name, content.to_string());
                }
            }
        }

        // Not a command.
        AgentCommand::NotCommand
    }

    /// Find the agent name that matches a given alias (case-insensitive).
    /// Returns None if the alias is not recognized.
    pub fn resolve_alias(&self, text: &str) -> Option<String> {
        let lower = text.to_lowercase();
        for (agent_name, agent_aliases) in &self.aliases {
            for alias in agent_aliases {
                if alias.to_lowercase() == lower {
                    return Some(agent_name.clone());
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_parser() -> AgentCommandParser {
        let mut aliases = HashMap::new();
        aliases.insert(
            "claude_code".to_string(),
            vec!["cc".to_string(), "claude".to_string(), "claude code".to_string()],
        );
        aliases.insert(
            "codex".to_string(),
            vec!["cx".to_string(), "codex".to_string()],
        );
        aliases.insert(
            "openclaw".to_string(),
            vec!["oc".to_string(), "openclaw".to_string()],
        );
        aliases.insert(
            "hermes".to_string(),
            vec!["h".to_string(), "hermes".to_string()],
        );
        AgentCommandParser::new(aliases)
    }

    #[test]
    fn test_parse_standalone_switch_short() {
        let parser = make_parser();
        assert_eq!(
            parser.parse("cc"),
            AgentCommand::Switch("claude_code".to_string())
        );
        assert_eq!(
            parser.parse("cx"),
            AgentCommand::Switch("codex".to_string())
        );
        assert_eq!(
            parser.parse("oc"),
            AgentCommand::Switch("openclaw".to_string())
        );
        assert_eq!(
            parser.parse("h"),
            AgentCommand::Switch("hermes".to_string())
        );
    }

    #[test]
    fn test_parse_standalone_switch_with_slash() {
        let parser = make_parser();
        assert_eq!(
            parser.parse("/cc"),
            AgentCommand::Switch("claude_code".to_string())
        );
        assert_eq!(
            parser.parse("/cx"),
            AgentCommand::Switch("codex".to_string())
        );
    }

    #[test]
    fn test_parse_standalone_switch_full_name() {
        let parser = make_parser();
        assert_eq!(
            parser.parse("claude"),
            AgentCommand::Switch("claude_code".to_string())
        );
        assert_eq!(
            parser.parse("claude code"),
            AgentCommand::Switch("claude_code".to_string())
        );
        assert_eq!(
            parser.parse("codex"),
            AgentCommand::Switch("codex".to_string())
        );
    }

    #[test]
    fn test_parse_switch_and_process() {
        let parser = make_parser();
        assert_eq!(
            parser.parse("cc 帮我总结这个"),
            AgentCommand::SwitchAndProcess(
                "claude_code".to_string(),
                "帮我总结这个".to_string()
            )
        );
        assert_eq!(
            parser.parse("/cx 这段代码有 bug 吗"),
            AgentCommand::SwitchAndProcess(
                "codex".to_string(),
                "这段代码有 bug 吗".to_string()
            )
        );
        assert_eq!(
            parser.parse("openclaw 怎么部署"),
            AgentCommand::SwitchAndProcess(
                "openclaw".to_string(),
                "怎么部署".to_string()
            )
        );
    }

    #[test]
    fn test_parse_query() {
        let parser = make_parser();
        assert_eq!(parser.parse("当前 agent"), AgentCommand::Query);
        assert_eq!(parser.parse("当前agent"), AgentCommand::Query);
        assert_eq!(parser.parse("/agent"), AgentCommand::Query);
        assert_eq!(parser.parse("agent"), AgentCommand::Query);
    }

    #[test]
    fn test_parse_not_command() {
        let parser = make_parser();
        assert_eq!(parser.parse("hello world"), AgentCommand::NotCommand);
        assert_eq!(parser.parse("这是一个普通消息"), AgentCommand::NotCommand);
        assert_eq!(parser.parse("xxx"), AgentCommand::NotCommand);
    }

    #[test]
    fn test_parse_whitespace_handling() {
        let parser = make_parser();
        assert_eq!(
            parser.parse("  cc  "),
            AgentCommand::Switch("claude_code".to_string())
        );
        assert_eq!(
            parser.parse("cc   帮我总结"),
            AgentCommand::SwitchAndProcess(
                "claude_code".to_string(),
                "帮我总结".to_string()
            )
        );
    }

    #[test]
    fn test_resolve_alias() {
        let parser = make_parser();
        assert_eq!(parser.resolve_alias("cc"), Some("claude_code".to_string()));
        assert_eq!(parser.resolve_alias("CC"), Some("claude_code".to_string()));
        assert_eq!(
            parser.resolve_alias("claude code"),
            Some("claude_code".to_string())
        );
        assert_eq!(parser.resolve_alias("cx"), Some("codex".to_string()));
        assert_eq!(parser.resolve_alias("unknown"), None);
    }
}
