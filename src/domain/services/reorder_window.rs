use std::collections::BTreeMap;

use crate::domain::entities::message::{AuditMark, Message};

/// ReorderWindow buffers messages and releases them in sorted order
/// after a configurable time window has elapsed.
///
/// Uses wall-clock based cutoff (`now - window_ms`) rather than
/// data-driven cutoff (`newest_sort_key - window_ms`) to avoid
/// premature release when high sequence numbers arrive.
#[derive(Clone, Debug)]
pub struct ReorderWindow {
    /// Messages buffered by their sort key (sequence or timestamp).
    buffer: BTreeMap<i64, Message>,
    /// How long to wait before flushing a message (ms).
    window_ms: u64,
    /// Timestamp of the most recent message arrival (ms), used as wall-clock anchor.
    latest_arrival_ms: i64,
}

impl ReorderWindow {
    pub fn new(window_ms: u64) -> Self {
        Self {
            buffer: BTreeMap::new(),
            window_ms,
            latest_arrival_ms: 0,
        }
    }

    /// Insert a message with an arrival timestamp. Returns messages whose
    /// sort key falls below the wall-clock cutoff (`arrival_ms - window_ms`).
    pub fn insert(&mut self, msg: Message, arrival_ms: i64) -> Vec<Message> {
        let key = msg.sort_key();
        self.buffer.insert(key, msg);
        self.latest_arrival_ms = self.latest_arrival_ms.max(arrival_ms);

        let cutoff = self.latest_arrival_ms.saturating_sub(self.window_ms as i64);

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
    fn inserts_and_flushes_by_wall_clock() {
        let mut window = ReorderWindow::new(200);

        // All messages arrive at the same time.
        // m1 (seq=100) and m3 (seq=300) are both below cutoff of 1000-200=800, so both flush.
        window.insert(make_msg("m3", 300), 1000);
        window.insert(make_msg("m1", 100), 1000);

        // With arrival_ms=1000 and window_ms=200, cutoff=800.
        // Both m1 (seq=100) and m3 (seq=300) are <= 800 in sort key, so both flush.
        // But they were inserted in two steps. The first insert flushed nothing.
        // The second insert triggers flush of both messages.
        let ready = window.insert(make_msg("m5", 500), 1000);
        // m5 is below 800, so it's in ready. m3 and m1 were already flushed by the 2nd insert.
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].id, "m5");

        // All messages flushed
        assert_eq!(window.len(), 0);
    }

    #[test]
    fn future_messages_held_until_window_passes() {
        let mut window = ReorderWindow::new(200);

        // Message arrives at t=0 with seq=100
        let ready = window.insert(make_msg("m1", 100), 0);
        assert!(ready.is_empty()); // 0 - 200 = -200, seq 100 > -200, held

        // Time passes, new arrival at t=500. Cutoff = 500-200 = 300. Seq 100 < 300, flushed.
        let ready = window.insert(make_msg("m2", 400), 500);
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].id, "m1");
    }

    #[test]
    fn flush_all_drains_buffer() {
        let mut window = ReorderWindow::new(200);
        window.insert(make_msg("m1", 100), 0);
        window.insert(make_msg("m2", 200), 0);

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
