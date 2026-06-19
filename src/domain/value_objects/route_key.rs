use serde::{Deserialize, Serialize};

#[derive(Hash, Eq, PartialEq, Clone, Debug, Serialize, Deserialize)]
pub struct RouteKey {
    pub channel: ChannelId,
    pub conversation_id: String,
    pub peer_id: String,
    pub conversation_type: ConversationType,
}

impl RouteKey {
    pub fn new(
        channel: ChannelId,
        conversation_id: impl Into<String>,
        peer_id: impl Into<String>,
        conversation_type: ConversationType,
    ) -> Self {
        Self {
            channel,
            conversation_id: conversation_id.into(),
            peer_id: peer_id.into(),
            conversation_type,
        }
    }
}

#[derive(Hash, Eq, PartialEq, Clone, Debug, Serialize, Deserialize)]
pub struct ChannelId(pub String);

impl ChannelId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ChannelId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Hash, Eq, PartialEq, Clone, Debug, Serialize, Deserialize)]
pub enum ConversationType {
    Direct,
    Group,
    Thread,
    BotSession,
}

impl std::fmt::Display for ConversationType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConversationType::Direct => write!(f, "direct"),
            ConversationType::Group => write!(f, "group"),
            ConversationType::Thread => write!(f, "thread"),
            ConversationType::BotSession => write!(f, "bot_session"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_key_equality() {
        let k1 = RouteKey::new(
            ChannelId::new("wechat"),
            "conv_001",
            "user_a",
            ConversationType::Direct,
        );
        let k2 = RouteKey::new(
            ChannelId::new("wechat"),
            "conv_001",
            "user_a",
            ConversationType::Direct,
        );
        assert_eq!(k1, k2);

        let k3 = RouteKey::new(
            ChannelId::new("wechat"),
            "conv_002",
            "user_a",
            ConversationType::Direct,
        );
        assert_ne!(k1, k3);
    }

    #[test]
    fn route_key_hashing() {
        use std::collections::HashSet;
        let mut set = HashSet::new();
        set.insert(RouteKey::new(
            ChannelId::new("wechat"),
            "conv_001",
            "user_a",
            ConversationType::Direct,
        ));
        set.insert(RouteKey::new(
            ChannelId::new("wechat"),
            "conv_002",
            "user_a",
            ConversationType::Direct,
        ));
        assert_eq!(set.len(), 2);

        // Same key should not add
        set.insert(RouteKey::new(
            ChannelId::new("wechat"),
            "conv_001",
            "user_a",
            ConversationType::Direct,
        ));
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn conversation_type_display() {
        assert_eq!(ConversationType::Direct.to_string(), "direct");
        assert_eq!(ConversationType::Group.to_string(), "group");
        assert_eq!(ConversationType::Thread.to_string(), "thread");
        assert_eq!(ConversationType::BotSession.to_string(), "bot_session");
    }
}
