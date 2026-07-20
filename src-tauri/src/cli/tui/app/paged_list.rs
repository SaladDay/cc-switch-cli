//! Page-boundary gating for very long TUI lists.
//!
//! The selected index always refers to a real row. Page boundaries are virtual
//! controls represented by [`PagedListFocus`], never by a sentinel index. A
//! wheel gesture may arm a boundary or consume one, but it cannot do both. This
//! prevents a queued burst of wheel events from silently crossing page after
//! page while keeping explicit keyboard activation straightforward.

use super::super::input::{ScrollDirection, WheelGestureId};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PageDirection {
    Previous,
    Next,
}

impl From<ScrollDirection> for PageDirection {
    fn from(direction: ScrollDirection) -> Self {
        match direction {
            ScrollDirection::Up => Self::Previous,
            ScrollDirection::Down => Self::Next,
        }
    }
}

impl PageDirection {
    const fn boundary(self) -> PageBoundary {
        match self {
            Self::Previous => PageBoundary::Previous,
            Self::Next => PageBoundary::Next,
        }
    }

    const fn edge(self) -> ListEdge {
        match self {
            Self::Previous => ListEdge::Start,
            Self::Next => ListEdge::End,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PageBoundary {
    Previous,
    Next,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ListEdge {
    Start,
    End,
}

/// Focus within a paged list.
///
/// Boundary focus retains the selected real row so details and accessibility
/// context remain stable while the virtual control is active.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PagedListFocus {
    Empty,
    Row,
    Boundary(PageBoundary),
}

/// Result of one state-machine input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PagedListOutcome {
    NoChange,
    Empty,
    SelectionChanged {
        from: usize,
        to: usize,
    },
    BoundaryFocused {
        boundary: PageBoundary,
        from: usize,
        selected: usize,
    },
    BoundaryDismissed {
        boundary: PageBoundary,
        selected: usize,
    },
    PageCrossed {
        direction: PageDirection,
        from_page: usize,
        to_page: usize,
        selected: usize,
    },
    AtEdge {
        edge: ListEdge,
        selected: usize,
    },
    Synced {
        previous_len: usize,
        len: usize,
        from: Option<usize>,
        selected: Option<usize>,
        focus_changed: bool,
    },
}

impl PagedListOutcome {
    /// Whether rendering-visible state changed.
    #[cfg(test)]
    pub(crate) const fn changed(self) -> bool {
        matches!(
            self,
            Self::SelectionChanged { .. }
                | Self::BoundaryFocused { .. }
                | Self::BoundaryDismissed { .. }
                | Self::PageCrossed { .. }
                | Self::Synced { .. }
        )
    }

    #[cfg(test)]
    pub(crate) const fn focused_boundary(self) -> Option<PageBoundary> {
        match self {
            Self::BoundaryFocused { boundary, .. } => Some(boundary),
            _ => None,
        }
    }

    pub(crate) const fn crossed_page(self) -> bool {
        matches!(self, Self::PageCrossed { .. })
    }
}

/// Selection and virtual-boundary state for a fixed-size paged list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PagedListState {
    page_size: usize,
    len: usize,
    selected: Option<usize>,
    focus: PagedListFocus,
    boundary_armed_by: Option<WheelGestureId>,
    active_wheel_gesture: Option<WheelGestureId>,
    active_wheel_crossed_page: bool,
}

impl PagedListState {
    pub(crate) fn new(page_size: usize, len: usize) -> Self {
        assert!(page_size > 0, "paged list page size must be non-zero");

        Self {
            page_size,
            len,
            selected: (len > 0).then_some(0),
            focus: if len == 0 {
                PagedListFocus::Empty
            } else {
                PagedListFocus::Row
            },
            boundary_armed_by: None,
            active_wheel_gesture: None,
            active_wheel_crossed_page: false,
        }
    }

    pub(crate) const fn page_size(&self) -> usize {
        self.page_size
    }

