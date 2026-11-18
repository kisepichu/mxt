//! The keymap resolver: turns keys into bindings to run, keys to release and
//! timers to start, like nvim's mappings with `timeoutlen`. It reads no
//! clock: the tui adapter runs the timers and reports when they expire.

use std::mem;

use crate::action::Binding;
use crate::keymap::EffectiveMap;
use crate::keys::Key;

/// A timer the resolver asked for. Only the latest one is current.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TimerId(u64);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Resolved {
    /// Run the binding; `count` is the count prefix if one was typed.
    Run {
        binding: Binding,
        count: Option<u32>,
    },
    /// Keys the keymap did not consume, for the context's sink.
    Release(Vec<Key>),
    /// Wait for more keys and ask the tui adapter for a timer.
    StartTimer { id: TimerId, after_ms: u64 },
}

/// Larger count prefixes are cut to this.
const MAX_COUNT: u32 = 9999;

/// The pending key sequence and count prefix (design, "Keymap resolver").
#[derive(Clone, Debug, Default)]
pub struct Resolver {
    /// Keys typed so far, a prefix of a longer binding.
    keys: Vec<Key>,
    count: Option<u32>,
    /// The timer for `keys`; older timers are stale.
    timer: Option<TimerId>,
    next_timer: u64,
}

impl Resolver {
    pub fn new() -> Resolver {
        Resolver::default()
    }

    /// Resolves `key` against `map`. `counts` lets digits start a count
    /// prefix; `timeoutlen_ms` is how long a prefix of a longer binding waits.
    ///
    /// When `key` makes the pending sequence match nothing, the pending keys
    /// run if they are a binding and are released otherwise, and `key` is
    /// resolved afresh.
    pub fn feed(
        &mut self,
        map: &EffectiveMap,
        counts: bool,
        timeoutlen_ms: u64,
        key: Key,
    ) -> Vec<Resolved> {
        let mut out = Vec::new();
        self.step(map, counts, timeoutlen_ms, key, &mut out);
        out
    }

    /// The timer `id` expired: the pending keys run if they are a binding and
    /// are released otherwise. Stale or unknown ids return nothing.
    pub fn timeout(&mut self, map: &EffectiveMap, id: TimerId) -> Option<Resolved> {
        (self.timer == Some(id)).then(|| self.finish(map))
    }

    /// Drops pending keys and the count (when the input context changes).
    pub fn reset(&mut self) {
        self.keys.clear();
        self.count = None;
        self.timer = None;
    }

    /// Pending keys, for the status line.
    pub fn pending(&self) -> &[Key] {
        &self.keys
    }

    pub fn count(&self) -> Option<u32> {
        self.count
    }

    fn step(
        &mut self,
        map: &EffectiveMap,
        counts: bool,
        timeoutlen_ms: u64,
        key: Key,
        out: &mut Vec<Resolved>,
    ) {
        if counts && self.keys.is_empty() && self.extend_count(map, key) {
            return;
        }
        self.keys.push(key);
        let found = map.lookup(&self.keys);
        if found.is_prefix {
            let id = TimerId(self.next_timer);
            self.next_timer += 1;
            self.timer = Some(id);
            out.push(Resolved::StartTimer {
                id,
                after_ms: timeoutlen_ms,
            });
        } else if let Some(binding) = found.binding {
            out.push(Resolved::Run {
                binding: binding.clone(),
                count: self.count.take(),
            });
            self.keys.clear();
            self.timer = None;
        } else {
            self.keys.pop();
            if self.keys.is_empty() {
                self.count = None;
                out.push(Resolved::Release(vec![key]));
            } else {
                out.push(self.finish(map));
                // Nothing is pending now, so this does not come back here.
                self.step(map, counts, timeoutlen_ms, key, out);
            }
        }
    }

    /// Adds an unmodified digit to the count: `1` to `9` when no binding
    /// starts with it, and `0` once a count has started, even when bound
    /// (nvim does not map `0` while a count is typed).
    fn extend_count(&mut self, map: &EffectiveMap, key: Key) -> bool {
        let Some(digit) = key.text().and_then(|c| c.to_digit(10)) else {
            return false;
        };
        let extends = match self.count {
            Some(_) if digit == 0 => true,
            _ => {
                let found = map.lookup(&[key]);
                digit != 0 && found.binding.is_none() && !found.is_prefix
            }
        };
        if extends {
            let count = self.count.unwrap_or(0).saturating_mul(10);
            self.count = Some(count.saturating_add(digit).min(MAX_COUNT));
        }
        extends
    }

