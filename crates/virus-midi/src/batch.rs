//! Pacing of batched outgoing messages (e.g. bank dumps), independent of any clock source.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// How long one byte takes on a DIN MIDI wire: 31 250 baud, 10 bits per byte.
pub const WIRE_TIME_PER_BYTE: Duration = Duration::from_micros(320);

/// How long `len` bytes take on a DIN MIDI wire (a 267-byte dump: ≈85 ms).
pub fn wire_time(len: usize) -> Duration {
    WIRE_TIME_PER_BYTE * u32::try_from(len).unwrap_or(u32::MAX)
}

/// Default gap for bank and Multi bank sends: milliseconds of silence after each dump has left
/// the wire (SPEC §3.7). With ≈85 ms of wire time per dump that is ≈385 ms start-to-start, a bit
/// slower than the Virus's own ≈339 ms, so a 128-dump bank takes ≈49 s. Used by the app and as
/// the CLI's `put-bank --gap-ms` default.
pub const DUMP_GAP_MS: u64 = 300;

/// How a batch ended: `sent` messages went out, `dropped` didn't (cancelled, or a send failed;
/// the failed message counts as dropped).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BatchSummary {
    pub sent: usize,
    pub dropped: usize,
}

/// FIFO of outgoing messages, each followed by a minimum gap of silence before the next one.
///
/// The caller takes a due message with [`pop_due`](Self::pop_due), sends it, then reports
/// [`sent`](Self::sent) or [`failed`](Self::failed). The gap is counted from the moment the
/// message has left the wire: the time `send` returned plus the message's wire time, so it holds
/// whether or not the OS send blocks until the bytes are out (SPEC §3.7).
#[derive(Debug, Default)]
pub struct BatchQueue {
    queue: VecDeque<(Vec<u8>, Duration)>,
    next_due: Option<Instant>,
    /// Gap of the message taken by `pop_due`, applied when it is reported sent.
    popped_gap: Duration,
    /// A batch was pushed and has not been reported finished.
    active: bool,
    sent: usize,
}

impl BatchQueue {
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends `messages`; `gap` of silence is kept after each of them. Appending to a running
    /// batch extends it: one summary covers both.
    pub fn push(&mut self, messages: Vec<Vec<u8>>, gap: Duration) {
        if messages.is_empty() {
            return;
        }
        self.active = true;
        self.queue.extend(messages.into_iter().map(|m| (m, gap)));
    }

