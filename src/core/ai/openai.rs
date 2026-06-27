use async_trait::async_trait;

use crate::domain::error::AiError;

use super::backend::AiBackend;

/// OpenAI backend stub. Full implementation requires API key and HTTP client.
pub struct OpenAiBackend {
    _api_key: String,
    _model: String,
}

impl OpenAiBackend {
    pub fn new(api_key: impl Into<String>, model: impl Into<String>) -> Self {
        Self { _api_key: api_key.into(), _model: model.into() }
    }
}

#[async_trait]
impl AiBackend for OpenAiBackend {
    fn name(&self) -> &'static str { "openai" }

    async fn generate(&self, _input: &str, _context: Option<&str>) -> Result<String, AiError> {
        // Stub: in production this calls the OpenAI chat completions API
        tracing::warn!("OpenAI backend is a stub — returning echo response");
        Ok("[openai stub] response placeholder".into())
    }
}