    pub(crate) const fn len(&self) -> usize {
        self.len
    }

    pub(crate) const fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub(crate) const fn selected_index(&self) -> Option<usize> {
        self.selected
    }

    pub(crate) const fn focus(&self) -> PagedListFocus {
        self.focus
    }

    pub(crate) const fn is_row_focused(&self) -> bool {
        matches!(self.focus, PagedListFocus::Row)
    }

    /// Zero-based current page index.
    pub(crate) fn page_index(&self) -> Option<usize> {
        self.selected.map(|selected| selected / self.page_size)
    }

    pub(crate) fn page_count(&self) -> usize {
        self.len.div_ceil(self.page_size)
    }

    /// Inclusive zero-based row range for the current page.
    #[cfg(test)]
    pub(crate) fn page_range(&self) -> Option<(usize, usize)> {
        let selected = self.selected?;
        Some(self.page_bounds(selected))
    }

    pub(crate) fn has_previous_page(&self) -> bool {
        self.selected
            .is_some_and(|selected| self.page_start(selected) > 0)
    }

    pub(crate) fn has_next_page(&self) -> bool {
        self.selected
            .is_some_and(|selected| self.page_end(selected) < self.len - 1)
    }

    #[cfg(test)]
    pub(crate) fn is_at_start(&self) -> bool {
        self.selected == Some(0) && !self.has_previous_page()
    }

    pub(crate) fn is_at_end(&self) -> bool {
        self.selected.is_some() && self.selected == self.len.checked_sub(1) && !self.has_next_page()
    }

    /// Replace the data set and selection, clearing any in-progress gesture.
    ///
    /// `selected` is clamped to the new data set. A non-empty list defaults to
    /// its first row when no selection is supplied.
    pub(crate) fn reset(&mut self, len: usize, selected: Option<usize>) -> PagedListOutcome {
        let previous_len = self.len;
        let from = self.selected;
        let previous_focus = self.focus;

        self.len = len;
        self.selected = Self::normalized_selection(len, selected);
        self.focus = if len == 0 {
            PagedListFocus::Empty
        } else {
            PagedListFocus::Row
        };
        self.clear_wheel_tracking();

        self.sync_outcome(previous_len, from, previous_focus)
    }

    /// Update only the data length, preserving and clamping the selection.
    pub(crate) fn sync_len(&mut self, len: usize) -> PagedListOutcome {
        let previous_len = self.len;
        let from = self.selected;
        let previous_focus = self.focus;

        self.len = len;
        self.selected = Self::normalized_selection(len, self.selected);
        self.focus = match (self.selected, previous_focus) {
            (None, _) => PagedListFocus::Empty,
            (Some(_), PagedListFocus::Boundary(boundary)) if self.can_focus_boundary(boundary) => {
                previous_focus
            }
            (Some(_), _) => PagedListFocus::Row,
        };
        if self.selected != from || self.focus != previous_focus {
            self.clear_wheel_tracking();
        }

        self.sync_outcome(previous_len, from, previous_focus)
    }

    /// Select a real row directly and return focus to the row surface.
    pub(crate) fn select(&mut self, index: usize) -> PagedListOutcome {
        let Some(current) = self.selected else {
            return PagedListOutcome::Empty;
        };
        let target = index.min(self.len - 1);
        let previous_focus = self.focus;

        if target == current && previous_focus == PagedListFocus::Row {
            return PagedListOutcome::NoChange;
        }

        self.selected = Some(target);
        self.focus = PagedListFocus::Row;
        self.clear_wheel_tracking();

        if target != current {
            PagedListOutcome::SelectionChanged {
                from: current,
                to: target,
            }
        } else if let PagedListFocus::Boundary(boundary) = previous_focus {
            PagedListOutcome::BoundaryDismissed {
                boundary,
                selected: target,
            }
        } else {
            PagedListOutcome::NoChange
        }
    }

