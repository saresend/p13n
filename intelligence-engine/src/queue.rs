//! Per-user handoff from the worker thread (producer) to SSE streams (consumers).
//!
//! Every open stream for the user receives each suggestion. With no stream open, suggestions
//! wait in a bounded buffer (oldest dropped first, since newer ones are more relevant) and go to
//! the next stream that connects. Fan-out rather than one-consumer-per-item, because a client
//! that has gone away keeps its stream open until the next write fails, and it must not
//! swallow suggestions meant for the client that replaced it.

use crate::action::Suggestion;
use std::collections::VecDeque;
use std::sync::Mutex;
use tokio::sync::broadcast;

pub struct SuggestionQueue {
    buffer: Mutex<VecDeque<Suggestion>>,
    live: broadcast::Sender<Suggestion>,
    capacity: usize,
}

/// A stream's view of the queue: what was waiting when it connected, then live suggestions.
pub struct Subscription {
    pub backlog: VecDeque<Suggestion>,
    pub live: broadcast::Receiver<Suggestion>,
}

impl SuggestionQueue {
    pub fn new(capacity: usize) -> Self {
        let capacity = capacity.max(1);
        Self {
            buffer: Mutex::new(VecDeque::new()),
            live: broadcast::channel(capacity).0,
            capacity,
        }
    }

    /// Callable from any thread, never blocks on consumers.
    pub fn push(&self, suggestion: Suggestion) {
        let mut buffer = self.buffer.lock().unwrap();
        if self.live.receiver_count() > 0 {
            let _ = self.live.send(suggestion);
            return;
        }
        if buffer.len() == self.capacity {
            buffer.pop_front();
            log::warn!(
                "no stream for {} and its buffer is full; dropped the oldest suggestion",
                suggestion.user_id
            );
        }
        buffer.push_back(suggestion);
    }

    /// Takes everything buffered and starts receiving live suggestions. Done under the buffer
    /// lock so a concurrent `push` lands in exactly one of the two.
    pub fn subscribe(&self) -> Subscription {
        let mut buffer = self.buffer.lock().unwrap();
        Subscription {
            live: self.live.subscribe(),
            backlog: std::mem::take(&mut *buffer),
        }
    }

    pub fn buffered(&self) -> usize {
        self.buffer.lock().unwrap().len()
    }
}

impl Subscription {
    /// The next suggestion for this stream. Cancel-safe.
    pub async fn next(&mut self) -> Option<Suggestion> {
        if let Some(suggestion) = self.backlog.pop_front() {
            return Some(suggestion);
        }
        loop {
            match self.live.recv().await {
                Ok(suggestion) => return Some(suggestion),
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    log::warn!("a slow stream skipped {skipped} suggestions");
                }
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::Duration;

    fn suggestion(n: f32) -> Suggestion {
        Suggestion {
            confidence: n,
            user_id: "u".into(),
            actions: vec![],
        }
    }

    #[actix_rt::test]
    async fn buffers_without_streams_and_drops_oldest_when_full() {
        let queue = SuggestionQueue::new(2);
        for n in [0.1, 0.2, 0.3] {
            queue.push(suggestion(n));
        }
        let mut sub = queue.subscribe();
        assert_eq!(sub.next().await.unwrap().confidence, 0.2);
        assert_eq!(sub.next().await.unwrap().confidence, 0.3);
        assert_eq!(queue.buffered(), 0, "the backlog went to the first stream");
    }

    #[actix_rt::test]
    async fn every_open_stream_gets_each_suggestion() {
        let queue = SuggestionQueue::new(4);
        // A client that has gone away but whose stream isn't closed yet.
        let _stale = queue.subscribe();
        let mut fresh = queue.subscribe();
        queue.push(suggestion(0.5));
        let got = tokio::time::timeout(Duration::from_secs(1), fresh.next()).await;
        assert_eq!(got.unwrap().unwrap().confidence, 0.5);
        assert_eq!(queue.buffered(), 0);
    }

    #[actix_rt::test]
    async fn buffers_again_once_streams_close() {
        let queue = SuggestionQueue::new(4);
        drop(queue.subscribe());
        queue.push(suggestion(0.7));
        assert_eq!(queue.buffered(), 1);
    }

    #[actix_rt::test]
    async fn wakes_a_waiting_stream_from_another_thread() {
        let queue = Arc::new(SuggestionQueue::new(4));
        let mut sub = queue.subscribe();
        let producer = Arc::clone(&queue);
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            producer.push(suggestion(0.5));
        });
        let got = tokio::time::timeout(Duration::from_secs(2), sub.next())
            .await
            .expect("timed out");
        assert_eq!(got.unwrap().confidence, 0.5);
    }
}
