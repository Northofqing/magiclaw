use async_trait::async_trait;

use super::backend::AiBackend;

/// Echo backend: returns the input as-is. Used as fallback/development backend.
pub struct EchoBackend;

#[async_trait]
impl AiBackend for EchoBackend {
    fn name(&self) -> &'static str { "echo" }

    async fn generate(&self, input: &str, _context: Option<&str>) -> Result<String, String> {
        tracing::debug!(input_len = input.len(), "echo backend: returning input");
        Ok(format!("[echo] {}", input))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn echo_returns_input() {
        let be = EchoBackend;
        let result = be.generate("hello", None).await.unwrap();
        assert_eq!(result, "[echo] hello");
    }
}