    /// Apply a coalesced wheel gesture without allowing it to cross two pages.
    pub(crate) fn wheel(
        &mut self,
        direction: PageDirection,
        gesture: WheelGestureId,
        steps: usize,
    ) -> PagedListOutcome {
        if steps == 0 {
            return PagedListOutcome::NoChange;
        }
        let Some(selected) = self.selected else {
            return PagedListOutcome::Empty;
        };
        if self.focus == PagedListFocus::Row && self.is_at_edge(direction) {
            return PagedListOutcome::AtEdge {
                edge: direction.edge(),
                selected,
            };
        }

        if self.active_wheel_gesture != Some(gesture) {
            self.active_wheel_gesture = Some(gesture);
            self.active_wheel_crossed_page = false;
        }
        if self.active_wheel_crossed_page {
            return PagedListOutcome::NoChange;
        }

        if let PagedListFocus::Boundary(boundary) = self.focus {
            if boundary == direction.boundary() {
                if self.boundary_armed_by == Some(gesture) {
                    return PagedListOutcome::NoChange;
                }
                let outcome = self.cross_page(direction);
                if outcome.crossed_page() {
                    self.active_wheel_crossed_page = true;
                }
                return outcome;
            }

            self.focus = PagedListFocus::Row;
            self.boundary_armed_by = None;
            let outcome = self.move_within_page(direction, steps, Some(gesture));
            if matches!(outcome, PagedListOutcome::AtEdge { .. }) {
                return PagedListOutcome::BoundaryDismissed { boundary, selected };
            }
            return outcome;
        }

        self.move_within_page(direction, steps, Some(gesture))
    }

    /// Move one row, or consume an already-focused boundary.
    pub(crate) fn line(&mut self, direction: PageDirection) -> PagedListOutcome {
        self.lines(direction, 1)
    }

    /// Move several rows within the current page, or consume an already-focused
    /// boundary. This keeps PageUp/PageDown viewport-sized without granting a
    /// single explicit input permission to skip multiple logical pages.
    pub(crate) fn lines(&mut self, direction: PageDirection, steps: usize) -> PagedListOutcome {
        if steps == 0 {
            return PagedListOutcome::NoChange;
        }
        if let Some(selected) = self.selected {
            if self.focus == PagedListFocus::Row && self.is_at_edge(direction) {
                return PagedListOutcome::AtEdge {
                    edge: direction.edge(),
                    selected,
                };
            }
        }
        self.begin_explicit_input();
        if self.focus == PagedListFocus::Boundary(direction.boundary()) {
            return self.cross_page(direction);
        }

        let dismissed = match self.focus {
            PagedListFocus::Boundary(boundary) => Some(boundary),
            _ => None,
        };
        self.focus = if self.selected.is_some() {
            PagedListFocus::Row
        } else {
            PagedListFocus::Empty
        };
        self.boundary_armed_by = None;

        let outcome = self.move_within_page(direction, steps, None);
        if matches!(outcome, PagedListOutcome::AtEdge { .. }) {
            if let (Some(boundary), Some(selected)) = (dismissed, self.selected) {
                return PagedListOutcome::BoundaryDismissed { boundary, selected };
            }
        }
        outcome
    }

