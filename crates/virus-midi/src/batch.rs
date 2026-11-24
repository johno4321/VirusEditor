//! Pacing of batched outgoing messages (e.g. bank dumps), independent of any clock source.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// FIFO of outgoing messages, each followed by a minimum gap before the next one is due.
#[derive(Debug, Default)]
pub struct BatchQueue {
    queue: VecDeque<(Vec<u8>, Duration)>,
    next_due: Option<Instant>,
}

impl BatchQueue {
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends `messages`; `gap` is waited after each of them.
    pub fn push(&mut self, messages: Vec<Vec<u8>>, gap: Duration) {
        self.queue.extend(messages.into_iter().map(|m| (m, gap)));
    }

    /// Drops all pending messages and returns how many were dropped.
    /// The gap after the last sent message is still honoured.
    pub fn clear(&mut self) -> usize {
        let dropped = self.queue.len();
        self.queue.clear();
        dropped
    }

    pub fn len(&self) -> usize {
        self.queue.len()
    }

    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    /// Time until the next message is due: `None` when empty, zero when due now.
    pub fn time_until_next(&self, now: Instant) -> Option<Duration> {
        if self.queue.is_empty() {
            return None;
        }
        Some(
            self.next_due
                .map_or(Duration::ZERO, |due| due.saturating_duration_since(now)),
        )
    }

    /// Takes the next message if it is due at `now`.
    pub fn pop_due(&mut self, now: Instant) -> Option<Vec<u8>> {
        if self.time_until_next(now)? > Duration::ZERO {
            return None;
        }
        let (message, gap) = self.queue.pop_front()?;
        self.next_due = Some(now + gap);
        Some(message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GAP: Duration = Duration::from_millis(20);

    #[test]
    fn paces_messages_by_gap() {
        let t0 = Instant::now();
        let mut q = BatchQueue::new();
        assert_eq!(q.time_until_next(t0), None);
        q.push(vec![vec![1], vec![2]], GAP);

        assert_eq!(q.time_until_next(t0), Some(Duration::ZERO));
        assert_eq!(q.pop_due(t0), Some(vec![1]));
        assert_eq!(q.time_until_next(t0), Some(GAP));
        assert_eq!(q.pop_due(t0 + GAP / 2), None);
        assert_eq!(q.time_until_next(t0 + GAP / 2), Some(GAP / 2));
        assert_eq!(q.pop_due(t0 + GAP), Some(vec![2]));
        assert!(q.is_empty());
        assert_eq!(q.time_until_next(t0 + GAP), None);
    }

    #[test]
    fn appends_batches_and_keeps_gap_after_clear() {
        let t0 = Instant::now();
        let mut q = BatchQueue::new();
        q.push(vec![vec![1], vec![2]], GAP);
        q.push(vec![vec![3]], Duration::ZERO);
        assert_eq!(q.pop_due(t0), Some(vec![1]));
        assert_eq!(q.clear(), 2);

        q.push(vec![vec![4]], Duration::ZERO);
        assert_eq!(q.pop_due(t0), None);
        assert_eq!(q.pop_due(t0 + GAP), Some(vec![4]));
    }
}
