use async_trait::async_trait;

use crate::domain::error::PipelineError;

use super::middleware::{Middleware, PipelineContext};

/// Permission middleware: checks if the sender is allowed.
pub struct Permission;

#[async_trait]
impl Middleware for Permission {
    fn name(&self) -> &'static str { "permission" }

    async fn process(&self, ctx: PipelineContext) -> Result<PipelineContext, PipelineError> {
        // Phase 4: check allowlist for sender
        // If not allowed, short_circuit to skip AI/Formatter/Outbox
        // For now, all messages pass
        tracing::debug!(message_id = %ctx.message.id, "permission: allowed");
        Ok(ctx)
    }
}