    /// Move to the corresponding edge of the current page. When that virtual
    /// boundary is already focused, explicitly cross it.
    #[cfg(test)]
    pub(crate) fn page(&mut self, direction: PageDirection) -> PagedListOutcome {
        if let Some(selected) = self.selected {
            if self.focus == PagedListFocus::Row && self.is_at_edge(direction) {
                return PagedListOutcome::AtEdge {
                    edge: direction.edge(),
                    selected,
                };
            }
        }
        self.begin_explicit_input();
        if self.focus == PagedListFocus::Boundary(direction.boundary()) {
            return self.cross_page(direction);
        }

        let Some(selected) = self.selected else {
            return PagedListOutcome::Empty;
        };
        let previous_focus = self.focus;
        self.focus = PagedListFocus::Row;
        self.boundary_armed_by = None;

        let (page_start, page_end) = self.page_bounds(selected);
        let target = match direction {
            PageDirection::Previous => page_start,
            PageDirection::Next => page_end,
        };
        if self.has_page(direction) {
            self.selected = Some(target);
            self.focus = PagedListFocus::Boundary(direction.boundary());
            return PagedListOutcome::BoundaryFocused {
                boundary: direction.boundary(),
                from: selected,
                selected: target,
            };
        }
        if target != selected {
            self.selected = Some(target);
            return PagedListOutcome::SelectionChanged {
                from: selected,
                to: target,
            };
        }
        if let PagedListFocus::Boundary(boundary) = previous_focus {
            return PagedListOutcome::BoundaryDismissed { boundary, selected };
        }
        PagedListOutcome::AtEdge {
            edge: direction.edge(),
            selected,
        }
    }

    /// Activate the focused virtual boundary. Row activation remains available
    /// to the caller because it returns [`PagedListOutcome::NoChange`].
    pub(crate) fn enter(&mut self) -> PagedListOutcome {
        match self.focus {
            PagedListFocus::Boundary(PageBoundary::Previous) => {
                self.begin_explicit_input();
                self.cross_page(PageDirection::Previous)
            }
            PagedListFocus::Boundary(PageBoundary::Next) => {
                self.begin_explicit_input();
                self.cross_page(PageDirection::Next)
            }
            PagedListFocus::Empty => PagedListOutcome::Empty,
            PagedListFocus::Row => PagedListOutcome::NoChange,
        }
    }

    fn normalized_selection(len: usize, selected: Option<usize>) -> Option<usize> {
        (len > 0).then(|| selected.unwrap_or(0).min(len - 1))
    }

    fn sync_outcome(
        &self,
        previous_len: usize,
        from: Option<usize>,
        previous_focus: PagedListFocus,
    ) -> PagedListOutcome {
        let focus_changed = previous_focus != self.focus;
        if previous_len == self.len && from == self.selected && !focus_changed {
            PagedListOutcome::NoChange
        } else {
            PagedListOutcome::Synced {
                previous_len,
                len: self.len,
                from,
                selected: self.selected,
                focus_changed,
            }
        }
    }

    fn clear_wheel_tracking(&mut self) {
        self.boundary_armed_by = None;
        self.active_wheel_gesture = None;
        self.active_wheel_crossed_page = false;
    }

    /// Preserve the one-crossing budget after an asynchronous page failure
    /// restores a gate snapshot captured before the wheel input.
    pub(crate) fn block_wheel_gesture_after_failed_cross(&mut self, gesture: WheelGestureId) {
        self.active_wheel_gesture = Some(gesture);
        self.active_wheel_crossed_page = true;
    }

    fn begin_explicit_input(&mut self) {
        self.active_wheel_gesture = None;
        self.active_wheel_crossed_page = false;
    }

    fn page_start(&self, selected: usize) -> usize {
        (selected / self.page_size) * self.page_size
    }

    fn page_end(&self, selected: usize) -> usize {
        self.page_start(selected)
            .saturating_add(self.page_size)
            .min(self.len)
            - 1
    }

    fn page_bounds(&self, selected: usize) -> (usize, usize) {
        (self.page_start(selected), self.page_end(selected))
    }

    fn has_page(&self, direction: PageDirection) -> bool {
        match direction {
            PageDirection::Previous => self.has_previous_page(),
            PageDirection::Next => self.has_next_page(),
        }
    }

    fn is_at_edge(&self, direction: PageDirection) -> bool {
        match direction {
            PageDirection::Previous => self.selected == Some(0),
            PageDirection::Next => self.selected == self.len.checked_sub(1),
        }
    }

    fn can_focus_boundary(&self, boundary: PageBoundary) -> bool {
        let Some(selected) = self.selected else {
            return false;
        };
        match boundary {
            PageBoundary::Previous => {
                selected == self.page_start(selected) && self.has_previous_page()
            }
            PageBoundary::Next => selected == self.page_end(selected) && self.has_next_page(),
        }
    }

