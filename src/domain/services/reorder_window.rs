use std::collections::BTreeMap;

use crate::domain::entities::message::{AuditMark, Message};

/// ReorderWindow buffers messages and releases them in sorted order
/// after a configurable time window has elapsed.
#[derive(Clone, Debug)]
pub struct ReorderWindow {
    /// Messages buffered by their sort key (sequence or timestamp).
    buffer: BTreeMap<i64, Message>,
    /// How long to wait before flushing a message (ms).
    window_ms: u64,
}

impl ReorderWindow {
    pub fn new(window_ms: u64) -> Self {
        Self {
            buffer: BTreeMap::new(),
            window_ms,
        }
    }

    /// Insert a message into the window. Returns any messages that are
    /// ready to be flushed (i.e., their sort key is beyond the cutoff).
    pub fn insert(&mut self, msg: Message) -> Vec<Message> {
        let key = msg.sort_key();
        self.buffer.insert(key, msg);

        if self.buffer.is_empty() {
            return vec![];
        }

        // The cutoff is `now - window_ms`. Since we don't have access to
        // the current time, the cutoff is derived from the newest key
        // minus the window.
        let newest_key = *self.buffer.last_key_value().unwrap().0;
        let cutoff = newest_key.saturating_sub(self.window_ms as i64);

        let ready: Vec<Message> = self
            .buffer
            .range(..=cutoff)
            .map(|(_, m)| m.clone())
            .collect();

        for m in &ready {
            self.buffer.remove(&m.sort_key());
        }

        ready
    }

    /// Handle a late-arriving message (sort key behind cutoff).
    /// Returns the message with a LateArrival audit mark.
    pub fn handle_late(&mut self, mut msg: Message, delay_ms: u64) -> Message {
        msg.audit_mark = Some(AuditMark::LateArrival { delay_ms });
        msg
    }

    /// Flush all remaining messages in the buffer (used during GC shutdown).
    pub fn flush_all(&mut self) -> Vec<Message> {
        let drained: Vec<Message> = self.buffer.values().cloned().collect();
        self.buffer.clear();
        drained
    }

    /// Number of messages currently buffered.
    pub fn len(&self) -> usize {
        self.buffer.len()
    }

    /// Whether the buffer is empty.
    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }
}

impl Default for ReorderWindow {
    fn default() -> Self {
        Self::new(200)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::entities::message::{Direction, MessageContent};
    use crate::domain::value_objects::route_key::{ChannelId, ConversationType, RouteKey};

    fn make_route_key() -> RouteKey {
        RouteKey::new(
            ChannelId::new("wechat"),
            "conv_001",
            "user_a",
            ConversationType::Direct,
        )
    }

    fn make_msg(id: &str, seq: i64) -> Message {
        Message {
            id: id.into(),
            route_key: make_route_key(),
            sequence: Some(seq),
            timestamp_ms: seq * 100,
            direction: Direction::Inbound,
            content: MessageContent::Text(format!("msg_{id}")),
            audit_mark: None,
        }
    }

    #[test]
    fn inserts_and_flushes_in_order() {
        let mut window = ReorderWindow::new(200);

        // Insert m3 (seq=300): newest=300, cutoff=100, nothing flushed
        let ready = window.insert(make_msg("m3", 300));
        assert!(ready.is_empty());

        // Insert m1 (seq=100): newest=300, cutoff=100, m1 <= 100 flushed immediately
        let ready = window.insert(make_msg("m1", 100));
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].id, "m1");

        // Insert m5 (seq=500): newest=500, cutoff=300, m3 <= 300 flushed
        let ready = window.insert(make_msg("m5", 500));
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].id, "m3");

        // m5 (seq=500) remains, waiting for a future message to push cutoff past 500
        assert_eq!(window.len(), 1);
    }

    #[test]
    fn flush_all_drains_buffer() {
        let mut window = ReorderWindow::new(200);
        window.insert(make_msg("m1", 100));
        window.insert(make_msg("m2", 200));

        let flushed = window.flush_all();
        assert_eq!(flushed.len(), 2);
        assert!(window.is_empty());
    }

    #[test]
    fn handle_late_adds_audit_mark() {
        let mut window = ReorderWindow::new(200);
        let msg = make_msg("late", 50);
        let result = window.handle_late(msg, 500);
        assert!(matches!(
            result.audit_mark,
            Some(AuditMark::LateArrival { delay_ms: 500 })
        ));
    }
}
