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
    /// Keys to resolve afresh, in order, after the output before them (the
    /// keys after a binding that ran or a key that was released). The caller
    /// feeds them again one by one in the input context current then, which
    /// the binding may have changed.
    Refeed(Vec<Key>),
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
    /// When `key` makes the pending sequence match nothing, the sequence ends
    /// as described at [`Resolver::timeout`].
    pub fn feed(
        &mut self,
        map: &EffectiveMap,
        counts: bool,
        timeoutlen_ms: u64,
        key: Key,
    ) -> Vec<Resolved> {
        if counts && self.keys.is_empty() && self.extend_count(map, key) {
            return Vec::new();
        }
        self.keys.push(key);
        let found = map.lookup(&self.keys);
        if found.is_prefix {
            let id = TimerId(self.next_timer);
            self.next_timer += 1;
            self.timer = Some(id);
            vec![Resolved::StartTimer {
                id,
                after_ms: timeoutlen_ms,
            }]
        } else if let Some(binding) = found.binding {
            self.keys.clear();
            self.timer = None;
            vec![Resolved::Run {
                binding: binding.clone(),
                count: self.count.take(),
            }]
        } else {
            // The whole sequence is no binding; a shorter prefix may be.
            self.finish(map, self.keys.len() - 1)
        }
    }

    /// The timer `id` expired, so the pending sequence ends as in nvim: the
    /// longest binding that is a prefix of it runs with the count, or the
    /// first key is released when there is none. The keys after it come
    /// back as [`Resolved::Refeed`]. Stale or unknown ids return nothing.
    pub fn timeout(&mut self, map: &EffectiveMap, id: TimerId) -> Vec<Resolved> {
        if self.timer != Some(id) {
            return Vec::new();
        }
        self.finish(map, self.keys.len())
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

    /// Ends the pending sequence (see [`Resolver::timeout`]), trying
    /// bindings of at most `longest` keys. Clears the count and the timer.
    fn finish(&mut self, map: &EffectiveMap, longest: usize) -> Vec<Resolved> {
        self.timer = None;
        let count = self.count.take();
        let ran = (1..=longest)
            .rev()
            .find_map(|len| map.get(&self.keys[..len]).map(|binding| (len, binding)));
        let rest = self.keys.split_off(ran.map_or(1, |(len, _)| len));
        let first = match ran {
            Some((_, binding)) => {
                self.keys.clear();
                Resolved::Run {
                    binding: binding.clone(),
                    count,
                }
            }
            None => Resolved::Release(mem::take(&mut self.keys)),
        };
        if rest.is_empty() {
            vec![first]
        } else {
            vec![first, Resolved::Refeed(rest)]
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
        let mut out = Vec::new();
        for &key in seq(text).keys() {
            let next = r.feed(km.effective(ctx), ctx.counts(), T, key);
            settle(r, km, ctx, next, &mut out);
        }
        out
    }
    /// The outputs of a timeout, collected like `feed_all`.
    fn timeout_all(r: &mut Resolver, km: &Keymap, ctx: InputContext, id: TimerId) -> Vec<Resolved> {
        let next = r.timeout(km.effective(ctx), id);
        let mut out = Vec::new();
        settle(r, km, ctx, next, &mut out);
        out
    }
    /// Collects `outputs` with timer ids erased. Keys handed back by `Refeed`
    /// are fed again, as the app does.
    fn settle(
        r: &mut Resolver,
        km: &Keymap,
        ctx: InputContext,
        outputs: Vec<Resolved>,
        out: &mut Vec<Resolved>,
    ) {
        for output in outputs {
            match output {
                Resolved::Refeed(keys) => {
                    for key in keys {
                        let next = r.feed(km.effective(ctx), ctx.counts(), T, key);
                        settle(r, km, ctx, next, out);
                    }
                }
                other => out.push(erase(other)),
            }
        }
    }
    /// Feeds keys that stay pending and returns the timer of the last one.
    fn pending_timer(r: &mut Resolver, km: &Keymap, ctx: InputContext, text: &str) -> TimerId {
        let out: Vec<Resolved> = seq(text)
            .keys()
            .iter()
            .flat_map(|&key| r.feed(km.effective(ctx), ctx.counts(), T, key))
            .collect();
        last_timer(&out)
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
    /// `]` in `timeline` and `]gq` in `normal`, the bindings the comparison
    /// with nvim 0.12.4 used.
    fn brackets() -> Keymap {
        Keymap::new(
            &space(),
            [
                (
                    TableId::Timeline,
                    seq("]"),
                    Binding::single(Action::CursorLast),
                ),
                (
                    TableId::Normal,
                    seq("]gq"),
                    Binding::single(Action::RoomMarkRead),
                ),
            ],
        )
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
            vec![release("g")]
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
        // The count typed before the pending keys reaches the binding.
        assert!(r.feed(km.effective(ctx), true, T, k("2")).is_empty());
        let out = r.feed(km.effective(ctx), true, T, k("]"));
        let id = last_timer(&out);
        assert_eq!(
            r.timeout(km.effective(ctx), id),
            vec![run(Action::CursorLast, Some(2))]
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
        assert_eq!(r.timeout(km.effective(ctx), first), vec![]);
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
    fn mismatch_runs_the_longest_binding_then_resolves_the_rest_afresh() {
        // As in nvim: `]` runs, `g` waits again, `gj` matches nothing and `g`
        // is no binding, so `g` is released and `j` runs.
        let km = brackets();
        let mut r = Resolver::new();
        assert_eq!(
            feed_all(&mut r, &km, InputContext::Timeline, "]gj"),
            vec![
                timer(),
                timer(),
                run(Action::CursorLast, None),
                timer(),
                release("g"),
                run(Action::CursorDown, None),
            ]
        );
    }

    #[test]
    fn timeout_runs_the_longest_binding_then_hands_back_the_rest() {
        // As in nvim: `]g` then the timeout runs `]`, and `g` waits again.
        let km = brackets();
        let ctx = InputContext::Timeline;
        let mut r = Resolver::new();
        let id = pending_timer(&mut r, &km, ctx, "]g");
        assert_eq!(
            r.timeout(km.effective(ctx), id),
            vec![
                run(Action::CursorLast, None),
                Resolved::Refeed(seq("g").keys().to_vec()),
            ]
        );
        assert_eq!(feed_all(&mut r, &km, ctx, "g"), vec![timer()]);
        assert_eq!(r.pending(), seq("g").keys());
    }

    #[test]
    fn a_full_miss_releases_only_the_first_key() {
        let km = Keymap::new(
            &space(),
            [(
                TableId::ComposerInsert,
                seq("<C-x><C-y><C-z>"),
                Binding::single(Action::Nop),
            )],
        );
        let mut r = Resolver::new();
        assert_eq!(
            feed_all(&mut r, &km, InputContext::ComposerInsert, "<C-x><C-y>a"),
            vec![
                timer(),
                timer(),
                release("<C-x>"),
                release("<C-y>"),
                release("a"),
            ]
        );
    }

    #[test]
    fn the_count_goes_to_the_binding_that_runs_and_not_to_the_rest() {
        let km = brackets();
        let ctx = InputContext::Timeline;
        let mut r = Resolver::new();
        let id = pending_timer(&mut r, &km, ctx, "2]g");
        assert_eq!(
            timeout_all(&mut r, &km, ctx, id),
            vec![run(Action::CursorLast, Some(2)), timer()]
        );
        assert_eq!(r.count(), None);
        assert_eq!(
            feed_all(&mut r, &km, ctx, "g"),
            vec![run(Action::CursorFirst, None)]
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