    fn move_within_page(
        &mut self,
        direction: PageDirection,
        steps: usize,
        armed_by: Option<WheelGestureId>,
    ) -> PagedListOutcome {
        let Some(selected) = self.selected else {
            return PagedListOutcome::Empty;
        };
        let (page_start, page_end) = self.page_bounds(selected);
        let target = match direction {
            PageDirection::Previous => selected.saturating_sub(steps).max(page_start),
            PageDirection::Next => selected.saturating_add(steps).min(page_end),
        };

        if target == selected && !self.has_page(direction) {
            return PagedListOutcome::AtEdge {
                edge: direction.edge(),
                selected,
            };
        }

        self.selected = Some(target);
        if target
            == match direction {
                PageDirection::Previous => page_start,
                PageDirection::Next => page_end,
            }
            && self.has_page(direction)
        {
            self.focus = PagedListFocus::Boundary(direction.boundary());
            self.boundary_armed_by = armed_by;
            return PagedListOutcome::BoundaryFocused {
                boundary: direction.boundary(),
                from: selected,
                selected: target,
            };
        }

        self.focus = PagedListFocus::Row;
        self.boundary_armed_by = None;
        if target == selected {
            PagedListOutcome::NoChange
        } else {
            PagedListOutcome::SelectionChanged {
                from: selected,
                to: target,
            }
        }
    }

