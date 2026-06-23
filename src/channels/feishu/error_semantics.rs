// src/channels/feishu/error_semantics.rs
//
// Task 4: Outbox Failure Semantics
// Normalize Feishu API rejection codes to retry vs terminal classification
// 
// Purpose: Ensure send failures are correctly categorized so outbox worker
// knows whether to retry or move to dead_letter.
//
// Phase A red line: Send state machine pending -> sending -> sent,
// failure -> retrying, threshold exceeded -> dead_letter

/// Classification of Feishu API errors for outbox state transition
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeishuErrorSemantics {
    /// Transient error: should retry (e.g., 429 rate limit, 500 server error, timeout)
    Retryable,
    
    /// Terminal error: should move to dead_letter without retry
    /// (e.g., 403 permission denied, 404 not found, 400 invalid param, 401 auth failed)
    Terminal,
    
    /// Unknown error: default to retryable (fail-safe)
    Unknown,
}

impl FeishuErrorSemantics {
    /// Classify HTTP status code to retry vs terminal
    pub fn from_http_status(status_code: u16) -> Self {
        match status_code {
            // 4xx Client Errors
            400 => FeishuErrorSemantics::Terminal, // Bad Request - client error
            401 | 403 => FeishuErrorSemantics::Terminal, // Auth failed / Permission denied
            404 => FeishuErrorSemantics::Terminal, // Not found - receiver doesn't exist
            405 => FeishuErrorSemantics::Terminal, // Method not allowed
            
            // 429 Too Many Requests - Retryable (rate limit)
            429 => FeishuErrorSemantics::Retryable,
            
            // 5xx Server Errors - Retryable
            500 | 502 | 503 | 504 => FeishuErrorSemantics::Retryable,
            
            // Timeout-like errors (not HTTP but connection errors) - Retryable
            // 408 Request Timeout
            408 => FeishuErrorSemantics::Retryable,
            
            // Unknown status - fail-safe to retryable
            _ => FeishuErrorSemantics::Unknown,
        }
    }
    
    /// Classify based on Feishu OpenAPI error code
    /// Reference: https://open.feishu.cn/document/server-docs/im-v1/message/create
    pub fn from_feishu_error_code(code: i64) -> Self {
        match code {
            // Success codes
            0 => FeishuErrorSemantics::Terminal, // Not an error; shouldn't call this
            
            // Common errors
            1001 => FeishuErrorSemantics::Terminal, // Invalid parameter
            1002 => FeishuErrorSemantics::Terminal, // Authentication required
            1003 => FeishuErrorSemantics::Terminal, // No permission
            1004 => FeishuErrorSemantics::Terminal, // Invalid receiver_id_type
            
            // Resource not found
            2001 => FeishuErrorSemantics::Terminal, // Chat not found
            2002 => FeishuErrorSemantics::Terminal, // User not found
            
            // Rate limiting (retryable with backoff)
            1008 => FeishuErrorSemantics::Retryable,
            
            // Service errors (retryable)
            5001 => FeishuErrorSemantics::Retryable, // Service internal error
            5002 => FeishuErrorSemantics::Retryable, // Service unavailable
            
            // Timeout errors (retryable)
            1010 => FeishuErrorSemantics::Retryable,
            
            // Unknown error - fail-safe to retryable
            _ => FeishuErrorSemantics::Unknown,
        }
    }
    
    /// Classify error message from response body
    pub fn from_error_message(msg: &str) -> Self {
        let lower = msg.to_lowercase();
        
        // Permission/Auth errors (terminal)
        if lower.contains("permission") || lower.contains("forbidden") || lower.contains("denied") {
            return FeishuErrorSemantics::Terminal;
        }
        
        // Not found errors (terminal)
        if lower.contains("not found") || lower.contains("does not exist") {
            return FeishuErrorSemantics::Terminal;
        }
        
        // Invalid/malformed errors (terminal)
        if lower.contains("invalid") || lower.contains("malformed") || lower.contains("bad request") {
            return FeishuErrorSemantics::Terminal;
        }
        
        // Rate limit (retryable)
        if lower.contains("rate limit") || lower.contains("too many") {
            return FeishuErrorSemantics::Retryable;
        }
        
        // Server errors (retryable)
        if lower.contains("server error") || lower.contains("unavailable") || lower.contains("timeout") {
            return FeishuErrorSemantics::Retryable;
        }
        
        FeishuErrorSemantics::Unknown
    }
    
    /// Determine if error should trigger immediate retry or move to dead_letter
    pub fn should_retry(&self) -> bool {
        matches!(self, FeishuErrorSemantics::Retryable | FeishuErrorSemantics::Unknown)
    }
    
    /// Determine if error is terminal (should move to dead_letter)
    pub fn is_terminal(&self) -> bool {
        matches!(self, FeishuErrorSemantics::Terminal)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_http_status_classification() {
        // Terminal errors
        assert!(FeishuErrorSemantics::from_http_status(400).is_terminal());
        assert!(FeishuErrorSemantics::from_http_status(401).is_terminal());
        assert!(FeishuErrorSemantics::from_http_status(403).is_terminal());
        assert!(FeishuErrorSemantics::from_http_status(404).is_terminal());
        
        // Retryable errors
        assert!(FeishuErrorSemantics::from_http_status(429).should_retry());
        assert!(FeishuErrorSemantics::from_http_status(500).should_retry());
        assert!(FeishuErrorSemantics::from_http_status(502).should_retry());
        assert!(FeishuErrorSemantics::from_http_status(503).should_retry());
        assert!(FeishuErrorSemantics::from_http_status(504).should_retry());
    }
    
    #[test]
    fn test_feishu_error_code_classification() {
        // Terminal codes
        assert!(FeishuErrorSemantics::from_feishu_error_code(1001).is_terminal());
        assert!(FeishuErrorSemantics::from_feishu_error_code(1003).is_terminal());
        assert!(FeishuErrorSemantics::from_feishu_error_code(2001).is_terminal());
        
        // Retryable codes
        assert!(FeishuErrorSemantics::from_feishu_error_code(1008).should_retry());
        assert!(FeishuErrorSemantics::from_feishu_error_code(5001).should_retry());
        assert!(FeishuErrorSemantics::from_feishu_error_code(5002).should_retry());
    }
    
    #[test]
    fn test_error_message_classification() {
        // Terminal messages
        assert!(FeishuErrorSemantics::from_error_message("permission denied").is_terminal());
        assert!(FeishuErrorSemantics::from_error_message("User not found").is_terminal());
        assert!(FeishuErrorSemantics::from_error_message("Invalid parameter").is_terminal());
        
        // Retryable messages
        assert!(FeishuErrorSemantics::from_error_message("rate limit exceeded").should_retry());
        assert!(FeishuErrorSemantics::from_error_message("server error").should_retry());
        assert!(FeishuErrorSemantics::from_error_message("service unavailable").should_retry());
    }
}
