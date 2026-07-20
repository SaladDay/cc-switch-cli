//! Bounded terminal input draining and wheel-gesture reduction.
//!
//! A wheel gesture does not end merely because a drain pass hit its work limit.
//! It ends only after the terminal queue was observed empty, stayed quiet for
//! [`WHEEL_GESTURE_QUIET`], and was observed empty again. This deliberately
//! favors keeping two bursts together over splitting one physical gesture while
//! terminal input is backlogged.

use std::collections::VecDeque;
use std::io;
use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyEvent, KeyEventKind, MouseEventKind};

/// Quiet time required between two empty-queue observations.
pub(super) const WHEEL_GESTURE_QUIET: Duration = Duration::from_millis(145);

const DEFAULT_MAX_DRAIN_EVENTS: usize = 256;
const DEFAULT_MAX_REDUCED_INPUTS: usize = 16;
const DEFAULT_DRAIN_BUDGET: Duration = Duration::from_millis(4);

/// Stable identity shared by all wheel segments in one physical gesture.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct WheelGestureId(u64);

impl WheelGestureId {
    /// Constructs an ID at an input boundary or in a deterministic state test.
    pub(crate) const fn from_raw(value: u64) -> Self {
        Self(value)
    }

    #[cfg(test)]
    pub(crate) const fn raw(self) -> u64 {
        self.0
    }
}

/// Vertical wheel direction understood by the TUI.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ScrollDirection {
    Up,
    Down,
}

/// An ordered, reduced unit of input consumed by the UI reducer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum UiInput {
    Key(KeyEvent),
    Wheel {
        direction: ScrollDirection,
        steps: u32,
        gesture: WheelGestureId,
    },
    Resize {
        width: u16,
        height: u16,
    },
}

/// Synchronous source used by [`InputReader`].
///
/// The small trait keeps queue and timing behavior independently testable while
/// the production implementation delegates directly to Crossterm.
pub(super) trait EventSource {
    fn poll(&mut self, timeout: Duration) -> io::Result<bool>;
    fn read(&mut self) -> io::Result<Event>;
}

/// Crossterm-backed terminal event source.
#[derive(Debug, Default)]
pub(super) struct CrosstermEventSource;

impl EventSource for CrosstermEventSource {
    fn poll(&mut self, timeout: Duration) -> io::Result<bool> {
        event::poll(timeout)
    }

    fn read(&mut self) -> io::Result<Event> {
        event::read()
    }
}

/// Pure state core for ordered coalescing and wheel-gesture lifetime tracking.
#[derive(Debug)]
pub(super) struct InputReducer {
    pending: VecDeque<UiInput>,
    active_gesture: Option<WheelGestureId>,
    empty_since: Option<Instant>,
    next_gesture_id: u64,
}

impl Default for InputReducer {
    fn default() -> Self {
        Self {
            pending: VecDeque::new(),
            active_gesture: None,
            empty_since: None,
            next_gesture_id: 1,
        }
    }
}

impl InputReducer {
    /// Reduces one raw terminal event without changing the relative order of
    /// visible input. Unsupported mouse/focus/paste events are intentionally
    /// ignored and do not prolong an otherwise quiet wheel gesture.
    pub(super) fn push_event(&mut self, event: Event) {
        match event {
            Event::Key(key) if key.kind == KeyEventKind::Press => {
                self.pending.push_back(UiInput::Key(key));
                // A real keyboard action is an unambiguous interaction
                // boundary. Any later wheel report must not inherit the ID of
                // inertia that arrived before this key.
                self.active_gesture = None;
                self.empty_since = None;
            }
            Event::Mouse(mouse) => match mouse.kind {
                MouseEventKind::ScrollUp => self.push_wheel(ScrollDirection::Up),
                MouseEventKind::ScrollDown => self.push_wheel(ScrollDirection::Down),
                _ => {}
            },
            Event::Resize(width, height) => self.push_resize(width, height),
            _ => {}
        }
    }

    /// Records that the raw source was observed empty.
    ///
    /// Returns `true` only when this observation closes an active gesture.
    pub(super) fn confirm_source_empty(&mut self, now: Instant) -> bool {
        let Some(_) = self.active_gesture else {
            self.empty_since = None;
            return false;
        };

        match self.empty_since {
            Some(started) if now.saturating_duration_since(started) >= WHEEL_GESTURE_QUIET => {
                self.active_gesture = None;
                self.empty_since = None;
                true
            }
            Some(_) => false,
            None => {
                self.empty_since = Some(now);
                false
            }
        }
    }

