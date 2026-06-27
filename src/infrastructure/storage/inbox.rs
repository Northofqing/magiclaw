use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InboxEntry {
    pub id: String,
    pub channel: String,
    pub conversation_id: String,
    pub payload: String,
    pub status: InboxStatus,
    pub created_at: i64,
    pub updated_at: i64,
}

impl InboxEntry {
    pub fn new(
        id: impl Into<String>,
        channel: impl Into<String>,
        conversation_id: impl Into<String>,
        payload: impl Into<String>,
        now_ts: i64,
    ) -> Self {
        Self {
            id: id.into(),
            channel: channel.into(),
            conversation_id: conversation_id.into(),
            payload: payload.into(),
            status: InboxStatus::Pending,
            created_at: now_ts,
            updated_at: now_ts,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InboxStatus {
    Pending,
    Processing,
    Processed,
}

impl InboxStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Processing => "processing",
            Self::Processed => "processed",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "pending" => Some(Self::Pending),
            "processing" => Some(Self::Processing),
            "processed" => Some(Self::Processed),
            _ => None,
        }
    }
}
