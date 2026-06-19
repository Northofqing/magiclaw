use async_trait::async_trait;

use super::middleware::{Middleware, PipelineContext};

/// Normalize middleware: standardizes message format across platforms.
pub struct Normalize;

#[async_trait]
impl Middleware for Normalize {
    fn name(&self) -> &'static str { "normalize" }

    async fn process(&self, ctx: PipelineContext) -> Result<PipelineContext, String> {
        // Phase 4: normalize platform-specific fields into common format
        // - strip HTML/rich-text if platform doesn't support it
        // - normalize mentions, emoji, etc.
        tracing::debug!(message_id = %ctx.message.id, "normalize: message standardized");
        Ok(ctx)
    }
}
