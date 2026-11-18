//! The timers the core asked for with `UiEffect::StartTimer`.

use std::time::Instant;

/// Pending timers by deadline. The core ignores timers it no longer waits
/// for, so starting one never cancels another.
pub(crate) struct Timers<Id> {
    /// Ordered by deadline; equal deadlines in the order they were started.
    pending: Vec<(Instant, Id)>,
}

impl<Id> Timers<Id> {
    pub(crate) fn new() -> Timers<Id> {
        Timers {
            pending: Vec::new(),
        }
    }

    pub(crate) fn start(&mut self, id: Id, deadline: Instant) {
        let at = self.pending.partition_point(|(due, _)| *due <= deadline);
        self.pending.insert(at, (deadline, id));
    }

    pub(crate) fn next_deadline(&self) -> Option<Instant> {
        self.pending.first().map(|(due, _)| *due)
    }

    /// Removes the timers due at `now` and yields them in deadline order.
    pub(crate) fn expired(&mut self, now: Instant) -> impl Iterator<Item = Id> {
        let due = self.pending.partition_point(|(due, _)| *due <= now);
        self.pending.drain(..due).map(|(_, id)| id)
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;

    #[test]
    fn the_next_deadline_is_the_earliest() {
        let start = Instant::now();
        let mut timers = Timers::new();
        assert_eq!(timers.next_deadline(), None);
        timers.start("late", start + Duration::from_millis(300));
        timers.start("early", start + Duration::from_millis(100));
        timers.start("middle", start + Duration::from_millis(200));
        assert_eq!(
            timers.next_deadline(),
            Some(start + Duration::from_millis(100))
        );
    }

    #[test]
    fn expired_returns_only_due_timers_in_deadline_order() {
        let start = Instant::now();
        let mut timers = Timers::new();
        timers.start("late", start + Duration::from_millis(300));
        timers.start("middle", start + Duration::from_millis(200));
        timers.start("early", start + Duration::from_millis(100));
        let mut expired = |now| timers.expired(now).collect::<Vec<_>>();

        assert_eq!(expired(start), Vec::<&str>::new());
        assert_eq!(
            expired(start + Duration::from_millis(200)),
            ["early", "middle"]
        );
        assert_eq!(expired(start + Duration::from_secs(1)), ["late"]);
        assert_eq!(timers.next_deadline(), None);
    }
}
