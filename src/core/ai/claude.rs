use async_trait::async_trait;

use super::backend::AiBackend;

/// Claude/Anthropic backend stub. Full implementation requires API key and HTTP client.
pub struct ClaudeBackend {
    _api_key: String,
    _model: String,
}

impl ClaudeBackend {
    pub fn new(api_key: impl Into<String>, model: impl Into<String>) -> Self {
        Self { _api_key: api_key.into(), _model: model.into() }
    }
}

#[async_trait]
impl AiBackend for ClaudeBackend {
    fn name(&self) -> &'static str { "claude" }

    async fn generate(&self, _input: &str, _context: Option<&str>) -> Result<String, String> {
        tracing::warn!("Claude backend is a stub — returning echo response");
        Ok("[claude stub] response placeholder".into())
    }
}
