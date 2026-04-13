#[derive(Debug, Clone)]
pub struct AuthInfo {
    pub api_key: String,
    pub strategy: AuthStrategy,
    pub access_token: Option<String>,
}

impl AuthInfo {
    pub fn new(api_key: String, strategy: AuthStrategy) -> Self {
        Self {
            api_key,
            strategy,
            access_token: None,
        }
    }

    pub fn with_access_token(api_key: String, access_token: String) -> Self {
        Self {
            api_key,
            strategy: AuthStrategy::GoogleOAuth,
            access_token: Some(access_token),
        }
    }

    #[allow(dead_code)]
    pub fn masked_key(&self) -> String {
        if self.api_key.chars().count() > 8 {
            let prefix: String = self.api_key.chars().take(4).collect();
            let suffix: String = self
                .api_key
                .chars()
                .rev()
                .take(4)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            format!("{prefix}...{suffix}")
        } else {
            "***".to_string()
        }
    }

    #[allow(dead_code)]
    pub fn masked_access_token(&self) -> Option<String> {
        self.access_token.as_ref().map(|token| {
            if token.chars().count() > 8 {
                let prefix: String = token.chars().take(4).collect();
                let suffix: String = token
                    .chars()
                    .rev()
                    .take(4)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect();
                format!("{prefix}...{suffix}")
            } else {
                "***".to_string()
            }
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthStrategy {
    Anthropic,
    ClaudeAuth,
    Bearer,
    Google,
    GoogleOAuth,
    GitHubCopilot,
    CodexOAuth,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masked_key_handles_long_and_short_values() {
        let long = AuthInfo::new("sk-1234567890abcdef".to_string(), AuthStrategy::Bearer);
        let short = AuthInfo::new("short".to_string(), AuthStrategy::Bearer);

        assert_eq!(long.masked_key(), "sk-1...cdef");
        assert_eq!(short.masked_key(), "***");
    }

    #[test]
    fn masked_access_token_handles_long_and_short_values() {
        let long = AuthInfo::with_access_token(
            "refresh-token".to_string(),
            "ya29.1234567890abcdef".to_string(),
        );
        let short = AuthInfo::with_access_token("refresh-token".to_string(), "short".to_string());

        assert_eq!(long.masked_access_token(), Some("ya29...cdef".to_string()));
        assert_eq!(short.masked_access_token(), Some("***".to_string()));
    }

    #[test]
    fn all_auth_strategies_are_distinct() {
        let strategies = [
            AuthStrategy::Anthropic,
            AuthStrategy::ClaudeAuth,
            AuthStrategy::Bearer,
            AuthStrategy::Google,
            AuthStrategy::GoogleOAuth,
            AuthStrategy::GitHubCopilot,
            AuthStrategy::CodexOAuth,
        ];

        for (left_index, left) in strategies.iter().enumerate() {
            for (right_index, right) in strategies.iter().enumerate() {
                if left_index == right_index {
                    assert_eq!(left, right);
                } else {
                    assert_ne!(left, right);
                }
            }
        }
    }
}