    /// Drops all pending messages. Returns the batch's summary, or `None` if no batch was running.
    /// The gap after the last sent message is still honoured.
    pub fn clear(&mut self) -> Option<BatchSummary> {
        if !self.active {
            return None;
        }
        let summary = BatchSummary { sent: self.sent, dropped: self.queue.len() };
        self.queue.clear();
        self.active = false;
        self.sent = 0;
        Some(summary)
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

    /// Takes the next message if it is due at `now`. Report the outcome with `sent` or `failed`.
    pub fn pop_due(&mut self, now: Instant) -> Option<Vec<u8>> {
        if self.time_until_next(now)? > Duration::ZERO {
            return None;
        }
        let (message, gap) = self.queue.pop_front()?;
        self.popped_gap = gap;
        Some(message)
    }

    /// The popped message of `len` bytes was handed to the port; `now` is when `send` returned.
    /// The next message is due after its wire time plus its gap. Returns the summary when that
    /// was the batch's last message.
    pub fn sent(&mut self, now: Instant, len: usize) -> Option<BatchSummary> {
        self.next_due = Some(now + wire_time(len) + self.popped_gap);
        self.sent += 1;
        if self.queue.is_empty() {
            return self.clear();
        }
        None
    }

    /// Sending the popped message failed: the rest of the batch is dropped.
    pub fn failed(&mut self) -> BatchSummary {
        let summary = self.clear().unwrap_or_default();
        BatchSummary { dropped: summary.dropped + 1, ..summary }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GAP: Duration = Duration::from_millis(20);

    #[test]
    fn wire_time_of_a_single_dump_is_about_85_ms() {
        assert_eq!(wire_time(267), Duration::from_micros(85_440));
        assert_eq!(wire_time(0), Duration::ZERO);
    }

    #[test]
    fn the_first_message_is_due_at_once() {
        let t0 = Instant::now();
        let mut q = BatchQueue::new();
        assert_eq!(q.time_until_next(t0), None);
        q.push(vec![vec![1], vec![2]], GAP);
        assert_eq!(q.time_until_next(t0), Some(Duration::ZERO));
        assert_eq!(q.pop_due(t0), Some(vec![1]));
    }

    #[test]
    fn gap_is_silence_after_the_message_has_left_the_wire() {
        let t0 = Instant::now();
        let gap = Duration::from_millis(300);
        let mut q = BatchQueue::new();
        q.push(vec![vec![0; 267], vec![1; 267]], gap);
        let first = q.pop_due(t0).expect("due");
        assert_eq!(q.sent(t0, first.len()), None);

        let next_due = gap + Duration::from_micros(85_440);
        assert_eq!(q.time_until_next(t0), Some(next_due));
        assert_eq!(q.pop_due(t0 + next_due - Duration::from_micros(1)), None);
        assert_eq!(q.pop_due(t0 + next_due), Some(vec![1; 267]));
    }

    #[test]
    fn gap_is_counted_from_when_a_blocking_send_returned() {
        let t0 = Instant::now();
        let mut q = BatchQueue::new();
        q.push(vec![vec![1, 2, 3], vec![4]], GAP);
        assert_eq!(q.pop_due(t0), Some(vec![1, 2, 3]));
        let returned = t0 + Duration::from_millis(50);
        q.sent(returned, 3);
        assert_eq!(q.time_until_next(returned), Some(GAP + wire_time(3)));
    }

    #[test]
    fn nothing_is_due_until_the_popped_message_is_reported() {
        let t0 = Instant::now();
        let mut q = BatchQueue::new();
        q.push(vec![vec![1], vec![2]], GAP);
        assert_eq!(q.pop_due(t0), Some(vec![1]));
        q.sent(t0, 1);
        assert_eq!(q.pop_due(t0), None);
    }

    #[test]
    fn the_last_sent_message_finishes_the_batch() {
        let t0 = Instant::now();
        let mut q = BatchQueue::new();
        q.push(vec![vec![1], vec![2]], Duration::ZERO);
        q.pop_due(t0);
        assert_eq!(q.sent(t0, 1), None);
        let later = t0 + wire_time(1);
        q.pop_due(later);
        assert_eq!(q.sent(later, 1), Some(BatchSummary { sent: 2, dropped: 0 }));
        assert!(q.is_empty());
        assert_eq!(q.clear(), None);
    }

    #[test]
    fn a_failed_send_drops_the_rest_and_counts_itself_as_dropped() {
        let t0 = Instant::now();
        let mut q = BatchQueue::new();
        q.push(vec![vec![1], vec![2], vec![3]], Duration::ZERO);
        q.pop_due(t0);
        q.sent(t0, 1);
        q.pop_due(t0 + wire_time(1));
        assert_eq!(q.failed(), BatchSummary { sent: 1, dropped: 2 });
        assert!(q.is_empty());
    }

    #[test]
    fn clearing_reports_the_batch_once_and_keeps_the_gap() {
        let t0 = Instant::now();
        let mut q = BatchQueue::new();
        q.push(vec![vec![1], vec![2]], GAP);
        q.push(vec![vec![3]], Duration::ZERO);
        q.pop_due(t0);
        q.sent(t0, 1);
        assert_eq!(q.clear(), Some(BatchSummary { sent: 1, dropped: 2 }));
        assert_eq!(q.clear(), None);

        q.push(vec![vec![4]], Duration::ZERO);
        assert_eq!(q.pop_due(t0), None);
        assert_eq!(q.pop_due(t0 + GAP + wire_time(1)), Some(vec![4]));
    }

    #[test]
    fn an_empty_push_starts_no_batch() {
        let mut q = BatchQueue::new();
        q.push(Vec::new(), GAP);
        assert_eq!(q.clear(), None);
    }
}