    /// Marks a bounded drain as incomplete. An incomplete pass is never proof
    /// of silence, so it cannot advance or close the current gesture.
    pub(super) fn drain_interrupted(&mut self) {
        self.empty_since = None;
    }

    pub(super) fn quiet_deadline(&self) -> Option<Instant> {
        self.active_gesture
            .and(self.empty_since)
            .and_then(|started| started.checked_add(WHEEL_GESTURE_QUIET))
    }

    pub(super) fn take_inputs(&mut self) -> Vec<UiInput> {
        self.pending.drain(..).collect()
    }

    fn push_wheel(&mut self, direction: ScrollDirection) {
        self.empty_since = None;
        let gesture = match self.active_gesture {
            Some(gesture) => gesture,
            None => {
                let gesture = WheelGestureId::from_raw(self.next_gesture_id);
                self.next_gesture_id = self.next_gesture_id.wrapping_add(1);
                if self.next_gesture_id == 0 {
                    self.next_gesture_id = 1;
                }
                self.active_gesture = Some(gesture);
                gesture
            }
        };

        match self.pending.back_mut() {
            Some(UiInput::Wheel {
                direction: previous_direction,
                steps,
                gesture: previous_gesture,
            }) if *previous_direction == direction && *previous_gesture == gesture => {
                *steps = steps.saturating_add(1);
            }
            _ => self.pending.push_back(UiInput::Wheel {
                direction,
                steps: 1,
                gesture,
            }),
        }
    }

    fn push_resize(&mut self, width: u16, height: u16) {
        match self.pending.back_mut() {
            Some(UiInput::Resize {
                width: previous_width,
                height: previous_height,
            }) => {
                *previous_width = width;
                *previous_height = height;
            }
            _ => self.pending.push_back(UiInput::Resize { width, height }),
        }
    }
}

/// Bounded synchronous reader that drains immediately available terminal input.
pub(super) struct InputReader<S = CrosstermEventSource> {
    source: S,
    reducer: InputReducer,
    max_drain_events: usize,
    drain_budget: Duration,
}

impl InputReader<CrosstermEventSource> {
    pub(super) fn crossterm() -> Self {
        Self::new(CrosstermEventSource)
    }
}

impl<S: EventSource> InputReader<S> {
    pub(super) fn new(source: S) -> Self {
        Self::with_limits(source, DEFAULT_MAX_DRAIN_EVENTS, DEFAULT_DRAIN_BUDGET)
    }

    pub(super) fn with_limits(source: S, max_drain_events: usize, drain_budget: Duration) -> Self {
        Self {
            source,
            reducer: InputReducer::default(),
            max_drain_events: max_drain_events.max(1),
            drain_budget,
        }
    }

    /// Waits for input and returns one ordered, reduced batch.
    ///
    /// The wait is shortened to an active gesture's quiet deadline so the
    /// second empty-queue confirmation happens even when the normal UI tick is
    /// longer than the gesture window.
    pub(super) fn read_batch(&mut self, timeout: Duration) -> io::Result<Vec<UiInput>> {
        let now = Instant::now();
        let timeout = self.reducer.quiet_deadline().map_or(timeout, |deadline| {
            timeout.min(deadline.saturating_duration_since(now))
        });

        if !self.source.poll(timeout)? {
            self.reducer.confirm_source_empty(Instant::now());
            return Ok(self.reducer.take_inputs());
        }

        let drain_started = Instant::now();
        let mut drained = 0usize;
        loop {
            let event = self.source.read()?;
            // Do not pre-consume input that belongs to a synchronous terminal
            // hand-off (for example, a key that opens an external editor).
            // Wheel reports before the key are still coalesced into this batch;
            // anything after it remains in Crossterm's queue.
            let stop_after_key =
                matches!(&event, Event::Key(key) if key.kind == KeyEventKind::Press);
            self.reducer.push_event(event);
            drained = drained.saturating_add(1);

            if stop_after_key {
                self.reducer.drain_interrupted();
                break;
            }

            if drained >= self.max_drain_events
                || self.reducer.pending.len() >= DEFAULT_MAX_REDUCED_INPUTS
                || drain_started.elapsed() >= self.drain_budget
            {
                // The bound may have landed exactly on the final queued event.
                // Probe once so an already-empty source starts its quiet window
                // now instead of adding a whole UI tick before the 145ms gate.
                if self.source.poll(Duration::ZERO)? {
                    self.reducer.drain_interrupted();
                } else {
                    self.reducer.confirm_source_empty(Instant::now());
                }
                break;
            }

            if !self.source.poll(Duration::ZERO)? {
                self.reducer.confirm_source_empty(Instant::now());
                break;
            }
        }

        Ok(self.reducer.take_inputs())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyModifiers, MouseEvent};

