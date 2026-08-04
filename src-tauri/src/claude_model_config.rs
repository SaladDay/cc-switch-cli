use serde_json::{Map, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClaudeModelRole {
    Haiku,
    Sonnet,
    Opus,
    Fable,
    Subagent,
}

impl ClaudeModelRole {
    /// TUI order preserves the existing three rows and appends the new roles.
    pub(crate) const ALL: [Self; 5] = [
        Self::Haiku,
        Self::Sonnet,
        Self::Opus,
        Self::Fable,
        Self::Subagent,
    ];
    pub(crate) const COUNT: usize = Self::ALL.len();

    pub(crate) const fn from_index(index: usize) -> Option<Self> {
        match index {
            0 => Some(Self::Haiku),
            1 => Some(Self::Sonnet),
            2 => Some(Self::Opus),
            3 => Some(Self::Fable),
            4 => Some(Self::Subagent),
            _ => None,
        }
    }

    pub(crate) const fn index(self) -> usize {
        match self {
            Self::Haiku => 0,
            Self::Sonnet => 1,
            Self::Opus => 2,
            Self::Fable => 3,
            Self::Subagent => 4,
        }
    }

    pub(crate) const fn model_env_key(self) -> &'static str {
        match self {
            Self::Haiku => "ANTHROPIC_DEFAULT_HAIKU_MODEL",
            Self::Sonnet => "ANTHROPIC_DEFAULT_SONNET_MODEL",
            Self::Opus => "ANTHROPIC_DEFAULT_OPUS_MODEL",
            Self::Fable => "ANTHROPIC_DEFAULT_FABLE_MODEL",
            Self::Subagent => "CLAUDE_CODE_SUBAGENT_MODEL",
        }
    }

    pub(crate) const fn display_name_env_key(self) -> Option<&'static str> {
        match self {
            Self::Haiku => Some("ANTHROPIC_DEFAULT_HAIKU_MODEL_NAME"),
            Self::Sonnet => Some("ANTHROPIC_DEFAULT_SONNET_MODEL_NAME"),
            Self::Opus => Some("ANTHROPIC_DEFAULT_OPUS_MODEL_NAME"),
            Self::Fable => Some("ANTHROPIC_DEFAULT_FABLE_MODEL_NAME"),
            Self::Subagent => None,
        }
    }

    pub(crate) const fn supports_one_m(self) -> bool {
        !matches!(self, Self::Haiku)
    }

    pub(crate) const fn takeover_model(self) -> Option<&'static str> {
        match self {
            Self::Haiku => Some("claude-haiku-4-5"),
            Self::Sonnet => Some("claude-sonnet-4-6"),
            Self::Opus => Some("claude-opus-4-8"),
            Self::Fable => Some("claude-fable-5"),
            Self::Subagent => None,
        }
    }
}

pub(crate) const CLAUDE_DEFAULT_MODEL_ENV_KEY: &str = "ANTHROPIC_MODEL";
pub(crate) const CLAUDE_LEGACY_SMALL_FAST_MODEL_ENV_KEY: &str = "ANTHROPIC_SMALL_FAST_MODEL";
pub(crate) const CLAUDE_SUBAGENT_MODEL_ENV_KEY: &str = "CLAUDE_CODE_SUBAGENT_MODEL";
pub(crate) const CLAUDE_CONTEXT_WINDOW_ENV_KEYS: [&str; 2] = [
    "CLAUDE_CODE_MAX_CONTEXT_TOKENS",
    "CLAUDE_CODE_AUTO_COMPACT_WINDOW",
];

pub(crate) fn split_claude_one_m_marker(value: &str) -> (String, bool) {
    const MARKER: &str = "[1M]";

    let trimmed_end = value.trim_end();
    let marker_start = trimmed_end.len().saturating_sub(MARKER.len());
    let has_marker = trimmed_end
        .get(marker_start..)
        .is_some_and(|suffix| suffix.eq_ignore_ascii_case(MARKER));
    if !has_marker {
        return (value.to_string(), false);
    }

    (
        trimmed_end
            .get(..marker_start)
            .unwrap_or_default()
            .trim_end()
            .to_string(),
        true,
    )
}