    fn cross_page(&mut self, direction: PageDirection) -> PagedListOutcome {
        let Some(selected) = self.selected else {
            return PagedListOutcome::Empty;
        };
        if !self.has_page(direction) {
            return PagedListOutcome::AtEdge {
                edge: direction.edge(),
                selected,
            };
        }

        let from_page = selected / self.page_size;
        let target = match direction {
            PageDirection::Previous => self.page_start(selected) - 1,
            PageDirection::Next => self.page_end(selected) + 1,
        };
        let to_page = target / self.page_size;

        self.selected = Some(target);
        self.focus = PagedListFocus::Row;
        self.boundary_armed_by = None;
        PagedListOutcome::PageCrossed {
            direction,
            from_page,
            to_page,
            selected: target,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gesture(id: u64) -> WheelGestureId {
        let gesture = WheelGestureId::from_raw(id);
        assert_eq!(gesture.raw(), id);
        gesture
    }

    #[test]
    fn reports_page_metadata_without_sentinel_rows() {
        let mut state = PagedListState::new(10, 25);

        assert_eq!(state.page_size(), 10);
        assert_eq!(state.len(), 25);
        assert!(!state.is_empty());
        assert_eq!(state.selected_index(), Some(0));
        assert_eq!(state.focus(), PagedListFocus::Row);
        assert!(state.is_row_focused());
        assert_eq!(state.page_index(), Some(0));
        assert_eq!(state.page_count(), 3);
        assert_eq!(state.page_range(), Some((0, 9)));
        assert!(!state.has_previous_page());
        assert!(state.has_next_page());
        assert!(state.is_at_start());
        assert!(!state.is_at_end());

        state.select(24);
        assert_eq!(state.page_index(), Some(2));
        assert_eq!(state.page_range(), Some((20, 24)));
        assert!(state.has_previous_page());
        assert!(!state.has_next_page());
        assert!(state.is_at_end());
    }

    #[test]
    fn one_wheel_gesture_arms_next_boundary_but_cannot_cross_it() {
        let mut state = PagedListState::new(10, 30);
        state.select(6);
        let id = gesture(1);

        let outcome = state.wheel(PageDirection::Next, id, 100);
        assert_eq!(
            outcome,
            PagedListOutcome::BoundaryFocused {
                boundary: PageBoundary::Next,
                from: 6,
                selected: 9,
            }
        );
        assert!(outcome.changed());
        assert_eq!(outcome.focused_boundary(), Some(PageBoundary::Next));
        assert_eq!(state.selected_index(), Some(9));
        assert_eq!(state.focus(), PagedListFocus::Boundary(PageBoundary::Next));

        assert_eq!(
            state.wheel(PageDirection::Next, id, 100),
            PagedListOutcome::NoChange
        );
        assert_eq!(state.selected_index(), Some(9));
    }

    #[test]
    fn new_wheel_gesture_crosses_one_page_then_locks() {
        let mut state = PagedListState::new(10, 30);
        state.select(8);
        state.wheel(PageDirection::Next, gesture(1), 20);

        let outcome = state.wheel(PageDirection::Next, gesture(2), 20);
        assert_eq!(
            outcome,
            PagedListOutcome::PageCrossed {
                direction: PageDirection::Next,
                from_page: 0,
                to_page: 1,
                selected: 10,
            }
        );
        assert!(outcome.changed());
        assert!(outcome.crossed_page());
        assert_eq!(state.focus(), PagedListFocus::Row);

        assert_eq!(
            state.wheel(PageDirection::Next, gesture(2), 20),
            PagedListOutcome::NoChange
        );
        assert_eq!(state.selected_index(), Some(10));

        assert_eq!(
            state.wheel(PageDirection::Next, gesture(3), 2),
            PagedListOutcome::SelectionChanged { from: 10, to: 12 }
        );
    }

    #[test]
    fn previous_boundary_is_symmetric() {
        let mut state = PagedListState::new(10, 30);
        state.select(14);

        assert_eq!(
            state.wheel(PageDirection::Previous, gesture(7), 100),
            PagedListOutcome::BoundaryFocused {
                boundary: PageBoundary::Previous,
                from: 14,
                selected: 10,
            }
        );
        assert_eq!(
            state.wheel(PageDirection::Previous, gesture(7), 1),
            PagedListOutcome::NoChange
        );
        assert_eq!(
            state.wheel(PageDirection::Previous, gesture(8), 100),
            PagedListOutcome::PageCrossed {
                direction: PageDirection::Previous,
                from_page: 1,
                to_page: 0,
                selected: 9,
            }
        );
        assert_eq!(
            state.wheel(PageDirection::Previous, gesture(8), 1),
            PagedListOutcome::NoChange
        );
    }

    #[test]
    fn page_keys_arm_then_explicitly_consume_boundaries() {
        let mut state = PagedListState::new(10, 25);
        state.select(3);

        assert_eq!(
            state.page(PageDirection::Next),
            PagedListOutcome::BoundaryFocused {
                boundary: PageBoundary::Next,
                from: 3,
                selected: 9,
            }
        );
        assert_eq!(
            state.page(PageDirection::Next),
            PagedListOutcome::PageCrossed {
                direction: PageDirection::Next,
                from_page: 0,
                to_page: 1,
                selected: 10,
            }
        );
        assert_eq!(
            state.page(PageDirection::Previous),
            PagedListOutcome::BoundaryFocused {
                boundary: PageBoundary::Previous,
                from: 10,
                selected: 10,
            }
        );
        assert_eq!(
            state.page(PageDirection::Previous),
            PagedListOutcome::PageCrossed {
                direction: PageDirection::Previous,
                from_page: 1,
                to_page: 0,
                selected: 9,
            }
        );
    }

    #[test]
    fn enter_only_consumes_a_focused_boundary() {
        let mut state = PagedListState::new(5, 12);

        assert_eq!(state.enter(), PagedListOutcome::NoChange);
        state.page(PageDirection::Next);
        assert_eq!(
            state.enter(),
            PagedListOutcome::PageCrossed {
                direction: PageDirection::Next,
                from_page: 0,
                to_page: 1,
                selected: 5,
            }
        );

        state.page(PageDirection::Previous);
        assert_eq!(
            state.enter(),
            PagedListOutcome::PageCrossed {
                direction: PageDirection::Previous,
                from_page: 1,
                to_page: 0,
                selected: 4,
            }
        );
    }

    #[test]
    fn line_navigation_uses_the_same_two_trigger_gate() {
        let mut state = PagedListState::new(3, 8);
        state.select(1);

        assert_eq!(
            state.line(PageDirection::Next),
            PagedListOutcome::BoundaryFocused {
                boundary: PageBoundary::Next,
                from: 1,
                selected: 2,
            }
        );
        assert_eq!(
            state.line(PageDirection::Next),
            PagedListOutcome::PageCrossed {
                direction: PageDirection::Next,
                from_page: 0,
                to_page: 1,
                selected: 3,
            }
        );
    }

    #[test]
    fn true_end_and_start_are_strict_no_op_edges() {
        let mut state = PagedListState::new(10, 25);
        state.select(24);
        let before = state.clone();

        for outcome in [
            state.wheel(PageDirection::Next, gesture(1), 100),
            state.page(PageDirection::Next),
            state.line(PageDirection::Next),
        ] {
            assert_eq!(
                outcome,
                PagedListOutcome::AtEdge {
                    edge: ListEdge::End,
                    selected: 24,
                }
            );
            assert!(!outcome.changed());
        }
        assert_eq!(state, before);

        state.select(0);
        let before = state.clone();
        assert_eq!(
            state.wheel(PageDirection::Previous, gesture(2), 100),
            PagedListOutcome::AtEdge {
                edge: ListEdge::Start,
                selected: 0,
            }
        );
        assert_eq!(state, before);
    }

    #[test]
    fn partial_last_page_has_no_fake_next_boundary() {
        let mut state = PagedListState::new(10, 25);
        state.select(21);

        assert_eq!(
            state.wheel(PageDirection::Next, gesture(1), 100),
            PagedListOutcome::SelectionChanged { from: 21, to: 24 }
        );
        assert_eq!(state.focus(), PagedListFocus::Row);
        assert_eq!(
            state.wheel(PageDirection::Next, gesture(1), 1),
            PagedListOutcome::AtEdge {
                edge: ListEdge::End,
                selected: 24,
            }
        );
    }

    #[test]
    fn reversing_direction_dismisses_the_opposite_boundary() {
        let mut state = PagedListState::new(10, 30);
        state.select(9);
        state.wheel(PageDirection::Next, gesture(1), 1);

        assert_eq!(
            state.wheel(PageDirection::Previous, gesture(1), 1),
            PagedListOutcome::SelectionChanged { from: 9, to: 8 }
        );
        assert_eq!(state.focus(), PagedListFocus::Row);
    }

    #[test]
    fn sync_len_clamps_selection_and_dismisses_invalid_boundary() {
        let mut state = PagedListState::new(10, 30);
        state.select(19);
        state.wheel(PageDirection::Next, gesture(1), 1);
        assert_eq!(state.focus(), PagedListFocus::Boundary(PageBoundary::Next));

        assert_eq!(
            state.sync_len(15),
            PagedListOutcome::Synced {
                previous_len: 30,
                len: 15,
                from: Some(19),
                selected: Some(14),
                focus_changed: true,
            }
        );
        assert_eq!(state.selected_index(), Some(14));
        assert_eq!(state.focus(), PagedListFocus::Row);
        assert!(state.is_at_end());
    }

    #[test]
    fn harmless_length_sync_does_not_unlock_the_active_wheel_gesture() {
        let mut state = PagedListState::new(10, 30);
        let first = gesture(1);
        state.select(8);
        state.wheel(PageDirection::Next, first, 10);

        assert!(state.sync_len(31).changed());
        assert_eq!(
            state.wheel(PageDirection::Next, first, 10),
            PagedListOutcome::NoChange
        );

        let crossing = gesture(2);
        assert!(state
            .wheel(PageDirection::Next, crossing, 10)
            .crossed_page());
        assert!(state.sync_len(32).changed());
        assert_eq!(
            state.wheel(PageDirection::Next, crossing, 10),
            PagedListOutcome::NoChange
        );
    }

    #[test]
    fn reselecting_the_focused_row_does_not_unlock_a_crossing_gesture() {
        let mut state = PagedListState::new(10, 30);
        state.page(PageDirection::Next);
        let crossing = gesture(4);
        assert!(state
            .wheel(PageDirection::Next, crossing, 10)
            .crossed_page());

        assert_eq!(state.select(10), PagedListOutcome::NoChange);
        assert_eq!(
            state.wheel(PageDirection::Next, crossing, 10),
            PagedListOutcome::NoChange
        );
    }

    #[test]
    fn sync_len_handles_empty_and_new_data() {
        let mut state = PagedListState::new(10, 4);
        state.select(3);

        assert!(state.sync_len(0).changed());
        assert!(state.is_empty());
        assert_eq!(state.selected_index(), None);
        assert_eq!(state.focus(), PagedListFocus::Empty);
        assert_eq!(state.page_index(), None);
        assert_eq!(state.page_range(), None);
        assert_eq!(state.page_count(), 0);
        assert_eq!(
            state.wheel(PageDirection::Next, gesture(1), 1),
            PagedListOutcome::Empty
        );
        assert_eq!(state.enter(), PagedListOutcome::Empty);

        assert_eq!(
            state.sync_len(6),
            PagedListOutcome::Synced {
                previous_len: 0,
                len: 6,
                from: None,
                selected: Some(0),
                focus_changed: true,
            }
        );
    }

    #[test]
    fn reset_uses_a_real_clamped_selection_and_clears_gesture_lock() {
        let mut state = PagedListState::new(2, 8);
        state.page(PageDirection::Next);
        state.wheel(PageDirection::Next, gesture(9), 10);
        assert_eq!(state.selected_index(), Some(2));

        assert!(state.reset(5, Some(99)).changed());
        assert_eq!(state.selected_index(), Some(4));
        assert_eq!(state.focus(), PagedListFocus::Row);
        assert_eq!(
            state.wheel(PageDirection::Previous, gesture(9), 1),
            PagedListOutcome::BoundaryFocused {
                boundary: PageBoundary::Previous,
                from: 4,
                selected: 4,
            }
        );
    }

    #[test]
    fn selecting_boundary_row_does_not_create_virtual_focus() {
        let mut state = PagedListState::new(10, 20);
        state.select(9);

        assert_eq!(state.focus(), PagedListFocus::Row);
        assert_eq!(
            state.wheel(PageDirection::Next, gesture(1), 1),
            PagedListOutcome::BoundaryFocused {
                boundary: PageBoundary::Next,
                from: 9,
                selected: 9,
            }
        );
    }

    #[test]
    fn page_size_one_still_requires_two_triggers() {
        let mut state = PagedListState::new(1, 3);

        assert_eq!(
            state.wheel(PageDirection::Next, gesture(1), 1),
            PagedListOutcome::BoundaryFocused {
                boundary: PageBoundary::Next,
                from: 0,
                selected: 0,
            }
        );
        assert_eq!(
            state.wheel(PageDirection::Next, gesture(2), 1),
            PagedListOutcome::PageCrossed {
                direction: PageDirection::Next,
                from_page: 0,
                to_page: 1,
                selected: 1,
            }
        );
        assert_eq!(
            state.wheel(PageDirection::Next, gesture(2), 1),
            PagedListOutcome::NoChange
        );
    }

    #[test]
    fn input_scroll_direction_maps_to_page_direction() {
        assert_eq!(
            PageDirection::from(ScrollDirection::Up),
            PageDirection::Previous
        );
        assert_eq!(
            PageDirection::from(ScrollDirection::Down),
            PageDirection::Next
        );
    }

    #[test]
    #[should_panic(expected = "page size must be non-zero")]
    fn zero_page_size_is_rejected() {
        let _ = PagedListState::new(0, 10);
    }
}