    /// Ends the pending sequence: runs it if it is a binding, otherwise
    /// releases its keys. Clears the count and the timer.
    fn finish(&mut self, map: &EffectiveMap) -> Resolved {
        let count = self.count.take();
        self.timer = None;
        match map.lookup(&self.keys).binding {
            Some(binding) => {
                let binding = binding.clone();
                self.keys.clear();
                Resolved::Run { binding, count }
            }
            None => Resolved::Release(mem::take(&mut self.keys)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::action::{Action, Binding};
    use crate::keymap::{InputContext, Keymap, TableId};
    use crate::keys::{Key, KeySeq};

    const T: u64 = 1000;

    fn space() -> KeySeq {
        KeySeq::parse("<Space>", &KeySeq::default()).unwrap()
    }
    fn seq(text: &str) -> KeySeq {
        KeySeq::parse(text, &space()).unwrap()
    }
    fn k(text: &str) -> Key {
        seq(text).keys()[0]
    }
    fn run(action: Action, count: Option<u32>) -> Resolved {
        Resolved::Run {
            binding: Binding::single(action),
            count,
        }
    }
    fn release(text: &str) -> Resolved {
        Resolved::Release(seq(text).keys().to_vec())
    }
    /// Feeds keys and returns all outputs, with timer ids erased for comparison.
    fn feed_all(r: &mut Resolver, km: &Keymap, ctx: InputContext, text: &str) -> Vec<Resolved> {
        seq(text)
            .keys()
            .iter()
            .flat_map(|&key| r.feed(km.effective(ctx), ctx.counts(), T, key))
            .map(erase)
            .collect()
    }
    fn erase(r: Resolved) -> Resolved {
        match r {
            Resolved::StartTimer { after_ms, .. } => Resolved::StartTimer {
                id: TimerId(0),
                after_ms,
            },
            other => other,
        }
    }
    fn timer() -> Resolved {
        Resolved::StartTimer {
            id: TimerId(0),
            after_ms: T,
        }
    }
    fn last_timer(outputs: &[Resolved]) -> TimerId {
        outputs
            .iter()
            .rev()
            .find_map(|o| match o {
                Resolved::StartTimer { id, .. } => Some(*id),
                _ => None,
            })
            .expect("a timer")
    }
    fn defaults() -> Keymap {
        Keymap::new(&space(), [])
    }

    #[test]
    fn complete_binding_runs_immediately() {
        let km = defaults();
        let mut r = Resolver::new();
        assert_eq!(
            feed_all(&mut r, &km, InputContext::Timeline, "j"),
            vec![run(Action::CursorDown, None)]
        );
        assert!(r.pending().is_empty());
    }

    #[test]
    fn prefix_waits_then_completes() {
        let km = defaults();
        let mut r = Resolver::new();
        assert_eq!(
            feed_all(&mut r, &km, InputContext::Timeline, "g"),
            vec![timer()]
        );
        assert_eq!(r.pending(), seq("g").keys());
        assert_eq!(
            feed_all(&mut r, &km, InputContext::Timeline, "g"),
            vec![run(Action::CursorFirst, None)]
        );
    }

    #[test]
    fn count_prefix() {
        let km = defaults();
        let mut r = Resolver::new();
        assert_eq!(
            feed_all(&mut r, &km, InputContext::Timeline, "5j"),
            vec![run(Action::CursorDown, Some(5))]
        );
        assert_eq!(
            feed_all(&mut r, &km, InputContext::Timeline, "10k"),
            vec![run(Action::CursorUp, Some(10))]
        );
        // `0` without a count is an ordinary (unbound) key.
        assert_eq!(
            feed_all(&mut r, &km, InputContext::Timeline, "0"),
            vec![release("0")]
        );
        assert_eq!(r.count(), None);
    }

    #[test]
    fn no_counts_in_text_contexts() {
        let km = defaults();
        let mut r = Resolver::new();
        assert_eq!(
            feed_all(&mut r, &km, InputContext::ComposerNormal, "5"),
            vec![release("5")]
        );
    }

    #[test]
    fn bound_digits_are_not_counts() {
        let km = Keymap::new(
            &space(),
            [(
                TableId::Timeline,
                seq("1"),
                Binding::single(Action::FocusRoomList),
            )],
        );
        let mut r = Resolver::new();
        assert_eq!(
            feed_all(&mut r, &km, InputContext::Timeline, "1"),
            vec![run(Action::FocusRoomList, None)]
        );
    }

    #[test]
    fn timeout_releases_an_incomplete_prefix() {
        let km = defaults();
        let mut r = Resolver::new();
        let out = r.feed(km.effective(InputContext::Timeline), true, T, k("g"));
        let id = last_timer(&out);
        assert_eq!(
            r.timeout(km.effective(InputContext::Timeline), id),
            Some(release("g"))
        );
        assert!(r.pending().is_empty());
    }

    #[test]
    fn timeout_runs_a_complete_prefix() {
        let km = Keymap::new(
            &space(),
            [
                (
                    TableId::Timeline,
                    seq("]"),
                    Binding::single(Action::CursorLast),
                ),
                (
                    TableId::Normal,
                    seq("]u"),
                    Binding::single(Action::RoomMarkRead),
                ),
            ],
        );
        let ctx = InputContext::Timeline;
        let mut r = Resolver::new();
        let out = r.feed(km.effective(ctx), true, T, k("]"));
        let id = last_timer(&out);
        assert_eq!(
            r.timeout(km.effective(ctx), id),
            Some(run(Action::CursorLast, None))
        );
    }

    #[test]
    fn stale_timers_are_ignored() {
        let km = defaults();
        let ctx = InputContext::Timeline;
        let mut r = Resolver::new();
        let first = last_timer(&r.feed(km.effective(ctx), true, T, k("g")));
        assert_eq!(
            feed_all(&mut r, &km, ctx, "g"),
            vec![run(Action::CursorFirst, None)]
        );
        assert_eq!(r.timeout(km.effective(ctx), first), None);
    }

    #[test]
    fn mismatch_runs_the_pending_complete_binding_then_resolves_the_new_key() {
        let km = Keymap::new(
            &space(),
            [
                (
                    TableId::Timeline,
                    seq("]"),
                    Binding::single(Action::CursorLast),
                ),
                (
                    TableId::Normal,
                    seq("]u"),
                    Binding::single(Action::RoomMarkRead),
                ),
            ],
        );
        let mut r = Resolver::new();
        assert_eq!(
            feed_all(&mut r, &km, InputContext::Timeline, "]j"),
            vec![
                timer(),
                run(Action::CursorLast, None),
                run(Action::CursorDown, None)
            ]
        );
    }

    #[test]
    fn mismatch_without_binding_releases_pending_keys() {
        let km = defaults();
        let mut r = Resolver::new();
        // composer.insert has <C-x><C-e>; <C-x> then `a` goes to the editor.
        assert_eq!(
            feed_all(&mut r, &km, InputContext::ComposerInsert, "<C-x>a"),
            vec![timer(), release("<C-x>"), release("a")]
        );
    }

    #[test]
    fn new_key_after_a_release_can_start_a_sequence() {
        let km = defaults();
        let mut r = Resolver::new();
        // `gq` is unbound: release `g`, then `q` is unbound too. `gg` after a
        // released `g` still works.
        assert_eq!(
            feed_all(&mut r, &km, InputContext::Timeline, "gqgg"),
            vec![
                timer(),
                release("g"),
                release("q"),
                timer(),
                run(Action::CursorFirst, None)
            ]
        );
    }

    #[test]
    fn count_is_kept_through_a_pending_sequence() {
        let km = defaults();
        let mut r = Resolver::new();
        assert_eq!(
            feed_all(&mut r, &km, InputContext::Timeline, "3gg"),
            vec![timer(), run(Action::CursorFirst, Some(3))]
        );
    }

    #[test]
    fn nop_runs_and_consumes_the_keys() {
        let km = Keymap::new(
            &space(),
            [(TableId::Timeline, seq("dd"), Binding::single(Action::Nop))],
        );
        let mut r = Resolver::new();
        assert_eq!(
            feed_all(&mut r, &km, InputContext::Timeline, "dd"),
            vec![timer(), run(Action::Nop, None)]
        );
    }

    #[test]
    fn reset_drops_pending_keys() {
        let km = defaults();
        let mut r = Resolver::new();
        feed_all(&mut r, &km, InputContext::Timeline, "4g");
        r.reset();
        assert!(r.pending().is_empty());
        assert_eq!(r.count(), None);
    }

    #[test]
    fn a_pending_sequence_runs_only_when_it_is_a_binding_itself() {
        // `]` is a binding and `]a` is only a prefix of `]ab`. The design runs
        // the pending keys only when they are a binding themselves, so `]a`
        // is released on a timeout and on a mismatch.
        let km = Keymap::new(
            &space(),
            [
                (
                    TableId::Timeline,
                    seq("]"),
                    Binding::single(Action::CursorLast),
                ),
                (
                    TableId::Normal,
                    seq("]ab"),
                    Binding::single(Action::RoomMarkRead),
                ),
            ],
        );
        let ctx = InputContext::Timeline;
        let mut r = Resolver::new();
        r.feed(km.effective(ctx), true, T, k("]"));
        let id = last_timer(&r.feed(km.effective(ctx), true, T, k("a")));
        assert_eq!(r.timeout(km.effective(ctx), id), Some(release("]a")));
        assert_eq!(
            feed_all(&mut r, &km, ctx, "]aj"),
            vec![
                timer(),
                timer(),
                release("]a"),
                run(Action::CursorDown, None)
            ]
        );
    }

    #[test]
    fn zero_continues_a_count_even_when_bound() {
        let km = Keymap::new(
            &space(),
            [(
                TableId::Timeline,
                seq("0"),
                Binding::single(Action::CursorFirst),
            )],
        );
        let mut r = Resolver::new();
        let ctx = InputContext::Timeline;
        assert_eq!(
            feed_all(&mut r, &km, ctx, "10j"),
            vec![run(Action::CursorDown, Some(10))]
        );
        assert_eq!(
            feed_all(&mut r, &km, ctx, "0"),
            vec![run(Action::CursorFirst, None)]
        );
    }
}