pub(crate) fn set_claude_role_model(
    env: &mut Map<String, Value>,
    role: ClaudeModelRole,
    value: &str,
) {
    let previous_model = env
        .get(role.model_env_key())
        .and_then(Value::as_str)
        .map(|value| split_claude_one_m_marker(value).0.trim().to_string());
    let display_name_key = role.display_name_env_key();
    let sync_display_name = display_name_key.is_some_and(|key| {
        env.get(key)
            .and_then(Value::as_str)
            .is_none_or(|display_name| {
                display_name.trim().is_empty()
                    || previous_model.as_deref() == Some(display_name.trim())
            })
    });

    let normalized_value;
    let value = if role.supports_one_m() {
        value
    } else {
        normalized_value = split_claude_one_m_marker(value).0;
        &normalized_value
    };

    set_or_remove_trimmed(env, role.model_env_key(), value);
    if sync_display_name {
        let (model_name, _) = split_claude_one_m_marker(value);
        set_or_remove_trimmed(
            env,
            display_name_key.expect("display-name key was present"),
            &model_name,
        );
    }
}

fn set_or_remove_trimmed(env: &mut Map<String, Value>, key: &str, value: &str) {
    let value = value.trim();
    if value.is_empty() {
        env.remove(key);
    } else {
        env.insert(key.to_string(), Value::String(value.to_string()));
    }
}

pub(crate) const CLAUDE_ROUTED_MODEL_ENV_KEYS: [&str; 6] = [
    CLAUDE_DEFAULT_MODEL_ENV_KEY,
    ClaudeModelRole::Haiku.model_env_key(),
    ClaudeModelRole::Sonnet.model_env_key(),
    ClaudeModelRole::Opus.model_env_key(),
    ClaudeModelRole::Fable.model_env_key(),
    CLAUDE_SUBAGENT_MODEL_ENV_KEY,
];

/// Provider-scoped model settings removed before proxy takeover aliases are
/// written and excluded from shared Claude configuration.
pub(crate) const CLAUDE_MODEL_OVERRIDE_ENV_KEYS: [&str; 12] = [
    CLAUDE_DEFAULT_MODEL_ENV_KEY,
    "ANTHROPIC_REASONING_MODEL",
    "ANTHROPIC_DEFAULT_HAIKU_MODEL",
    "ANTHROPIC_DEFAULT_HAIKU_MODEL_NAME",
    "ANTHROPIC_DEFAULT_SONNET_MODEL",
    "ANTHROPIC_DEFAULT_SONNET_MODEL_NAME",
    "ANTHROPIC_DEFAULT_OPUS_MODEL",
    "ANTHROPIC_DEFAULT_OPUS_MODEL_NAME",
    "ANTHROPIC_DEFAULT_FABLE_MODEL",
    "ANTHROPIC_DEFAULT_FABLE_MODEL_NAME",
    CLAUDE_LEGACY_SMALL_FAST_MODEL_ENV_KEY,
    CLAUDE_SUBAGENT_MODEL_ENV_KEY,
];

pub(crate) fn claude_provider_owned_env_keys() -> impl Iterator<Item = &'static str> {
    CLAUDE_MODEL_OVERRIDE_ENV_KEYS
        .into_iter()
        .chain(CLAUDE_CONTEXT_WINDOW_ENV_KEYS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn haiku_writer_strips_unsupported_one_m_marker() {
        let mut env = Map::new();
        set_claude_role_model(&mut env, ClaudeModelRole::Haiku, "claude-haiku-4-5 [1m] ");

        assert_eq!(
            env.get(ClaudeModelRole::Haiku.model_env_key())
                .and_then(Value::as_str),
            Some("claude-haiku-4-5")
        );
    }
}