    #[derive(Default)]
    struct FakeEventSource {
        events: VecDeque<Event>,
    }

    impl FakeEventSource {
        fn from_events(events: impl IntoIterator<Item = Event>) -> Self {
            Self {
                events: events.into_iter().collect(),
            }
        }
    }

    impl EventSource for FakeEventSource {
        fn poll(&mut self, _timeout: Duration) -> io::Result<bool> {
            Ok(!self.events.is_empty())
        }

        fn read(&mut self) -> io::Result<Event> {
            self.events.pop_front().ok_or_else(|| {
                io::Error::new(io::ErrorKind::UnexpectedEof, "fake event queue is empty")
            })
        }
    }

    fn wheel(direction: ScrollDirection) -> Event {
        let kind = match direction {
            ScrollDirection::Up => MouseEventKind::ScrollUp,
            ScrollDirection::Down => MouseEventKind::ScrollDown,
        };
        Event::Mouse(MouseEvent {
            kind,
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        })
    }

    fn moved() -> Event {
        Event::Mouse(MouseEvent {
            kind: MouseEventKind::Moved,
            column: 12,
            row: 7,
            modifiers: KeyModifiers::NONE,
        })
    }

    fn key(code: KeyCode) -> Event {
        Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn wheel_parts(input: UiInput) -> (ScrollDirection, u32, WheelGestureId) {
        match input {
            UiInput::Wheel {
                direction,
                steps,
                gesture,
            } => (direction, steps, gesture),
            other => panic!("expected wheel input, got {other:?}"),
        }
    }

    #[test]
    fn backlog_cut_by_drain_limit_keeps_one_gesture() -> io::Result<()> {
        let source = FakeEventSource::from_events([
            wheel(ScrollDirection::Down),
            wheel(ScrollDirection::Down),
            wheel(ScrollDirection::Down),
        ]);
        let mut reader = InputReader::with_limits(source, 2, Duration::from_secs(1));

        let first = reader.read_batch(Duration::ZERO)?;
        let (_, first_steps, first_gesture) = wheel_parts(first[0]);
        assert_eq!(first_steps, 2);
        assert!(reader.reducer.empty_since.is_none());

        let second = reader.read_batch(Duration::ZERO)?;
        let (_, second_steps, second_gesture) = wheel_parts(second[0]);
        assert_eq!(second_steps, 1);
        assert_eq!(first_gesture, second_gesture);
        assert!(reader.reducer.empty_since.is_some());
        Ok(())
    }

    #[test]
    fn key_stops_raw_drain_so_later_input_stays_available_for_terminal_handoff() -> io::Result<()> {
        let source = FakeEventSource::from_events([
            wheel(ScrollDirection::Down),
            key(KeyCode::Char('e')),
            key(KeyCode::Char('x')),
        ]);
        let mut reader = InputReader::with_limits(source, 256, Duration::from_secs(1));

        let first = reader.read_batch(Duration::ZERO)?;
        assert_eq!(first.len(), 2);
        assert!(matches!(first[0], UiInput::Wheel { steps: 1, .. }));
        assert!(matches!(
            first[1],
            UiInput::Key(KeyEvent {
                code: KeyCode::Char('e'),
                ..
            })
        ));
        assert_eq!(reader.source.events.len(), 1);

        let second = reader.read_batch(Duration::ZERO)?;
        assert!(matches!(
            second.as_slice(),
            [UiInput::Key(KeyEvent {
                code: KeyCode::Char('x'),
                ..
            })]
        ));
        Ok(())
    }

    #[test]
    fn elapsed_time_without_second_empty_confirmation_does_not_end_gesture() {
        let start = Instant::now();
        let mut reducer = InputReducer::default();
        reducer.push_event(wheel(ScrollDirection::Down));
        let (_, _, first_gesture) = wheel_parts(reducer.take_inputs()[0]);
        assert!(!reducer.confirm_source_empty(start));

        reducer.push_event(wheel(ScrollDirection::Down));
        let (_, _, later_gesture) = wheel_parts(reducer.take_inputs()[0]);
        assert_eq!(first_gesture, later_gesture);
    }

    #[test]
    fn quiet_window_needs_two_empty_observations_and_then_allocates_new_id() {
        let start = Instant::now();
        let mut reducer = InputReducer::default();
        reducer.push_event(wheel(ScrollDirection::Down));
        let (_, _, first_gesture) = wheel_parts(reducer.take_inputs()[0]);

        assert!(!reducer.confirm_source_empty(start));
        assert!(
            !reducer.confirm_source_empty(start + WHEEL_GESTURE_QUIET - Duration::from_millis(1))
        );
        assert!(reducer.confirm_source_empty(start + WHEEL_GESTURE_QUIET));

        reducer.push_event(wheel(ScrollDirection::Down));
        let (_, _, second_gesture) = wheel_parts(reducer.take_inputs()[0]);
        assert_ne!(first_gesture, second_gesture);
        assert_eq!(first_gesture.raw() + 1, second_gesture.raw());
    }

    #[test]
    fn key_resize_and_direction_changes_preserve_order_and_segment_wheels() {
        let mut reducer = InputReducer::default();
        for event in [
            wheel(ScrollDirection::Down),
            wheel(ScrollDirection::Down),
            key(KeyCode::Char('x')),
            wheel(ScrollDirection::Down),
            Event::Resize(80, 24),
            Event::Resize(100, 30),
            wheel(ScrollDirection::Up),
            wheel(ScrollDirection::Down),
        ] {
            reducer.push_event(event);
        }

        let inputs = reducer.take_inputs();
        assert_eq!(inputs.len(), 6);
        let (direction, steps, first_gesture) = wheel_parts(inputs[0]);
        assert_eq!((direction, steps), (ScrollDirection::Down, 2));
        assert_eq!(
            inputs[1],
            UiInput::Key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE))
        );
        let (direction, steps, second_gesture) = wheel_parts(inputs[2]);
        assert_eq!((direction, steps), (ScrollDirection::Down, 1));
        assert_ne!(first_gesture, second_gesture);
        assert_eq!(
            inputs[3],
            UiInput::Resize {
                width: 100,
                height: 30,
            }
        );
        assert_eq!(
            inputs[4],
            UiInput::Wheel {
                direction: ScrollDirection::Up,
                steps: 1,
                gesture: second_gesture,
            }
        );
        assert_eq!(
            inputs[5],
            UiInput::Wheel {
                direction: ScrollDirection::Down,
                steps: 1,
                gesture: second_gesture,
            }
        );
    }

    #[test]
    fn mouse_movement_is_ignored_and_does_not_break_visible_wheel_coalescing() {
        let mut reducer = InputReducer::default();
        reducer.push_event(wheel(ScrollDirection::Down));
        reducer.push_event(moved());
        reducer.push_event(wheel(ScrollDirection::Down));

        let inputs = reducer.take_inputs();
        assert_eq!(inputs.len(), 1);
        let (direction, steps, _) = wheel_parts(inputs[0]);
        assert_eq!((direction, steps), (ScrollDirection::Down, 2));
    }

    #[test]
    fn consecutive_resizes_keep_latest_size_but_never_cross_a_key() {
        let mut reducer = InputReducer::default();
        reducer.push_event(Event::Resize(80, 24));
        reducer.push_event(Event::Resize(90, 25));
        reducer.push_event(key(KeyCode::Enter));
        reducer.push_event(Event::Resize(100, 30));
        reducer.push_event(Event::Resize(120, 40));

        assert_eq!(
            reducer.take_inputs(),
            vec![
                UiInput::Resize {
                    width: 90,
                    height: 25,
                },
                UiInput::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
                UiInput::Resize {
                    width: 120,
                    height: 40,
                },
            ]
        );
    }
}
