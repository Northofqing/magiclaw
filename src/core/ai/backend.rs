use async_trait::async_trait;

use crate::domain::error::AiError;

/// A pluggable AI backend for generating responses.
#[async_trait]
pub trait AiBackend: Send + Sync {
    /// Backend name for logging and config selection.
    fn name(&self) -> &'static str;

    /// Generate a response for the given input text.
    async fn generate(&self, input: &str, context: Option<&str>) -> Result<String, AiError>;
}
