use super::*;

#[derive(Debug, Clone)]
pub struct FilterState {
    pub active: bool,
    pub input: TextInput,
    pub scope: FilterScope,
}

impl FilterState {
    pub fn new() -> Self {
        Self {
            active: false,
            input: TextInput::new(""),
            scope: FilterScope::Global,
        }
    }

    pub fn query_lower(&self) -> Option<String> {
        let trimmed = self.input.value.trim();
        if trimmed.is_empty() {
            return None;
        }
        Some(trimmed.to_lowercase())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterScope {
    Global,
    SessionMessages,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SkillsDiscoverSource {
    Repos,
    Marketplace,
}

impl SkillsDiscoverSource {
    pub fn toggled(self) -> Self {
        match self {
            Self::Repos => Self::Marketplace,
            Self::Marketplace => Self::Repos,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Nav,
    Content,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionsPane {
    List,
    Detail,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsagePane {
    Models,
    Providers,
    Recent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsageMetric {
    Cost,
    Tokens,
    Requests,
    Errors,
}

// The active UiData head and this pager's immutable head account for 200 rows.
// Two additional pages are the minimum needed to keep the visible page and a
// prefetched boundary page simultaneously, while staying below 500 active rows.
const USAGE_LOG_CACHE_PAGES: usize = 2;
const USAGE_LOG_MAX_PENDING_PAGES: usize = 2;
const USAGE_LOG_MAX_ERROR_PAGES: usize = 5;
const USAGE_LOG_FINGERPRINT_STRING_PREFIX_CHARS: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq)]
struct UsageLogPageSource {
    app_type: AppType,
    range: crate::cli::tui::data::UsageRangePreset,
    first_page_cursor: Option<crate::cli::tui::data::UsageLogCursor>,
    first_page_fingerprint: u64,
    total: usize,
}

fn usage_log_rows_fingerprint(rows: &[crate::cli::tui::data::UsageLogRow]) -> u64 {
    use std::hash::{Hash, Hasher};

    fn hash_str_prefix(value: &str, hasher: &mut impl Hasher) {
        let mut chars = value.chars();
        for _ in 0..USAGE_LOG_FINGERPRINT_STRING_PREFIX_CHARS {
            let Some(ch) = chars.next() else {
                false.hash(hasher);
                return;
            };
            ch.hash(hasher);
        }
        // Distinguish an exact-prefix-length value from a longer value without
        // scanning or hashing the unbounded suffix.
        chars.next().is_some().hash(hasher);
    }

    fn hash_optional_str_prefix(value: Option<&str>, hasher: &mut impl Hasher) {
        value.is_some().hash(hasher);
        if let Some(value) = value {
            hash_str_prefix(value, hasher);
        }
    }

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    rows.len().hash(&mut hasher);
    for row in rows {
        hash_str_prefix(&row.request_id, &mut hasher);
        row.created_at.hash(&mut hasher);
        row.cursor_rowid.hash(&mut hasher);
        hash_str_prefix(&row.app_type, &mut hasher);
        hash_str_prefix(&row.provider_id, &mut hasher);
        hash_optional_str_prefix(row.provider_name.as_deref(), &mut hasher);
        hash_str_prefix(&row.model, &mut hasher);
        hash_optional_str_prefix(row.request_model.as_deref(), &mut hasher);
        row.status_code.hash(&mut hasher);
        row.input_tokens.hash(&mut hasher);
        row.output_tokens.hash(&mut hasher);
        row.cache_read_tokens.hash(&mut hasher);
        row.cache_creation_tokens.hash(&mut hasher);
        row.input_token_semantics.hash(&mut hasher);
        row.total_cost_usd.to_bits().hash(&mut hasher);
        row.latency_ms.hash(&mut hasher);
        row.first_token_ms.hash(&mut hasher);
        row.duration_ms.hash(&mut hasher);
        hash_optional_str_prefix(row.session_id.as_deref(), &mut hasher);
        hash_optional_str_prefix(row.provider_type.as_deref(), &mut hasher);
        row.is_streaming.hash(&mut hasher);
        hash_optional_str_prefix(row.error_message.as_deref(), &mut hasher);
        row.error_message_truncated.hash(&mut hasher);
        hash_optional_str_prefix(row.data_source.as_deref(), &mut hasher);
        row.text_truncation_mask().hash(&mut hasher);
    }
    hasher.finish()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PendingUsageLogPage {
    request_id: u64,
    direction: crate::cli::tui::data::UsageLogPageDirection,
    load: Option<UsageLogPageRequest>,
    refresh: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct UsageLogPageRequest {
    pub(crate) cursor: crate::cli::tui::data::UsageLogCursor,
    pub(crate) direction: crate::cli::tui::data::UsageLogPageDirection,
}

#[derive(Debug, Clone)]
pub(crate) struct UsageLogPager {
    source: Option<UsageLogPageSource>,
    first_page: Vec<crate::cli::tui::data::UsageLogRow>,
    pub(crate) gate: super::paged_list::PagedListState,
    pages: HashMap<usize, crate::cli::tui::data::UsageLogPage>,
    page_loads: HashMap<usize, UsageLogPageRequest>,
    lru: std::collections::VecDeque<usize>,
    pending: HashMap<usize, PendingUsageLogPage>,
    errors: HashMap<usize, String>,
    blocked_boundary_gesture: Option<(usize, crate::cli::tui::input::WheelGestureId)>,
}

impl Default for UsageLogPager {
    fn default() -> Self {
        Self {
            source: None,
            first_page: Vec::new(),
            gate: super::paged_list::PagedListState::new(
                crate::cli::tui::data::USAGE_LOG_PAGE_SIZE,
                0,
            ),
            pages: HashMap::new(),
            page_loads: HashMap::new(),
            lru: std::collections::VecDeque::new(),
            pending: HashMap::new(),
            errors: HashMap::new(),
            blocked_boundary_gesture: None,
        }
    }
}

impl UsageLogPager {
    pub(crate) fn sync_source(
        &mut self,
        app_type: &AppType,
        range: crate::cli::tui::data::UsageRangePreset,
        first_page: &[crate::cli::tui::data::UsageLogRow],
        total: u64,
    ) -> bool {
        let first_page = &first_page[..first_page
            .len()
            .min(crate::cli::tui::data::USAGE_LOG_PAGE_SIZE)];
        let total = usize::try_from(total).unwrap_or(usize::MAX);
        let first_page_cursor = first_page
            .last()
            .map(crate::cli::tui::data::UsageLogCursor::from_row);
        let source = UsageLogPageSource {
            app_type: app_type.clone(),
            range,
            first_page_cursor,
            first_page_fingerprint: usage_log_rows_fingerprint(first_page),
            total,
        };
        if self.source.as_ref() == Some(&source) {
            return false;
        }

        let same_context = self.source.as_ref().is_some_and(|current| {
            current.app_type == source.app_type && current.range == source.range
        });
        // A refreshed aggregate can move the keyset head while the user is on
        // a later page. Keep that browsing snapshot intact until they return to
        // page zero; mixing a new head with old cursor pages would otherwise
        // both jump the selection and produce gaps/duplicates.
        if same_context && self.current_page() > 0 {
            return false;
        }

        let selected = same_context.then(|| {
            let selected = self.gate.selected_index().unwrap_or(0);
            let local = selected % self.gate.page_size();
            self.first_page
                .get(local)
                .and_then(|selected_row| {
                    first_page
                        .iter()
                        .position(|row| row.cursor_rowid == selected_row.cursor_rowid)
                })
                .unwrap_or_else(|| local.min(first_page.len().saturating_sub(1)))
        });

        self.source = Some(source);
        self.first_page = first_page.to_vec();
        self.pages.clear();
        self.page_loads.clear();
        self.lru.clear();
        self.pending.clear();
        self.errors.clear();
        self.blocked_boundary_gesture = None;
        let selected = (total > 0).then_some(selected.unwrap_or(0));
        self.gate.reset(total, selected);
        true
    }

    pub(crate) fn invalidate_source(&mut self) {
        self.source = None;
        self.first_page.clear();
        self.pages.clear();
        self.page_loads.clear();
        self.lru.clear();
        self.pending.clear();
        self.errors.clear();
        self.blocked_boundary_gesture = None;
        self.gate.reset(0, None);
    }

    pub(crate) fn source_matches(
        &self,
        app_type: &AppType,
        range: crate::cli::tui::data::UsageRangePreset,
    ) -> bool {
        self.source
            .as_ref()
            .is_some_and(|source| source.app_type == *app_type && source.range == range)
    }

    pub(crate) fn current_page(&self) -> usize {
        self.gate.page_index().unwrap_or(0)
    }

    pub(crate) fn current_rows<'a>(
        &'a self,
        first_page: &'a [crate::cli::tui::data::UsageLogRow],
    ) -> &'a [crate::cli::tui::data::UsageLogRow] {
        let page = self.current_page();
        if page == 0 {
            if self.source.is_some() {
                &self.first_page
            } else {
                first_page
            }
        } else {
            self.pages
                .get(&page)
                .map(|loaded| loaded.rows.as_slice())
                .unwrap_or(&[])
        }
    }

    pub(crate) fn page_is_available(&self, page: usize) -> bool {
        page == 0 || self.pages.contains_key(&page)
    }

    pub(crate) fn page_is_pending(&self, page: usize) -> bool {
        self.pending.contains_key(&page)
    }

    pub(crate) fn has_refresh_pending(&self) -> bool {
        self.pending.values().any(|pending| pending.refresh)
    }

    pub(crate) fn page_error(&self, page: usize) -> Option<&str> {
        self.errors.get(&page).map(String::as_str)
    }

    pub(crate) fn clear_page_error(&mut self, page: usize) {
        self.errors.remove(&page);
    }

    pub(crate) fn block_boundary_gesture(
        &mut self,
        page: usize,
        gesture: crate::cli::tui::input::WheelGestureId,
    ) {
        self.blocked_boundary_gesture = Some((page, gesture));
    }

    pub(crate) fn boundary_gesture_is_blocked(
        &self,
        page: usize,
        gesture: crate::cli::tui::input::WheelGestureId,
    ) -> bool {
        self.blocked_boundary_gesture == Some((page, gesture))
    }

    pub(crate) fn clear_blocked_boundary_gesture(&mut self) {
        self.blocked_boundary_gesture = None;
    }

    pub(crate) fn page_request(&self, page: usize) -> Option<UsageLogPageRequest> {
        if page == 0
            || page >= self.gate.page_count()
            || self.page_is_available(page)
            || self.page_is_pending(page)
            || self.page_error(page).is_some()
            || self.pending.len() >= USAGE_LOG_MAX_PENDING_PAGES
        {
            return None;
        }

        let current = self.current_page();
        let current_rows = if current == 0 {
            self.source.as_ref()?;
            self.first_page.as_slice()
        } else {
            self.pages.get(&current)?.rows.as_slice()
        };
        if page == current.checked_add(1)? {
            return current_rows.last().map(|row| UsageLogPageRequest {
                cursor: crate::cli::tui::data::UsageLogCursor::from_row(row),
                direction: crate::cli::tui::data::UsageLogPageDirection::Older,
            });
        }
        if page.checked_add(1) == Some(current) {
            return current_rows.first().map(|row| UsageLogPageRequest {
                cursor: crate::cli::tui::data::UsageLogCursor::from_row(row),
                direction: crate::cli::tui::data::UsageLogPageDirection::Newer,
            });
        }
        None
    }

    pub(crate) fn preferred_prefetch_page(&self) -> Option<usize> {
        use super::paged_list::{PageBoundary, PagedListFocus};

        let current = self.current_page();
        let boundary_target = match self.gate.focus() {
            PagedListFocus::Boundary(PageBoundary::Next) => current.checked_add(1),
            PagedListFocus::Boundary(PageBoundary::Previous) => current.checked_sub(1),
            PagedListFocus::Empty | PagedListFocus::Row => None,
        };
        if let Some(page) = boundary_target.filter(|page| self.page_request(*page).is_some()) {
            return Some(page);
        }

        // Speculatively prepare only the forward page. With a two-page cache,
        // eagerly filling both neighbours would evict them in alternation and
        // issue a database query on every tick. A previous page is loaded on
        // demand as soon as its boundary receives focus.
        current
            .checked_add(1)
            .filter(|page| self.page_request(*page).is_some())
    }

    #[cfg(test)]
    pub(crate) fn start_request(
        &mut self,
        page: usize,
        request_id: u64,
        direction: crate::cli::tui::data::UsageLogPageDirection,
    ) -> bool {
        self.start_request_inner(page, request_id, direction, None)
    }

    pub(crate) fn start_load_request(
        &mut self,
        page: usize,
        request_id: u64,
        load: UsageLogPageRequest,
    ) -> bool {
        self.start_request_inner(page, request_id, load.direction, Some(load))
    }

    fn start_request_inner(
        &mut self,
        page: usize,
        request_id: u64,
        direction: crate::cli::tui::data::UsageLogPageDirection,
        load: Option<UsageLogPageRequest>,
    ) -> bool {
        if self.pending.contains_key(&page) || self.pending.len() >= USAGE_LOG_MAX_PENDING_PAGES {
            return false;
        }
        let refresh = self.pages.contains_key(&page);
        self.errors.remove(&page);
        self.pending.insert(
            page,
            PendingUsageLogPage {
                request_id,
                direction,
                load,
                refresh,
            },
        );
        true
    }

    /// Re-run the exact keyset query that originally populated the visible
    /// page. Keeping the original anchor avoids mixing a refreshed head with
    /// old page cursors, and still works when the preceding page was evicted.
    pub(crate) fn current_page_refresh_request(&self) -> Option<UsageLogPageRequest> {
        let page = self.current_page();
        if page == 0 || self.page_is_pending(page) || !self.pages.contains_key(&page) {
            return None;
        }
        self.page_loads.get(&page).copied()
    }

    pub(crate) fn request_is_refresh(
        &self,
        page: usize,
        request_id: u64,
        direction: crate::cli::tui::data::UsageLogPageDirection,
    ) -> bool {
        self.pending.get(&page).is_some_and(|pending| {
            pending.request_id == request_id && pending.direction == direction && pending.refresh
        })
    }

    pub(crate) fn fail_request(
        &mut self,
        page: usize,
        request_id: u64,
        direction: crate::cli::tui::data::UsageLogPageDirection,
        error: String,
    ) -> bool {
        if !self.request_matches(page, request_id, direction) {
            return false;
        }
        self.pending.remove(&page);
        self.errors.insert(page, error);
        self.prune_errors();
        true
    }

    /// Drop an obsolete in-flight request without turning it into a user-facing
    /// error. A fresh request can then be queued against the current data
    /// generation on the next loop iteration.
    pub(crate) fn cancel_request(
        &mut self,
        page: usize,
        request_id: u64,
        direction: crate::cli::tui::data::UsageLogPageDirection,
    ) -> bool {
        if !self.request_matches(page, request_id, direction) {
            return false;
        }
        self.pending.remove(&page);
        true
    }

    pub(crate) fn finish_request(
        &mut self,
        page: usize,
        request_id: u64,
        direction: crate::cli::tui::data::UsageLogPageDirection,
        mut loaded: crate::cli::tui::data::UsageLogPage,
    ) -> bool {
        if !self.request_matches(page, request_id, direction) {
            return false;
        }
        let pending = self
            .pending
            .remove(&page)
            .expect("matching usage log request must still be pending");
        let load = pending.load;
        self.errors.remove(&page);

        // The user can navigate to an already-cached neighbour while a manual
        // refresh is in flight. That result belongs to the page that was
        // visible when `r` was pressed; applying it after navigation would
        // discard the new current page's cache. Keep the coherent old snapshot
        // instead and let a later refresh target the page now on screen.
        if pending.refresh && self.current_page() != page {
            return true;
        }

        if loaded.rows.len() > self.gate.page_size() {
            loaded.rows.truncate(self.gate.page_size());
            loaded.has_more = true;
            loaded.next_cursor = match direction {
                crate::cli::tui::data::UsageLogPageDirection::Older => loaded.rows.last(),
                crate::cli::tui::data::UsageLogPageDirection::Newer => loaded.rows.first(),
            }
            .map(crate::cli::tui::data::UsageLogCursor::from_row);
        }

        if matches!(
            direction,
            crate::cli::tui::data::UsageLogPageDirection::Older
        ) {
            if loaded.has_more {
                // Counts are a point-in-time estimate and can lag an upstream
                // history sync. Seeing older rows is stronger evidence: expose
                // one logical row on the following page so it can be requested.
                let known_lower_bound = page
                    .saturating_add(1)
                    .saturating_mul(self.gate.page_size())
                    .saturating_add(1);
                if known_lower_bound > self.gate.len() {
                    self.gate.sync_len(known_lower_bound);
                }
            } else {
                let loaded_total = page
                    .saturating_mul(self.gate.page_size())
                    .saturating_add(loaded.rows.len());
                // Only an older-direction terminal page proves the history end.
                // A newer-direction page ends at the head and must never shrink
                // the total tail length.
                self.gate.sync_len(loaded_total);
            }
        }

        // A refreshed page can have a different tail cursor. Drop neighbouring
        // cached pages so the next navigation rebuilds them from this page,
        // rather than mixing two keyset snapshots and creating gaps/duplicates.
        if pending.refresh {
            self.pages.retain(|cached_page, _| *cached_page == page);
            self.page_loads
                .retain(|cached_page, _| *cached_page == page);
            self.lru.retain(|cached_page| *cached_page == page);
            self.pending.clear();
            self.errors.clear();
            self.blocked_boundary_gesture = None;
        }

        self.pages.insert(page, loaded);
        if let Some(load) = load {
            self.page_loads.insert(page, load);
        } else {
            self.page_loads.remove(&page);
        }
        self.touch(page);
        self.evict_rows();
        true
    }

    fn request_matches(
        &self,
        page: usize,
        request_id: u64,
        direction: crate::cli::tui::data::UsageLogPageDirection,
    ) -> bool {
        self.pending.get(&page).is_some_and(|pending| {
            pending.request_id == request_id && pending.direction == direction
        })
    }

    fn prune_errors(&mut self) {
        let current = self.current_page();
        while self.errors.len() > USAGE_LOG_MAX_ERROR_PAGES {
            let Some(page) = self
                .errors
                .keys()
                .copied()
                .max_by_key(|page| page.abs_diff(current))
            else {
                break;
            };
            self.errors.remove(&page);
        }
    }

    fn touch(&mut self, page: usize) {
        self.lru.retain(|cached| *cached != page);
        self.lru.push_back(page);
    }

    fn evict_rows(&mut self) {
        let current = self.current_page();
        while self.pages.len() > USAGE_LOG_CACHE_PAGES {
            let position = self
                .lru
                .iter()
                .position(|page| {
                    *page != current
                        && page.abs_diff(current) > 1
                        && !self.pending.contains_key(page)
                })
                .or_else(|| {
                    self.lru
                        .iter()
                        .position(|page| *page != current && !self.pending.contains_key(page))
                });
            let page = position
                .and_then(|position| self.lru.remove(position))
                .or_else(|| {
                    self.pages
                        .keys()
                        .copied()
                        .find(|page| *page != current && !self.pending.contains_key(page))
                });
            let Some(page) = page else {
                break;
            };
            self.pages.remove(&page);
            self.page_loads.remove(&page);
            self.lru.retain(|cached| *cached != page);
        }
    }

    #[cfg(test)]
    pub(crate) fn cache_metrics(&self) -> (usize, usize, usize, usize) {
        (
            self.pages.len(),
            self.lru.len(),
            self.pending.len(),
            self.errors.len(),
        )
    }

    #[cfg(test)]
    pub(crate) fn retained_row_count(&self, ui_head_rows: usize, has_detail: bool) -> usize {
        ui_head_rows
            .saturating_add(self.first_page.len())
            .saturating_add(
                self.pages
                    .values()
                    .map(|page| page.rows.len())
                    .sum::<usize>(),
            )
            .saturating_add(usize::from(has_detail))
    }
}

#[derive(Debug, Clone)]
pub(crate) struct UsageLogDetailSnapshot {
    app_type: AppType,
    range: crate::cli::tui::data::UsageRangePreset,
    row: crate::cli::tui::data::UsageLogRow,
}

impl UsageLogDetailSnapshot {
    fn new(
        app_type: AppType,
        range: crate::cli::tui::data::UsageRangePreset,
        row: crate::cli::tui::data::UsageLogRow,
    ) -> Self {
        Self {
            app_type,
            range,
            row,
        }
    }

    pub(crate) fn row_for(
        &self,
        app_type: &AppType,
        range: crate::cli::tui::data::UsageRangePreset,
        rowid: i64,
    ) -> Option<&crate::cli::tui::data::UsageLogRow> {
        (self.app_type == *app_type && self.range == range && self.row.cursor_rowid == rowid)
            .then_some(&self.row)
    }
}

#[derive(Debug, Clone)]
pub struct UsageState {
    pub range: crate::cli::tui::data::UsageRangePreset,
    pub metric: UsageMetric,
    pub pane: UsagePane,
    pub selected_idx: usize,
    pub logs_idx: usize,
    pub(crate) log_pager: UsageLogPager,
    pub(crate) log_detail_snapshot: Option<UsageLogDetailSnapshot>,
    log_detail_pending: Option<u64>,
    refresh_log_page_after_aggregate: bool,
    manual_session_refreshing: bool,
    loading_ranges: HashSet<(AppType, crate::cli::tui::data::UsageRangePreset)>,
}

impl Default for UsageState {
    fn default() -> Self {
        Self {
            range: crate::cli::tui::data::UsageRangePreset::SevenDays,
            metric: UsageMetric::Cost,
            pane: UsagePane::Models,
            selected_idx: 0,
            logs_idx: 0,
            log_pager: UsageLogPager::default(),
            log_detail_snapshot: None,
            log_detail_pending: None,
            refresh_log_page_after_aggregate: false,
            manual_session_refreshing: false,
            loading_ranges: HashSet::new(),
        }
    }
}

impl UsageState {
    pub(crate) fn sync_log_pager(
        &mut self,
        app_type: &AppType,
        range: crate::cli::tui::data::UsageRangePreset,
        first_page: &[crate::cli::tui::data::UsageLogRow],
        total: u64,
    ) {
        let source_changed = self
            .log_pager
            .sync_source(app_type, range, first_page, total);
        self.finish_log_pager_sync(source_changed, app_type, range, first_page);
    }

    fn finish_log_pager_sync(
        &mut self,
        source_changed: bool,
        app_type: &AppType,
        range: crate::cli::tui::data::UsageRangePreset,
        first_page: &[crate::cli::tui::data::UsageLogRow],
    ) {
        if source_changed {
            self.refresh_log_detail_snapshot(app_type, range, first_page);
        }
        let current_len = self.log_pager.current_rows(first_page).len();
        let selected = self.log_pager.gate.selected_index().unwrap_or(0);
        self.logs_idx = if current_len == 0 {
            0
        } else {
            (selected % self.log_pager.gate.page_size()).min(current_len - 1)
        };
    }

    pub(crate) fn invalidate_log_pages(&mut self) {
        self.log_pager.invalidate_source();
        self.log_detail_snapshot = None;
        self.log_detail_pending = None;
        self.refresh_log_page_after_aggregate = false;
        self.logs_idx = 0;
    }

    pub(crate) fn request_log_page_refresh_after_aggregate(&mut self) {
        self.refresh_log_page_after_aggregate = true;
    }

    pub(crate) fn log_page_refresh_after_aggregate_requested(&self) -> bool {
        self.refresh_log_page_after_aggregate
    }

    pub(crate) fn finish_log_page_refresh_after_aggregate(&mut self) {
        self.refresh_log_page_after_aggregate = false;
    }

    pub(crate) fn start_manual_session_refresh(&mut self) {
        self.manual_session_refreshing = true;
    }

    pub(crate) fn finish_manual_session_refresh(&mut self) {
        self.manual_session_refreshing = false;
    }

    pub(crate) fn manual_session_refreshing(&self) -> bool {
        self.manual_session_refreshing
    }

    pub(crate) fn sync_current_log_selection(
        &mut self,
        first_page: &[crate::cli::tui::data::UsageLogRow],
    ) {
        let current_len = self.log_pager.current_rows(first_page).len();
        if current_len == 0 {
            self.logs_idx = 0;
            return;
        }

        let selected = self.log_pager.gate.selected_index().unwrap_or(0);
        self.logs_idx = (selected % self.log_pager.gate.page_size()).min(current_len - 1);
        let page_start = self
            .log_pager
            .current_page()
            .saturating_mul(self.log_pager.gate.page_size());
        self.log_pager
            .gate
            .select(page_start.saturating_add(self.logs_idx));
    }

    pub(crate) fn remember_log_detail(
        &mut self,
        app_type: AppType,
        range: crate::cli::tui::data::UsageRangePreset,
        row: crate::cli::tui::data::UsageLogRow,
    ) {
        self.log_detail_snapshot = Some(UsageLogDetailSnapshot::new(app_type, range, row));
    }

    pub(crate) fn start_log_detail_refresh(&mut self, request_id: u64) {
        self.log_detail_pending = Some(request_id);
    }

    pub(crate) fn finish_log_detail_refresh(&mut self, request_id: u64) -> bool {
        if self.log_detail_pending != Some(request_id) {
            return false;
        }
        self.log_detail_pending = None;
        true
    }

    fn refresh_log_detail_snapshot(
        &mut self,
        app_type: &AppType,
        range: crate::cli::tui::data::UsageRangePreset,
        first_page: &[crate::cli::tui::data::UsageLogRow],
    ) {
        let Some(snapshot) = &self.log_detail_snapshot else {
            return;
        };
        let rowid = snapshot.row.cursor_rowid;
        let replacement = snapshot
            .row_for(app_type, range, rowid)
            .and_then(|_| {
                self.log_pager
                    .current_rows(first_page)
                    .iter()
                    .find(|row| row.cursor_rowid == rowid)
            })
            .cloned();
        if let Some(row) = replacement {
            self.log_detail_snapshot =
                Some(UsageLogDetailSnapshot::new(app_type.clone(), range, row));
        }
    }

    pub(crate) fn start_loading(
        &mut self,
        app_type: AppType,
        range: crate::cli::tui::data::UsageRangePreset,
    ) {
        self.loading_ranges.insert((app_type, range));
    }

    pub(crate) fn finish_loading(
        &mut self,
        app_type: &AppType,
        range: crate::cli::tui::data::UsageRangePreset,
    ) {
        self.loading_ranges.remove(&(app_type.clone(), range));
    }

    pub(crate) fn clear_loading(&mut self) {
        self.loading_ranges.clear();
    }

    pub(crate) fn clear_custom_loading_for_app(&mut self, app_type: &AppType) {
        self.loading_ranges.retain(|(loading_app_type, range)| {
            loading_app_type != app_type
                || !matches!(range, crate::cli::tui::data::UsageRangePreset::Custom(_))
        });
    }

    pub(crate) fn is_loading_for(
        &self,
        app_type: &AppType,
        range: crate::cli::tui::data::UsageRangePreset,
    ) -> bool {
        self.loading_ranges
            .iter()
            .any(|(loading_app_type, loading_range)| {
                loading_app_type == app_type && usage_loading_range_matches(*loading_range, range)
            })
    }
}

fn usage_loading_range_matches(
    loading_range: crate::cli::tui::data::UsageRangePreset,
    active_range: crate::cli::tui::data::UsageRangePreset,
) -> bool {
    match (loading_range, active_range) {
        (
            crate::cli::tui::data::UsageRangePreset::Custom(loading),
            crate::cli::tui::data::UsageRangePreset::Custom(active),
        ) => loading == active,
        (crate::cli::tui::data::UsageRangePreset::Custom(_), _) => false,
        (_, crate::cli::tui::data::UsageRangePreset::Custom(_)) => false,
        _ => true,
    }
}

#[derive(Debug, Clone, Default)]
pub struct PricingState {
    pub selected_idx: usize,
}

const SESSION_PAGE_CACHE_PAGES: usize = 2;

/// Identifies one immutable, generation-pinned Sessions page source.
///
/// Every asynchronous page result carries this token. Scope changes, filter
/// changes and manifest publications each advance one component, so an old
/// worker can never populate the currently-visible page accidentally.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct SessionPageToken {
    pub(crate) scope_epoch: u64,
    pub(crate) view_epoch: u64,
    pub(crate) source: SessionPageSource,
    pub(crate) scope: String,
    pub(crate) generation: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum SessionPageSource {
    Base,
    Query,
}

#[derive(Debug, Clone)]
pub(crate) struct SessionBaseManifest {
    pub(crate) scope_epoch: u64,
    pub(crate) scope: String,
    pub(crate) generation: String,
    pub(crate) build_epoch: u64,
    pub(crate) total_rows: usize,
    pub(crate) reader: crate::session_manager::paged_manifest::ManifestReader,
}

#[derive(Debug, Clone)]
pub(crate) struct SessionProjectCatalogCache {
    pub(crate) scope_epoch: u64,
    pub(crate) scope: String,
    pub(crate) base_generation: String,
    pub(crate) catalog:
        std::sync::Arc<crate::session_manager::project_scope::SessionProjectCatalog>,
}

#[derive(Debug, Clone)]
struct CachedSessionPage {
    rows: Vec<crate::session_manager::SessionMeta>,
}

#[derive(Debug, Clone)]
struct PendingSessionPageCross {
    page: usize,
    previous_gate: super::paged_list::PagedListState,
    wheel_gesture: Option<crate::cli::tui::input::WheelGestureId>,
}

/// A newly-published source waits here while a worker locates the stable
/// selection in bounded disk pages. The old page remains interactive until the
/// located page is ready, avoiding a detail/selection mismatch during refresh.
#[derive(Debug, Clone)]
pub(crate) struct PendingSessionManifest {
    pub(crate) scope_epoch: u64,
    /// Refresh request whose publication is waiting for UI reconciliation.
    /// Purges and query/base restores may supersede it while retaining this ID
    /// so a late completion never clears a newer refresh's loading state.
    origin_scan_request_id: Option<u64>,
    pub(crate) request_id: Option<u64>,
    pub(crate) source: SessionPageSource,
    pub(crate) generation: String,
    pub(crate) total_rows: usize,
    pub(crate) reader: crate::session_manager::paged_manifest::ManifestReader,
    pub(crate) anchor: Option<SessionRowIdentity>,
    pub(crate) fallback_absolute: usize,
    clear_tombstones_on_install: HashMap<String, u64>,
}

#[derive(Debug, Clone)]
struct PurgeTombstoneRevision {
    request_id: u64,
    /// A deletion can be visible in its provider scope and in the synthetic
    /// `all` scope. Each scope is released independently only after a
    /// post-delete manifest for that scope is known safe.
    pending_scopes: HashSet<String>,
}

const MAX_SESSION_UI_TOMBSTONES: usize = 4_096;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SessionRowIdentity {
    pub(crate) provider_id: String,
    pub(crate) session_id: String,
    pub(crate) source_path: Option<String>,
}

impl SessionRowIdentity {
    pub(crate) fn capture(row: &crate::session_manager::SessionMeta) -> Self {
        Self {
            provider_id: row.provider_id.clone(),
            session_id: row.session_id.clone(),
            source_path: row.source_path.clone(),
        }
    }

    pub(crate) fn matches(&self, row: &crate::session_manager::SessionMeta) -> bool {
        self.provider_id == row.provider_id
            && self.session_id == row.session_id
            && self.source_path == row.source_path
    }
}

/// Bounded page cache for the Sessions list. `SessionsState::rows` owns the
/// active page and this cache owns at most two adjacent/off-screen pages, so the
/// UI retains at most three logical pages (300 rows) regardless of history size.
#[derive(Debug, Clone, Default)]
pub(crate) struct SessionRemotePager {
    token: Option<SessionPageToken>,
    active_reader: Option<crate::session_manager::paged_manifest::ManifestReader>,
    current_page: usize,
    total_rows: usize,
    cache: HashMap<usize, CachedSessionPage>,
    lru: std::collections::VecDeque<usize>,
    pending: HashMap<usize, u64>,
    errors: HashMap<usize, String>,
    pending_cross: Option<PendingSessionPageCross>,
    /// Last boundary page that failed after the gate was restored. Keeping this
    /// separate from `pending_cross` leaves the list interactive and lets the
    /// same explicit boundary action retry the load.
    failed_page: Option<usize>,
    request_seq: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SessionMaterializationFailure {
    scope_epoch: u64,
    scope: String,
    base_generation: String,
    view: crate::session_manager::project_scope::SessionViewSpec,
}

#[derive(Debug, Clone)]
pub struct SessionsState {
    pub provider_id: Option<String>,
    pub(crate) scope_epoch: u64,
    pub(crate) view_epoch: u64,
    pub time_anchor_ms: i64,
    /// The active manifest page only. This Vec is never allowed to exceed 100
    /// rows; adjacent pages live in `remote` under a two-page hard cap.
    pub rows: Vec<crate::session_manager::SessionMeta>,
    pub selected_idx: usize,
    pub(crate) pagination: super::paged_list::PagedListState,
    pub(crate) remote: SessionRemotePager,
    pub(crate) pending_manifest: Option<PendingSessionManifest>,
    pub(crate) base_manifest: Option<SessionBaseManifest>,
    pub project_scope: crate::session_manager::project_scope::SessionProjectScope,
    pub(crate) materialized_view: Option<crate::session_manager::project_scope::SessionViewSpec>,
    /// Base generation from which the installed query manifest was built.
    /// A matching project/query is still stale after a newer base is published.
    materialized_base_generation: Option<String>,
    /// Terminal failure for one exact desired view over one exact base. The
    /// scheduler must not retry this identity on every UI loop; a new view,
    /// base generation, or explicit refresh naturally creates a new attempt.
    materialization_failure: Option<SessionMaterializationFailure>,
    pub(crate) query_namespace: crate::session_manager::paged_manifest::QueryManifestNamespace,
    pub(crate) project_catalog: Option<SessionProjectCatalogCache>,
    pub project_catalog_loading: bool,
    pub project_catalog_error: Option<String>,
    project_catalog_seq: u64,
    pub(crate) project_catalog_active: Option<u64>,
    project_filter_seq: u64,
    pub(crate) project_filter_active: Option<u64>,
    /// Deletes acknowledged while a query generation remains visible. They are
    /// cleared only after that exact query is rebuilt from the newest base.
    query_tombstones_to_clear: HashMap<String, u64>,
    manifest_reconcile_seq: u64,
    pub(crate) rows_revision: u64,
    pub(crate) visibility_cache: std::cell::RefCell<super::helpers::SessionVisibilityCache>,
    pub pane: SessionsPane,
    pub message_idx: usize,
    pub loading: bool,
    pub loaded_once: bool,
    pub last_error: Option<String>,
    pub scan_seq: u64,
    pub scan_active: Option<u64>,
    /// Scope of the most recent scan attempt, successful or failed. The event
    /// loop uses this terminal marker to avoid restarting a failed automatic
    /// entry scan on every tick. Explicit refresh still calls `start_scan`
    /// directly, and a different scope naturally records a new attempt.
    scan_attempted_scope: Option<String>,
    /// Whether the active scan may replace the visible rows with bounded
    /// progressive previews. A manual refresh keeps an already-authoritative
    /// list on screen and ignores previews, avoiding any O(N) retain/sort on the
    /// UI thread.
    scan_accepts_previews: bool,
    /// True only when `rows` came from an accepted successful `ScanFinished` or
    /// an in-memory authoritative restore. Cached snapshots, progressive rows,
    /// and failed partial scans must never enter the inactive scope cache.
    rows_authoritative: bool,
    pub detail_key: Option<String>,
    pub messages_key: Option<String>,
    pub messages: Vec<crate::session_manager::SessionMessage>,
    pub(crate) messages_revision: u64,
    pub(crate) message_visibility_cache:
        std::cell::RefCell<super::helpers::SessionMessageVisibilityCache>,
    pub message_filter: TextInput,
    pub messages_loading: bool,
    pub messages_loaded: bool,
    /// True when the detail pane contains only the bounded prefix/window of a
    /// larger transcript.
    pub messages_truncated: bool,
    pub messages_error: Option<String>,
    pub message_seq: u64,
    pub message_active: Option<u64>,
    /// UI mutations happen before their resulting `Action` reaches the runtime
    /// dispatcher. Remember that a detail close invalidated an in-flight read
    /// so the dispatcher can advance the independent message generation.
    message_cancel_pending: bool,
    pub delete_seq: u64,
    pub delete_active: HashSet<u64>,
    /// UI 侧 tombstone：在途扫描期间删除成功的会话键（`session_key`），用于挡住
    /// 删除前读到旧列表的 partial/finished 把已删会话放回 UI。在 `finish_scan`
    /// 终态清空（见其时序注释）。键与删除流程一致（provider:session:source_path）。
    pub scan_tombstones: HashSet<String>,
    /// Exact delete revision that owns each manifest-purge tombstone. The
    /// visible set above remains the hot-path filter; this map prevents a late
    /// purge for an earlier incarnation of the same identity from clearing a
    /// newer delete barrier.
    purge_tombstone_revisions: HashMap<String, PurgeTombstoneRevision>,
    /// Tombstones already present when a fresh scan starts. A successful
    /// manifest from that request is authoritative for these identities and
    /// may release them for the scanned scope.
    scan_tombstones_to_clear: Option<(u64, String, HashMap<String, u64>)>,
    /// A purge that could not publish the currently visible scope requests one
    /// fresh scan. The event loop consumes this flag once, avoiding a retry
    /// storm while still giving the tombstone a convergence path.
    purge_refresh_required: bool,
    /// If exact UI tombstones ever hit their fixed cap, old pinned generations
    /// for the affected scopes are treated as cache misses until a fresh scan
    /// publishes. This bounds memory without permitting a deleted row to
    /// reappear from an old page.
    invalidated_tombstone_scopes: HashSet<String>,
    /// Deep search state: query string and results from backend full-content search.
    pub deep_search_query: Option<String>,
    pub deep_search_seq: u64,
    pub deep_search_active: Option<u64>,
    pub deep_search_results: Vec<crate::session_manager::SessionSearchHit>,
    /// Pending deep search: (query, ticks since last input). When ticks >= threshold, fire search.
    pub deep_search_pending: Option<(String, u64)>,
}

impl Default for SessionsState {
    fn default() -> Self {
        Self {
            provider_id: None,
            scope_epoch: 0,
            view_epoch: 0,
            time_anchor_ms: chrono::Utc::now().timestamp_millis(),
            rows: Vec::new(),
            selected_idx: 0,
            pagination: super::paged_list::PagedListState::new(100, 0),
            remote: SessionRemotePager::default(),
            pending_manifest: None,
            base_manifest: None,
            project_scope: crate::session_manager::project_scope::SessionProjectScope::All,
            materialized_view: None,
            materialized_base_generation: None,
            materialization_failure: None,
            query_namespace:
                crate::session_manager::paged_manifest::QueryManifestNamespace::new_unique(),
            project_catalog: None,
            project_catalog_loading: false,
            project_catalog_error: None,
            project_catalog_seq: 0,
            project_catalog_active: None,
            project_filter_seq: 0,
            project_filter_active: None,
            query_tombstones_to_clear: HashMap::new(),
            manifest_reconcile_seq: 0,
            rows_revision: 0,
            visibility_cache: std::cell::RefCell::new(
                super::helpers::SessionVisibilityCache::default(),
            ),
            pane: SessionsPane::List,
            message_idx: 0,
            loading: false,
            loaded_once: false,
            last_error: None,
            scan_seq: 0,
            scan_active: None,
            scan_attempted_scope: None,
            scan_accepts_previews: false,
            rows_authoritative: false,
            detail_key: None,
            messages_key: None,
            messages: Vec::new(),
            messages_revision: 0,
            message_visibility_cache: std::cell::RefCell::new(
                super::helpers::SessionMessageVisibilityCache::default(),
            ),
            message_filter: TextInput::new(""),
            messages_loading: false,
            messages_loaded: false,
            messages_truncated: false,
            messages_error: None,
            message_seq: 0,
            message_active: None,
            message_cancel_pending: false,
            delete_seq: 0,
            delete_active: HashSet::new(),
            scan_tombstones: HashSet::new(),
            purge_tombstone_revisions: HashMap::new(),
            scan_tombstones_to_clear: None,
            purge_refresh_required: false,
            invalidated_tombstone_scopes: HashSet::new(),
            deep_search_query: None,
            deep_search_seq: 0,
            deep_search_active: None,
            deep_search_results: Vec::new(),
            deep_search_pending: None,
        }
    }
}

/// Enforce the worker/UI preview contract without ever dropping an accidentally
/// huge tail on the UI thread. Only the fixed prefix is cloned; ownership and
/// destruction of the original Vec move to a background thread.
fn bounded_session_preview(
    rows: Vec<crate::session_manager::SessionMeta>,
) -> Vec<crate::session_manager::SessionMeta> {
    let limit = crate::session_manager::SCAN_CACHE_FIRST_PAINT_LIMIT;
    if rows.len() <= limit {
        return rows;
    }
    let preview = rows.iter().take(limit).cloned().collect();
    retire_session_rows(rows);
    preview
}

/// Releasing hundreds of thousands of String-heavy rows can itself stall the
/// event loop. Small Vecs are cheaper to drop inline; large retired snapshots
/// are moved to a short-lived background thread.
pub(crate) fn retire_session_rows(rows: Vec<crate::session_manager::SessionMeta>) {
    retire_large_vec(rows, "cc-switch-session-drop");
}

pub(crate) fn retire_session_messages(messages: Vec<crate::session_manager::SessionMessage>) {
    retire_large_vec(messages, "cc-switch-session-message-drop");
}

pub(crate) fn retire_session_search_hits(hits: Vec<crate::session_manager::SessionSearchHit>) {
    retire_large_vec(hits, "cc-switch-session-search-drop");
}

fn retire_session_project_catalog(cache: SessionProjectCatalogCache) {
    retire_large_project_value(cache.catalog.projects.len(), cache);
}

fn retire_uninstalled_session_project_catalog(
    catalog: crate::session_manager::project_scope::SessionProjectCatalog,
) {
    retire_large_project_value(catalog.projects.len(), catalog);
}

/// Catalog entries own multiple path strings, so rejecting a stale worker
/// result can be just as expensive as replacing an installed cache. Keep both
/// ownership exits off the event-loop thread once the catalog is large.
fn retire_large_project_value<T: Send + 'static>(project_count: usize, value: T) {
    const BACKGROUND_DROP_THRESHOLD: usize = 4_096;
    if project_count < BACKGROUND_DROP_THRESHOLD {
        drop(value);
        return;
    }
    let _ = std::thread::Builder::new()
        .name("cc-switch-project-catalog-drop".to_string())
        .spawn(move || drop(value));
}

fn retire_large_vec<T: Send + 'static>(rows: Vec<T>, thread_name: &'static str) {
    const BACKGROUND_DROP_THRESHOLD: usize = 4_096;
    if rows.len() < BACKGROUND_DROP_THRESHOLD {
        drop(rows);
        return;
    }
    let _ = std::thread::Builder::new()
        .name(thread_name.to_string())
        .spawn(move || drop(rows));
}

impl SessionRemotePager {
    pub(crate) fn token(&self) -> Option<&SessionPageToken> {
        self.token.as_ref()
    }

    pub(crate) const fn current_page(&self) -> usize {
        self.current_page
    }

    pub(crate) const fn total_rows(&self) -> usize {
        self.total_rows
    }

    pub(crate) fn page_count(&self) -> usize {
        self.total_rows
            .div_ceil(crate::session_manager::paged_manifest::PAGE_SIZE)
    }

    pub(crate) fn is_page_pending(&self, page: usize) -> bool {
        self.pending.contains_key(&page)
    }

    pub(crate) const fn failed_page(&self) -> Option<usize> {
        self.failed_page
    }

    pub(crate) fn dismiss_page_error(&mut self) {
        if let Some(page) = self.failed_page.take() {
            self.errors.remove(&page);
        }
    }

    pub(crate) fn has_page(&self, page: usize) -> bool {
        page == self.current_page || self.cache.contains_key(&page)
    }

    pub(crate) fn reset_scope(&mut self) {
        self.token = None;
        self.active_reader = None;
        self.current_page = 0;
        self.total_rows = 0;
        self.retire_cache();
        self.pending.clear();
        self.errors.clear();
        self.pending_cross = None;
        self.failed_page = None;
    }

    pub(crate) fn install_source(
        &mut self,
        token: SessionPageToken,
        reader: crate::session_manager::paged_manifest::ManifestReader,
        total_rows: usize,
        page_index: usize,
        rows: Vec<crate::session_manager::SessionMeta>,
        selected_absolute: usize,
        active_rows: &mut Vec<crate::session_manager::SessionMeta>,
        selected_idx: &mut usize,
        gate: &mut super::paged_list::PagedListState,
    ) {
        debug_assert!(rows.len() <= crate::session_manager::paged_manifest::PAGE_SIZE);
        self.reset_scope();
        self.token = Some(token);
        self.active_reader = Some(reader);
        self.total_rows = total_rows;
        self.current_page = page_index;
        retire_session_rows(std::mem::replace(active_rows, rows));
        let page_start =
            page_index.saturating_mul(crate::session_manager::paged_manifest::PAGE_SIZE);
        let selected_absolute = if total_rows == 0 {
            0
        } else {
            selected_absolute
                .max(page_start)
                .min(page_start.saturating_add(active_rows.len().saturating_sub(1)))
                .min(total_rows - 1)
        };
        *selected_idx = selected_absolute.saturating_sub(page_start);
        gate.reset(total_rows, (total_rows > 0).then_some(selected_absolute));
    }

    pub(crate) fn next_request(
        &mut self,
        page: usize,
    ) -> Option<(
        u64,
        SessionPageToken,
        crate::session_manager::paged_manifest::ManifestReader,
    )> {
        let token = self.token.clone()?;
        let reader = self.active_reader.as_ref()?;
        if reader.scope() != token.scope || reader.generation() != token.generation {
            return None;
        }
        if page >= self.page_count()
            || self.has_page(page)
            || self.pending.contains_key(&page)
            || self.errors.contains_key(&page)
        {
            return None;
        }
        self.request_seq = self.request_seq.wrapping_add(1);
        let request_id = self.request_seq;
        self.pending.insert(page, request_id);
        self.errors.remove(&page);
        Some((request_id, token, reader.clone()))
    }

    pub(crate) fn start_cross(
        &mut self,
        page: usize,
        previous_gate: super::paged_list::PagedListState,
        wheel_gesture: Option<crate::cli::tui::input::WheelGestureId>,
    ) {
        if !self.has_page(page) {
            self.errors.remove(&page);
            self.failed_page = None;
            self.pending_cross = Some(PendingSessionPageCross {
                page,
                previous_gate,
                wheel_gesture,
            });
        }
    }

    pub(crate) fn input_is_blocked(&self) -> bool {
        self.pending_cross.is_some()
    }

    pub(crate) fn pending_cross_page(&self) -> Option<usize> {
        self.pending_cross.as_ref().map(|cross| cross.page)
    }

    fn cancel_cross(
        &mut self,
        direction: super::paged_list::PageDirection,
        gate: &mut super::paged_list::PagedListState,
    ) -> bool {
        let Some(cross) = self.pending_cross.as_ref() else {
            return false;
        };
        let is_reverse = match direction {
            super::paged_list::PageDirection::Previous => cross.page > self.current_page,
            super::paged_list::PageDirection::Next => cross.page < self.current_page,
        };
        if !is_reverse {
            return false;
        }
        let cross = self.pending_cross.take().expect("checked above");
        *gate = cross.previous_gate;
        self.failed_page = None;
        true
    }

    pub(crate) fn finish_page(
        &mut self,
        request_id: u64,
        token: &SessionPageToken,
        page: usize,
        rows: Vec<crate::session_manager::SessionMeta>,
        active_rows: &mut Vec<crate::session_manager::SessionMeta>,
        selected_idx: &mut usize,
        gate: &mut super::paged_list::PagedListState,
    ) -> bool {
        if self.token.as_ref() != Some(token)
            || self.pending.get(&page).copied() != Some(request_id)
            || rows.len() > crate::session_manager::paged_manifest::PAGE_SIZE
        {
            retire_session_rows(rows);
            return false;
        }
        self.pending.remove(&page);
        self.errors.remove(&page);
        self.failed_page = None;

        if self
            .pending_cross
            .as_ref()
            .is_some_and(|cross| cross.page == page)
        {
            self.pending_cross = None;
            self.activate_page(page, rows, active_rows, selected_idx, gate);
        } else {
            self.insert_cached(page, rows);
        }
        true
    }

    pub(crate) fn fail_page(
        &mut self,
        request_id: u64,
        token: &SessionPageToken,
        page: usize,
        error: String,
        gate: &mut super::paged_list::PagedListState,
    ) -> bool {
        if self.token.as_ref() != Some(token)
            || self.pending.get(&page).copied() != Some(request_id)
        {
            return false;
        }
        self.pending.remove(&page);
        self.errors.insert(page, error);
        if self
            .pending_cross
            .as_ref()
            .is_some_and(|cross| cross.page == page)
        {
            if let Some(cross) = self.pending_cross.take() {
                *gate = cross.previous_gate;
                if let Some(gesture) = cross.wheel_gesture {
                    gate.block_wheel_gesture_after_failed_cross(gesture);
                }
            }
            self.failed_page = Some(page);
        }
        true
    }

    pub(crate) fn activate_cached(
        &mut self,
        page: usize,
        active_rows: &mut Vec<crate::session_manager::SessionMeta>,
        selected_idx: &mut usize,
        gate: &super::paged_list::PagedListState,
    ) -> bool {
        let Some(cached) = self.cache.remove(&page) else {
            return false;
        };
        self.failed_page = None;
        self.lru.retain(|cached_page| *cached_page != page);
        self.activate_page(page, cached.rows, active_rows, selected_idx, gate);
        true
    }

    fn activate_page(
        &mut self,
        page: usize,
        rows: Vec<crate::session_manager::SessionMeta>,
        active_rows: &mut Vec<crate::session_manager::SessionMeta>,
        selected_idx: &mut usize,
        gate: &super::paged_list::PagedListState,
    ) {
        let old_page = self.current_page;
        let old_rows = std::mem::replace(active_rows, rows);
        if old_page != page {
            self.insert_cached(old_page, old_rows);
        } else {
            retire_session_rows(old_rows);
        }
        self.current_page = page;
        let absolute = gate.selected_index().unwrap_or(0);
        *selected_idx = absolute
            .saturating_sub(page.saturating_mul(crate::session_manager::paged_manifest::PAGE_SIZE))
            .min(active_rows.len().saturating_sub(1));
    }

    fn insert_cached(&mut self, page: usize, rows: Vec<crate::session_manager::SessionMeta>) {
        if rows.is_empty() || page == self.current_page {
            retire_session_rows(rows);
            return;
        }
        if let Some(old) = self.cache.insert(page, CachedSessionPage { rows }) {
            retire_session_rows(old.rows);
        }
        self.lru.retain(|cached_page| *cached_page != page);
        self.lru.push_back(page);
        while self.cache.len() > SESSION_PAGE_CACHE_PAGES {
            let Some(evicted) = self.lru.pop_front() else {
                break;
            };
            if let Some(page) = self.cache.remove(&evicted) {
                retire_session_rows(page.rows);
            }
        }
    }

    fn retire_cache(&mut self) {
        for (_, page) in self.cache.drain() {
            retire_session_rows(page.rows);
        }
        self.lru.clear();
    }

    fn remove_by_key(&mut self, key: &str) -> bool {
        let mut removed = false;
        for page in self.cache.values_mut() {
            let before = page.rows.len();
            page.rows
                .retain(|row| !crate::cli::tui::app::session_key_matches(row, key));
            removed |= page.rows.len() != before;
        }
        removed
    }

    #[cfg(test)]
    pub(crate) fn retained_rows(&self, active_len: usize) -> usize {
        active_len
            + self
                .cache
                .values()
                .map(|page| page.rows.len())
                .sum::<usize>()
    }
}

impl SessionsState {
    pub(crate) fn loaded_for_provider(&self, provider_id: &str) -> bool {
        !self.scope_cache_is_invalidated(provider_id)
            && self.loaded_once
            && self.provider_id.as_deref() == Some(provider_id)
            && (self.remote.token().is_some_and(|token| {
                token.source == SessionPageSource::Base || self.materialized_view.is_some()
            }) || self
                .pending_manifest
                .as_ref()
                .is_some_and(|pending| pending.source == SessionPageSource::Base))
    }

    pub(crate) fn reset_time_anchor(&mut self) {
        self.time_anchor_ms = chrono::Utc::now().timestamp_millis();
    }

    pub(crate) fn selected_absolute(&self) -> usize {
        self.pagination.selected_index().unwrap_or(0)
    }

    /// Exact manifest size once a source is open; during the first-ever scan,
    /// fall back to the bounded provisional page so selection and footer state
    /// remain usable before publication.
    pub(crate) fn logical_total_rows(&self) -> usize {
        if self.remote.token().is_some() {
            self.remote.total_rows()
        } else {
            self.rows.len()
        }
    }

    /// Map the selected visible row back onto the immutable source page. A
    /// delete or a local filter can leave fewer than 100 visible rows while the
    /// old generation is still pinned. Its last visible row must still arm the
    /// real source-page boundary, otherwise surviving rows on the next page
    /// become unreachable until repacking finishes.
    pub(crate) fn selected_source_absolute(
        &self,
        visible_len: usize,
        toward_next_page: bool,
    ) -> usize {
        let total = self.logical_total_rows();
        if total == 0 {
            return 0;
        }
        let page_start = self
            .remote
            .current_page()
            .saturating_mul(crate::session_manager::paged_manifest::PAGE_SIZE);
        let collapsed = page_start.saturating_add(self.selected_idx);
        let source_page_end = page_start
            .saturating_add(crate::session_manager::paged_manifest::PAGE_SIZE)
            .min(total)
            .saturating_sub(1);
        if toward_next_page
            && visible_len > 0
            && self.selected_idx.saturating_add(1) >= visible_len
            && source_page_end.saturating_add(1) < total
        {
            source_page_end
        } else {
            collapsed.min(total.saturating_sub(1))
        }
    }

    pub(crate) fn selected_collapsed_absolute(&self) -> usize {
        self.remote
            .current_page()
            .saturating_mul(crate::session_manager::paged_manifest::PAGE_SIZE)
            .saturating_add(self.selected_idx)
            .min(self.logical_total_rows().saturating_sub(1))
    }

    pub(crate) fn selected_row(&self) -> Option<&crate::session_manager::SessionMeta> {
        self.rows.get(self.selected_idx)
    }

    pub(crate) fn page_token(&self) -> Option<&SessionPageToken> {
        self.remote.token()
    }

    pub(crate) fn remember_base_manifest(
        &mut self,
        scope_epoch: u64,
        scope: &str,
        generation: String,
        total_rows: usize,
        reader: crate::session_manager::paged_manifest::ManifestReader,
    ) -> bool {
        if self.scope_epoch != scope_epoch
            || self.provider_id.as_deref() != Some(scope)
            || reader.scope() != scope
            || reader.generation() != generation
        {
            return false;
        }
        let incoming_epoch = reader.build_epoch();
        if self.base_manifest.as_ref().is_some_and(|current| {
            current.scope_epoch == scope_epoch
                && current.scope == scope
                && current.build_epoch > incoming_epoch
        }) {
            return false;
        }
        let generation_changed = self.base_manifest.as_ref().is_none_or(|current| {
            current.scope_epoch != scope_epoch
                || current.scope != scope
                || current.generation != generation
        });
        self.base_manifest = Some(SessionBaseManifest {
            scope_epoch,
            scope: scope.to_string(),
            generation,
            build_epoch: incoming_epoch,
            total_rows,
            reader,
        });
        if generation_changed {
            self.retire_project_catalog();
            self.project_catalog_active = None;
            self.project_catalog_loading = false;
            self.project_catalog_error = None;
            self.project_filter_active = None;
        }
        true
    }

    pub(crate) fn base_query_source(
        &self,
    ) -> Option<(
        SessionPageToken,
        crate::session_manager::paged_manifest::ManifestReader,
    )> {
        let base = self.base_manifest.as_ref()?;
        (base.scope_epoch == self.scope_epoch).then(|| {
            (
                SessionPageToken {
                    scope_epoch: base.scope_epoch,
                    view_epoch: self.view_epoch,
                    source: SessionPageSource::Base,
                    scope: base.scope.clone(),
                    generation: base.generation.clone(),
                },
                base.reader.clone(),
            )
        })
    }

    pub(crate) fn project_catalog_is_current(&self) -> bool {
        let Some(base) = self.base_manifest.as_ref() else {
            return false;
        };
        self.project_catalog.as_ref().is_some_and(|cache| {
            cache.scope_epoch == base.scope_epoch
                && cache.scope == base.scope
                && cache.base_generation == base.generation
        })
    }

    fn retire_project_catalog(&mut self) {
        if let Some(cache) = self.project_catalog.take() {
            retire_session_project_catalog(cache);
        }
    }

    pub(crate) fn start_project_catalog(&mut self) -> Option<u64> {
        let base = self.base_manifest.as_ref()?;
        if base.scope_epoch != self.scope_epoch
            || self.provider_id.as_deref() != Some(base.scope.as_str())
        {
            return None;
        }
        self.project_catalog_seq = self.project_catalog_seq.wrapping_add(1);
        self.project_catalog_active = Some(self.project_catalog_seq);
        self.project_catalog_loading = true;
        self.project_catalog_error = None;
        Some(self.project_catalog_seq)
    }

    pub(crate) fn finish_project_catalog(
        &mut self,
        request_id: u64,
        scope_epoch: u64,
        scope: String,
        base_generation: String,
        catalog: crate::session_manager::project_scope::SessionProjectCatalog,
    ) -> bool {
        let current_base = self.base_manifest.as_ref().is_some_and(|base| {
            base.scope_epoch == scope_epoch
                && base.scope == scope
                && base.generation == base_generation
        });
        if self.project_catalog_active != Some(request_id)
            || self.scope_epoch != scope_epoch
            || !current_base
        {
            retire_uninstalled_session_project_catalog(catalog);
            return false;
        }
        self.project_catalog_active = None;
        self.project_catalog_loading = false;
        self.project_catalog_error = None;
        self.retire_project_catalog();
        self.project_catalog = Some(SessionProjectCatalogCache {
            scope_epoch,
            scope,
            base_generation,
            catalog: std::sync::Arc::new(catalog),
        });
        true
    }

    pub(crate) fn fail_project_catalog(&mut self, request_id: u64, error: String) -> bool {
        if self.project_catalog_active != Some(request_id) {
            return false;
        }
        self.project_catalog_active = None;
        self.project_catalog_loading = false;
        self.project_catalog_error = Some(error);
        true
    }

    pub(crate) fn cancel_project_catalog(&mut self) {
        // The worker intentionally drops a cancelled result. Clear the UI-side
        // request marker at the same time so a later picker open can start a
        // fresh catalog instead of waiting forever for that dropped result.
        self.project_catalog_active = None;
        self.project_catalog_loading = false;
    }

    pub(crate) fn start_project_filter(&mut self) -> Option<u64> {
        self.project_catalog_is_current().then(|| {
            self.project_filter_seq = self.project_filter_seq.wrapping_add(1);
            self.project_filter_active = Some(self.project_filter_seq);
            self.project_filter_seq
        })
    }

    pub(crate) fn finish_project_filter(
        &mut self,
        request_id: u64,
        scope_epoch: u64,
        scope: &str,
        base_generation: &str,
    ) -> bool {
        let current = self.project_catalog.as_ref().is_some_and(|cache| {
            cache.scope_epoch == scope_epoch
                && cache.scope == scope
                && cache.base_generation == base_generation
                && self.project_catalog_is_current()
        });
        if self.project_filter_active != Some(request_id) || !current {
            return false;
        }
        self.project_filter_active = None;
        true
    }

    pub(crate) fn fail_project_filter(&mut self, request_id: u64) -> bool {
        if self.project_filter_active != Some(request_id) {
            return false;
        }
        self.project_filter_active = None;
        true
    }

    pub(crate) fn cancel_project_filter(&mut self) {
        self.project_filter_active = None;
    }

    pub(crate) fn set_project_scope(
        &mut self,
        scope: crate::session_manager::project_scope::SessionProjectScope,
    ) -> bool {
        if self.project_scope == scope {
            return false;
        }
        self.project_scope = scope;
        self.deep_search_active = None;
        self.deep_search_pending = None;
        self.materialization_failure = None;
        // This is a desired-view transition only. Keep the installed manifest,
        // its bounded rows, and any pending refresh owner alive until the new
        // view is atomically installed. Local visibility applies the new scope
        // to the old page in the meantime without losing scan/tombstone state.
        self.selected_idx = 0;
        self.clear_detail();
        self.last_error = None;
        true
    }

    pub(crate) fn desired_view_spec(
        &self,
        query: Option<&str>,
    ) -> crate::session_manager::project_scope::SessionViewSpec {
        crate::session_manager::project_scope::SessionViewSpec::new(
            self.project_scope.clone(),
            query.unwrap_or_default(),
        )
    }

    pub(crate) fn desired_view_requires_materialization(&self, query: Option<&str>) -> bool {
        !self.desired_view_spec(query).is_base_view()
    }

    pub(crate) fn materialized_view_is_current(&self, query: Option<&str>) -> bool {
        let desired = self.desired_view_spec(query);
        let built_from_current_base = self.base_manifest.as_ref().is_some_and(|base| {
            self.materialized_base_generation.as_deref() == Some(base.generation.as_str())
        });
        self.remote
            .token()
            .is_some_and(|token| token.source == SessionPageSource::Query)
            && self.materialized_view.as_ref() == Some(&desired)
            && built_from_current_base
    }

    pub(crate) fn materialization_failed_for_current_base(
        &self,
        view: &crate::session_manager::project_scope::SessionViewSpec,
    ) -> bool {
        let Some(base) = self.base_manifest.as_ref() else {
            return false;
        };
        self.materialization_failure
            .as_ref()
            .is_some_and(|failure| {
                failure.scope_epoch == self.scope_epoch
                    && failure.scope_epoch == base.scope_epoch
                    && failure.scope == base.scope
                    && failure.base_generation == base.generation
                    && &failure.view == view
            })
    }

    pub(crate) fn mark_materialization_failed(
        &mut self,
        scope_epoch: u64,
        scope: &str,
        base_generation: &str,
        view: crate::session_manager::project_scope::SessionViewSpec,
    ) {
        let current_base = self.base_manifest.as_ref().is_some_and(|base| {
            self.scope_epoch == scope_epoch
                && base.scope_epoch == scope_epoch
                && base.scope == scope
                && base.generation == base_generation
        });
        if current_base {
            self.materialization_failure = Some(SessionMaterializationFailure {
                scope_epoch,
                scope: scope.to_string(),
                base_generation: base_generation.to_string(),
                view,
            });
        }
    }

    pub(crate) fn clear_materialization_failure(&mut self) {
        self.materialization_failure = None;
    }

    pub(crate) fn register_purge_tombstone(&mut self, request_id: u64, key: String) {
        let provider_scope = key
            .split_once(':')
            .map(|(provider, _)| provider)
            .filter(|provider| !provider.is_empty())
            .unwrap_or_else(|| self.provider_id.as_deref().unwrap_or("all"))
            .to_string();
        let pending_scopes = [provider_scope, "all".to_string()]
            .into_iter()
            .collect::<HashSet<_>>();

        if !self.purge_tombstone_revisions.contains_key(&key)
            && self.purge_tombstone_revisions.len() >= MAX_SESSION_UI_TOMBSTONES
        {
            for tombstone in self.purge_tombstone_revisions.values() {
                self.invalidated_tombstone_scopes
                    .extend(tombstone.pending_scopes.iter().cloned());
            }
            self.invalidated_tombstone_scopes
                .extend(pending_scopes.iter().cloned());
            self.purge_tombstone_revisions.clear();
            self.scan_tombstones.clear();
            self.invalidate_current_tombstone_scope_if_needed();
            return;
        }

        self.scan_tombstones.insert(key.clone());
        self.purge_tombstone_revisions.insert(
            key,
            PurgeTombstoneRevision {
                request_id,
                pending_scopes,
            },
        );
    }

    pub(crate) fn purge_tombstone_is_current(&self, request_id: u64, key: &str) -> bool {
        self.scan_tombstones.contains(key)
            && self
                .purge_tombstone_revisions
                .get(key)
                .is_some_and(|revision| revision.request_id == request_id)
    }

    pub(crate) fn purge_tombstone_applies_to_scope(
        &self,
        request_id: u64,
        key: &str,
        scope: &str,
    ) -> bool {
        self.purge_tombstone_revisions
            .get(key)
            .is_some_and(|revision| {
                revision.request_id == request_id && revision.pending_scopes.contains(scope)
            })
    }

    fn clear_purge_tombstone_scope(&mut self, request_id: u64, key: &str, scope: &str) -> bool {
        if !self.purge_tombstone_is_current(request_id, key) {
            return false;
        }
        let remove_identity = self
            .purge_tombstone_revisions
            .get_mut(key)
            .is_some_and(|revision| {
                revision.pending_scopes.remove(scope);
                revision.pending_scopes.is_empty()
            });
        if remove_identity {
            self.purge_tombstone_revisions.remove(key);
            self.scan_tombstones.remove(key);
        }
        true
    }

    fn tombstones_for_scope(&self, scope: &str) -> HashMap<String, u64> {
        self.purge_tombstone_revisions
            .iter()
            .filter(|(_, revision)| revision.pending_scopes.contains(scope))
            .map(|(key, revision)| (key.clone(), revision.request_id))
            .collect()
    }

    fn clear_tombstones_for_scope(&mut self, scope: &str, tombstones: HashMap<String, u64>) {
        for (key, request_id) in tombstones {
            self.clear_purge_tombstone_scope(request_id, &key, scope);
        }
        self.invalidated_tombstone_scopes.remove(scope);
    }

    pub(crate) fn clear_published_purge_scope(
        &mut self,
        request_id: u64,
        key: &str,
        scope: &str,
    ) -> bool {
        self.clear_purge_tombstone_scope(request_id, key, scope)
    }

    pub(crate) fn scope_cache_is_invalidated(&self, scope: &str) -> bool {
        self.invalidated_tombstone_scopes.contains(scope)
    }

    pub(crate) fn require_purge_refresh(&mut self) {
        self.purge_refresh_required = true;
    }

    pub(crate) fn purge_refresh_required(&self) -> bool {
        self.purge_refresh_required
    }

    pub(crate) fn consume_purge_refresh(&mut self) {
        self.purge_refresh_required = false;
    }

    pub(crate) fn take_scan_tombstones_for_manifest(
        &mut self,
        request_id: u64,
        scope: &str,
    ) -> HashMap<String, u64> {
        let matches = self.scan_tombstones_to_clear.as_ref().is_some_and(
            |(pending_request, pending_scope, _)| {
                *pending_request == request_id && pending_scope == scope
            },
        );
        if !matches {
            return HashMap::new();
        }
        self.scan_tombstones_to_clear
            .take()
            .map(|(_, _, tombstones)| tombstones)
            .unwrap_or_default()
    }

    pub(crate) fn clear_safe_manifest_tombstones(
        &mut self,
        scope: &str,
        tombstones: HashMap<String, u64>,
    ) {
        self.clear_tombstones_for_scope(scope, tombstones);
    }

    fn invalidate_current_tombstone_scope_if_needed(&mut self) {
        let Some(scope) = self.provider_id.as_deref() else {
            return;
        };
        if !self.invalidated_tombstone_scopes.contains(scope) {
            return;
        }
        self.scope_epoch = self.scope_epoch.wrapping_add(1);
        self.view_epoch = self.view_epoch.wrapping_add(1);
        self.remote.reset_scope();
        retire_session_rows(std::mem::take(&mut self.rows));
        self.rows_revision = self.rows_revision.wrapping_add(1);
        self.selected_idx = 0;
        self.pagination.reset(0, None);
        self.pending_manifest = None;
        self.base_manifest = None;
        self.materialized_view = None;
        self.materialized_base_generation = None;
        self.materialization_failure = None;
        self.scan_active = None;
        self.loading = false;
        self.loaded_once = false;
        self.clear_detail();
        self.purge_refresh_required = true;
    }

    pub(crate) fn mark_query_tombstone_for_rebuild(&mut self, request_id: u64, key: String) {
        if self.purge_tombstone_is_current(request_id, &key) {
            self.query_tombstones_to_clear.insert(key, request_id);
        }
    }

    pub(crate) fn mark_query_tombstones_for_rebuild(&mut self, tombstones: HashMap<String, u64>) {
        for (key, request_id) in tombstones {
            self.mark_query_tombstone_for_rebuild(request_id, key);
        }
    }

    pub(crate) fn apply_opened_manifest(
        &mut self,
        scope_epoch: u64,
        scope: &str,
        generation: String,
        total_rows: usize,
        page_index: usize,
        rows: Vec<crate::session_manager::SessionMeta>,
        reader: crate::session_manager::paged_manifest::ManifestReader,
    ) -> bool {
        self.apply_manifest_source(
            SessionPageSource::Base,
            None,
            scope_epoch,
            scope,
            generation,
            total_rows,
            page_index,
            rows,
            reader,
            page_index.saturating_mul(crate::session_manager::paged_manifest::PAGE_SIZE),
        )
    }

    pub(crate) fn apply_query_manifest(
        &mut self,
        scope_epoch: u64,
        scope: &str,
        base_generation: &str,
        view: crate::session_manager::project_scope::SessionViewSpec,
        generation: String,
        total_rows: usize,
        page_index: usize,
        rows: Vec<crate::session_manager::SessionMeta>,
        reader: crate::session_manager::paged_manifest::ManifestReader,
    ) -> bool {
        let base_is_current = self.base_manifest.as_ref().is_some_and(|base| {
            base.scope_epoch == scope_epoch
                && base.scope == scope
                && base.generation == base_generation
        });
        if !base_is_current {
            retire_session_rows(rows);
            return false;
        }
        let applied = self.apply_manifest_source(
            SessionPageSource::Query,
            Some(view),
            scope_epoch,
            scope,
            generation,
            total_rows,
            page_index,
            rows,
            reader,
            page_index.saturating_mul(crate::session_manager::paged_manifest::PAGE_SIZE),
        );
        if applied {
            self.materialized_base_generation = Some(base_generation.to_string());
            self.materialization_failure = None;
            // A query accepted here was materialized from the exact current
            // base generation checked above. Switching to it therefore also
            // retires any older pinned source whose post-delete cleanup was
            // waiting for selection reconciliation.
            let (pending_cleanup, superseded_scan) = if self
                .pending_manifest
                .as_ref()
                .is_some_and(|pending| pending.scope_epoch == scope_epoch)
            {
                self.pending_manifest
                    .take()
                    .map(|pending| {
                        (
                            pending.clear_tombstones_on_install,
                            pending.origin_scan_request_id,
                        )
                    })
                    .unwrap_or_default()
            } else {
                (HashMap::new(), None)
            };
            for (key, request_id) in std::mem::take(&mut self.query_tombstones_to_clear) {
                self.clear_purge_tombstone_scope(request_id, &key, scope);
            }
            for (key, request_id) in pending_cleanup {
                self.clear_purge_tombstone_scope(request_id, &key, scope);
            }
            if superseded_scan.is_some() && self.scan_active == superseded_scan {
                self.scan_active = None;
                self.loading = false;
            } else if self.scan_active.is_none() {
                self.loading = false;
            }
            self.rows_authoritative = true;
            self.invalidated_tombstone_scopes.remove(scope);
        }
        applied
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "source installation is an atomic UI transition"
    )]
    fn apply_manifest_source(
        &mut self,
        source: SessionPageSource,
        materialized_view: Option<crate::session_manager::project_scope::SessionViewSpec>,
        scope_epoch: u64,
        scope: &str,
        generation: String,
        total_rows: usize,
        page_index: usize,
        mut rows: Vec<crate::session_manager::SessionMeta>,
        reader: crate::session_manager::paged_manifest::ManifestReader,
        selected_absolute: usize,
    ) -> bool {
        if self.scope_epoch != scope_epoch
            || self.provider_id.as_deref() != Some(scope)
            || reader.scope() != scope
            || reader.generation() != generation
        {
            retire_session_rows(rows);
            return false;
        }
        if source == SessionPageSource::Base
            && self.base_manifest.as_ref().is_some_and(|base| {
                base.scope_epoch == scope_epoch
                    && base.scope == scope
                    && base.build_epoch > reader.build_epoch()
            })
        {
            retire_session_rows(rows);
            return false;
        }
        self.drop_tombstoned_rows(&mut rows);
        self.view_epoch = self.view_epoch.wrapping_add(1);
        let token = SessionPageToken {
            scope_epoch,
            view_epoch: self.view_epoch,
            source,
            scope: scope.to_string(),
            generation,
        };
        self.remote.install_source(
            token,
            reader,
            total_rows,
            page_index,
            rows,
            selected_absolute,
            &mut self.rows,
            &mut self.selected_idx,
            &mut self.pagination,
        );
        self.rows_revision = self.rows_revision.wrapping_add(1);
        self.loaded_once = true;
        self.last_error = None;
        self.materialized_view = materialized_view;
        if source == SessionPageSource::Base {
            self.materialized_base_generation = None;
            self.materialization_failure = None;
        }
        if self.detail_key.as_deref().is_some_and(|key| {
            self.rows
                .get(self.selected_idx)
                .is_none_or(|row| !crate::cli::tui::app::session_key_matches(row, key))
        }) {
            self.clear_detail();
        }
        true
    }

    pub(crate) fn apply_provisional_page(
        &mut self,
        request_id: u64,
        scope_epoch: u64,
        scope: &str,
        mut rows: Vec<crate::session_manager::SessionMeta>,
    ) -> bool {
        if self.scan_active != Some(request_id)
            || self.scope_epoch != scope_epoch
            || self.provider_id.as_deref() != Some(scope)
            || self.remote.token().is_some()
        {
            retire_session_rows(rows);
            return false;
        }
        rows.truncate(crate::session_manager::paged_manifest::PAGE_SIZE);
        self.drop_tombstoned_rows(&mut rows);
        self.replace_rows(rows);
        self.selected_idx = self.selected_idx.min(self.rows.len().saturating_sub(1));
        self.pagination.reset(
            self.rows.len(),
            (!self.rows.is_empty()).then_some(self.selected_idx),
        );
        self.loaded_once = true;
        true
    }

    pub(crate) fn stage_manifest(
        &mut self,
        origin_scan_request_id: u64,
        scope_epoch: u64,
        scope: &str,
        generation: String,
        total_rows: usize,
        first_page: Vec<crate::session_manager::SessionMeta>,
        reader: crate::session_manager::paged_manifest::ManifestReader,
    ) -> bool {
        self.stage_manifest_source(
            SessionPageSource::Base,
            Some(origin_scan_request_id),
            scope_epoch,
            scope,
            generation,
            total_rows,
            first_page,
            reader,
        )
    }

    fn stage_manifest_source(
        &mut self,
        source: SessionPageSource,
        mut origin_scan_request_id: Option<u64>,
        scope_epoch: u64,
        scope: &str,
        generation: String,
        total_rows: usize,
        mut first_page: Vec<crate::session_manager::SessionMeta>,
        reader: crate::session_manager::paged_manifest::ManifestReader,
    ) -> bool {
        if self.scope_epoch != scope_epoch
            || self.provider_id.as_deref() != Some(scope)
            || reader.scope() != scope
            || reader.generation() != generation
        {
            retire_session_rows(first_page);
            return false;
        }
        if source == SessionPageSource::Base
            && self.base_manifest.as_ref().is_some_and(|base| {
                base.scope_epoch == scope_epoch
                    && base.scope == scope
                    && base.build_epoch > reader.build_epoch()
            })
        {
            retire_session_rows(first_page);
            return false;
        }
        self.drop_tombstoned_rows(&mut first_page);
        retire_session_rows(first_page);
        let mut clear_tombstones_on_install = HashMap::new();
        if let Some(previous) = self.pending_manifest.take() {
            if previous.scope_epoch == scope_epoch
                && previous.source == source
                && reader.build_epoch() >= previous.reader.build_epoch()
            {
                if origin_scan_request_id.is_none() {
                    origin_scan_request_id = previous.origin_scan_request_id;
                }
                clear_tombstones_on_install = previous.clear_tombstones_on_install;
            } else if !previous.clear_tombstones_on_install.is_empty() {
                // The global tombstones still protect every old page. Ask for
                // one fresh authoritative source instead of silently losing
                // the only convergence path for those identities.
                self.purge_refresh_required = true;
            }
        }
        let anchor = self.selected_row().map(SessionRowIdentity::capture);
        self.pending_manifest = Some(PendingSessionManifest {
            scope_epoch,
            origin_scan_request_id,
            request_id: None,
            source,
            generation,
            total_rows,
            reader,
            anchor,
            fallback_absolute: self.selected_absolute(),
            clear_tombstones_on_install,
        });
        true
    }

    pub(crate) fn stage_purged_manifest(
        &mut self,
        source: SessionPageSource,
        scope_epoch: u64,
        scope: &str,
        generation: String,
        total_rows: usize,
        first_page: Vec<crate::session_manager::SessionMeta>,
        reader: crate::session_manager::paged_manifest::ManifestReader,
        delete_request_id: u64,
        deleted_key: String,
    ) -> bool {
        if !self.stage_manifest_source(
            source,
            None,
            scope_epoch,
            scope,
            generation,
            total_rows,
            first_page,
            reader,
        ) {
            return false;
        }
        if let Some(pending) = self.pending_manifest.as_mut() {
            pending
                .clear_tombstones_on_install
                .insert(deleted_key, delete_request_id);
        }
        true
    }

    pub(crate) fn attach_pending_manifest_tombstones(&mut self, tombstones: HashMap<String, u64>) {
        let tombstones = tombstones
            .into_iter()
            .filter(|(key, request_id)| self.purge_tombstone_is_current(*request_id, key))
            .collect::<Vec<_>>();
        if let Some(pending) = self.pending_manifest.as_mut() {
            for (key, request_id) in tombstones {
                pending.clear_tombstones_on_install.insert(key, request_id);
            }
        }
    }

    pub(crate) fn next_manifest_reconcile(
        &mut self,
    ) -> Option<(
        u64,
        SessionPageSource,
        String,
        String,
        usize,
        Option<SessionRowIdentity>,
        crate::session_manager::paged_manifest::ManifestReader,
    )> {
        let pending = self.pending_manifest.as_mut()?;
        if pending.request_id.is_some() {
            return None;
        }
        self.manifest_reconcile_seq = self.manifest_reconcile_seq.wrapping_add(1);
        let request_id = self.manifest_reconcile_seq;
        pending.request_id = Some(request_id);
        Some((
            request_id,
            pending.source,
            self.provider_id.clone()?,
            pending.generation.clone(),
            pending.fallback_absolute,
            pending.anchor.clone(),
            pending.reader.clone(),
        ))
    }

    pub(crate) fn finish_manifest_reconcile(
        &mut self,
        request_id: u64,
        scope_epoch: u64,
        generation: &str,
        page_index: usize,
        mut rows: Vec<crate::session_manager::SessionMeta>,
        selected_local: usize,
        reader: crate::session_manager::paged_manifest::ManifestReader,
    ) -> bool {
        let Some(pending) = self.pending_manifest.take() else {
            retire_session_rows(rows);
            return false;
        };
        if pending.request_id != Some(request_id)
            || pending.scope_epoch != scope_epoch
            || pending.generation != generation
            || self.scope_epoch != scope_epoch
            || reader.scope() != self.provider_id.as_deref().unwrap_or_default()
            || reader.generation() != generation
        {
            self.pending_manifest = Some(pending);
            retire_session_rows(rows);
            return false;
        }
        self.drop_tombstoned_rows(&mut rows);
        self.view_epoch = self.view_epoch.wrapping_add(1);
        let token = SessionPageToken {
            scope_epoch,
            view_epoch: self.view_epoch,
            source: pending.source,
            scope: self.provider_id.clone().unwrap_or_default(),
            generation: generation.to_string(),
        };
        let installed_scope = token.scope.clone();
        let completed_scan = pending.origin_scan_request_id;
        if pending.source == SessionPageSource::Base {
            self.materialized_view = None;
            self.materialized_base_generation = None;
            self.materialization_failure = None;
        }
        let selected_local = selected_local.min(rows.len().saturating_sub(1));
        let selected_absolute = page_index
            .saturating_mul(crate::session_manager::paged_manifest::PAGE_SIZE)
            .saturating_add(selected_local);
        self.remote.install_source(
            token,
            reader,
            pending.total_rows,
            page_index,
            rows,
            selected_absolute,
            &mut self.rows,
            &mut self.selected_idx,
            &mut self.pagination,
        );
        if self.detail_key.as_deref().is_some_and(|key| {
            self.rows
                .get(self.selected_idx)
                .is_none_or(|row| !crate::cli::tui::app::session_key_matches(row, key))
        }) {
            self.clear_detail();
        }
        self.rows_revision = self.rows_revision.wrapping_add(1);
        for (key, delete_request_id) in pending.clear_tombstones_on_install {
            self.clear_purge_tombstone_scope(delete_request_id, &key, &installed_scope);
        }
        self.invalidated_tombstone_scopes.remove(&installed_scope);
        if completed_scan.is_some() && self.scan_active == completed_scan {
            self.scan_active = None;
            self.loading = false;
        } else if self.scan_active.is_none() {
            self.loading = false;
        }
        self.loaded_once = true;
        self.rows_authoritative = true;
        self.last_error = None;
        true
    }

    pub(crate) fn stage_base_restore(&mut self) -> bool {
        let Some(base) = self.base_manifest.clone() else {
            return false;
        };
        if base.scope_epoch != self.scope_epoch
            || self.provider_id.as_deref() != Some(base.scope.as_str())
        {
            return false;
        }
        let mut clear_tombstones_on_install = HashMap::new();
        let mut origin_scan_request_id = None;
        if let Some(previous) = self.pending_manifest.take() {
            if previous.scope_epoch == base.scope_epoch
                && previous.source == SessionPageSource::Base
                && base.reader.build_epoch() >= previous.reader.build_epoch()
            {
                origin_scan_request_id = previous.origin_scan_request_id;
                clear_tombstones_on_install = previous.clear_tombstones_on_install;
            } else if !previous.clear_tombstones_on_install.is_empty() {
                self.purge_refresh_required = true;
            }
        }
        // Clearing a query switches to the saved current base. A query purge
        // is built from that exact base, so the base installation is also a
        // safe convergence point when a query rebuild failed or was cancelled.
        clear_tombstones_on_install.extend(std::mem::take(&mut self.query_tombstones_to_clear));
        self.pending_manifest = Some(PendingSessionManifest {
            scope_epoch: base.scope_epoch,
            origin_scan_request_id,
            request_id: None,
            source: SessionPageSource::Base,
            generation: base.generation,
            total_rows: base.total_rows,
            reader: base.reader,
            anchor: None,
            fallback_absolute: 0,
            clear_tombstones_on_install,
        });
        true
    }

    pub(crate) fn ensure_base_restore_staged(&mut self) -> bool {
        self.pending_manifest.is_some() || self.stage_base_restore()
    }

    pub(crate) fn base_view_is_current(&self) -> bool {
        let Some(base) = self.base_manifest.as_ref() else {
            return false;
        };
        self.remote.token().is_some_and(|token| {
            token.source == SessionPageSource::Base
                && token.scope_epoch == base.scope_epoch
                && token.scope == base.scope
                && token.generation == base.generation
        })
    }

    pub(crate) fn has_query_tombstones_to_clear(&self) -> bool {
        !self.query_tombstones_to_clear.is_empty()
    }

    pub(crate) fn fail_manifest_reconcile(
        &mut self,
        request_id: u64,
        scope_epoch: u64,
        generation: &str,
        error: String,
    ) -> bool {
        let is_current = self.pending_manifest.as_ref().is_some_and(|pending| {
            pending.request_id == Some(request_id)
                && pending.scope_epoch == scope_epoch
                && pending.generation == generation
                && self.scope_epoch == scope_epoch
        });
        if !is_current {
            return false;
        }
        let pending = self
            .pending_manifest
            .take()
            .expect("current reconcile must retain its pending manifest");
        if !pending.clear_tombstones_on_install.is_empty() {
            self.purge_refresh_required = true;
        }
        if let Some(scan) = pending
            .origin_scan_request_id
            .filter(|scan| self.scan_active == Some(*scan))
        {
            self.fail_scan(scan, error);
        } else {
            self.last_error = Some(error);
        }
        true
    }

    pub(crate) fn begin_page_cross(
        &mut self,
        target_page: usize,
        previous_gate: super::paged_list::PagedListState,
        wheel_gesture: Option<crate::cli::tui::input::WheelGestureId>,
    ) -> bool {
        // Before an immutable manifest is installed, `rows` is the complete
        // in-memory source (legacy/test state or the bounded provisional
        // first paint). There is no worker page to wait for in that mode.
        if self.remote.token().is_none() {
            self.selected_idx = self
                .pagination
                .selected_index()
                .unwrap_or(0)
                .min(self.rows.len().saturating_sub(1));
            return true;
        }
        if self.remote.activate_cached(
            target_page,
            &mut self.rows,
            &mut self.selected_idx,
            &self.pagination,
        ) {
            self.rows_revision = self.rows_revision.wrapping_add(1);
            return true;
        }
        self.remote
            .start_cross(target_page, previous_gate, wheel_gesture);
        false
    }

    pub(crate) fn cancel_page_cross(
        &mut self,
        direction: super::paged_list::PageDirection,
    ) -> bool {
        self.remote.cancel_cross(direction, &mut self.pagination)
    }

    pub(crate) fn next_page_request(
        &mut self,
        page: usize,
    ) -> Option<(
        u64,
        SessionPageToken,
        crate::session_manager::paged_manifest::ManifestReader,
    )> {
        self.remote.next_request(page)
    }

    pub(crate) fn finish_page_request(
        &mut self,
        request_id: u64,
        token: &SessionPageToken,
        page: usize,
        mut rows: Vec<crate::session_manager::SessionMeta>,
    ) -> bool {
        self.drop_tombstoned_rows(&mut rows);
        let applied = self.remote.finish_page(
            request_id,
            token,
            page,
            rows,
            &mut self.rows,
            &mut self.selected_idx,
            &mut self.pagination,
        );
        if applied {
            self.rows_revision = self.rows_revision.wrapping_add(1);
            if self.remote.current_page() == page {
                // The page switch completes asynchronously; messages for the
                // old page must not remain paired with the new selected row.
                self.clear_detail();
            }
        }
        applied
    }

    pub(crate) fn fail_page_request(
        &mut self,
        request_id: u64,
        token: &SessionPageToken,
        page: usize,
        error: String,
    ) -> bool {
        self.remote
            .fail_page(request_id, token, page, error, &mut self.pagination)
    }

    pub(crate) fn start_scan(&mut self, provider_id: String) -> u64 {
        let changing_scope = self.provider_id.as_deref() != Some(provider_id.as_str());
        if changing_scope {
            self.scope_epoch = self.scope_epoch.wrapping_add(1);
            self.view_epoch = self.view_epoch.wrapping_add(1);
            self.remote.reset_scope();
            retire_session_rows(std::mem::take(&mut self.rows));
            self.rows_revision = self.rows_revision.wrapping_add(1);
            self.selected_idx = 0;
            self.loaded_once = false;
            self.rows_authoritative = false;
            self.pending_manifest = None;
            self.base_manifest = None;
            self.materialized_view = None;
            self.materialized_base_generation = None;
            self.materialization_failure = None;
            self.retire_project_catalog();
            self.project_catalog_active = None;
            self.project_catalog_loading = false;
            self.project_catalog_error = None;
            self.project_filter_active = None;
            self.deep_search_active = None;
            self.clear_deep_search_results();
            self.pagination.reset(0, None);
            self.clear_detail();
        }
        self.provider_id = Some(provider_id.clone());
        // `start_scan` is reached only for an explicit refresh or a new
        // convergence attempt. Either is a deliberate retry boundary.
        self.materialization_failure = None;
        self.scan_attempted_scope = Some(provider_id.clone());
        self.time_anchor_ms = chrono::Utc::now().timestamp_millis();
        self.scan_seq = self.scan_seq.wrapping_add(1);
        self.scan_active = Some(self.scan_seq);
        self.scan_tombstones_to_clear = Some((
            self.scan_seq,
            provider_id,
            self.tombstones_for_scope(self.provider_id.as_deref().unwrap_or_default()),
        ));
        self.scan_accepts_previews = false;
        self.purge_refresh_required = false;
        self.loading = true;
        self.last_error = None;
        self.scan_seq
    }

    pub(crate) fn scan_attempted_for_scope(&self, scope: &str) -> bool {
        self.scan_attempted_scope.as_deref() == Some(scope)
    }

    pub(crate) fn fail_scan(&mut self, request_id: u64, error: String) {
        if self.scan_active == Some(request_id) {
            if self
                .scan_tombstones_to_clear
                .as_ref()
                .is_some_and(|(pending_request, _, _)| *pending_request == request_id)
            {
                self.scan_tombstones_to_clear = None;
            }
            self.scan_active = None;
            self.scan_accepts_previews = false;
            self.loading = false;
            self.loaded_once = true;
            // Provisional rows are not a durable query source. Preserve the
            // terminal error whenever no manifest is installed so an open
            // project picker can stop loading and explain the failure.
            self.last_error = if self.remote.token().is_none() {
                Some(error)
            } else {
                None
            };
            // Delete tombstones outlive a failed scan. They are removed only
            // after the asynchronously repacked manifest is installed.
        }
    }

    /// Drop rows tombstoned by an in-flight delete (see `scan_tombstones`). A scan
    /// thread may have read a session file *before* the user deleted it, so the
    /// partial/finished it later delivers still carries that session; filtering
    /// here keeps the deleted row from resurrecting in the UI. No-op (early
    /// return) when there are no tombstones, which is the common case.
    fn drop_tombstoned_rows(&self, rows: &mut Vec<crate::session_manager::SessionMeta>) {
        let scope = self.provider_id.as_deref().unwrap_or_default();
        if self.invalidated_tombstone_scopes.contains(scope) {
            rows.clear();
            return;
        }
        if self.scan_tombstones.is_empty() {
            return;
        }
        // Build at most one bounded composite key per page row, then use hash
        // lookups. Runtime cost is O(page rows), never O(rows × tombstones).
        rows.retain(|row| !self.row_is_tombstoned_for_scope(row, scope));
    }

    fn row_is_tombstoned_for_scope(
        &self,
        row: &crate::session_manager::SessionMeta,
        scope: &str,
    ) -> bool {
        if self.invalidated_tombstone_scopes.contains(scope) {
            return true;
        }
        let key = crate::cli::tui::app::session_key(row);
        self.purge_tombstone_revisions
            .get(&key)
            .map(|revision| revision.pending_scopes.contains(scope))
            // Legacy unit fixtures insert directly into `scan_tombstones`;
            // those entries intentionally apply to every scope.
            .unwrap_or_else(|| self.scan_tombstones.contains(&key))
    }

    pub(crate) fn finish_scan(
        &mut self,
        request_id: u64,
        mut rows: Vec<crate::session_manager::SessionMeta>,
    ) -> bool {
        if self.scan_active != Some(request_id) {
            return false;
        }
        self.drop_tombstoned_rows(&mut rows);
        let scope = self.provider_id.clone().unwrap_or_default();
        let safe_tombstones = self.take_scan_tombstones_for_manifest(request_id, &scope);
        // Tombstones are cleared by the purge publication, never merely by a
        // scan result that may have raced the delete barrier.
        self.scan_active = None;
        self.scan_accepts_previews = false;
        self.loading = false;
        self.loaded_once = true;
        self.rows_authoritative = true;
        self.last_error = None;
        self.replace_rows(rows);
        self.clear_tombstones_for_scope(&scope, safe_tombstones);
        // The message handler reconciles detail/selection against its captured
        // structured identity. Keeping that check there avoids a second O(N)
        // pass through a million-row result on the UI thread.
        true
    }

    /// Apply the stale-while-revalidate first paint: the list built from the
    /// bounded recency snapshot, delivered before the revalidating scan finishes. The
    /// rows become interactive immediately, but `loading`/`scan_active` stay set
    /// so the header keeps showing the refresh indicator and the eventual
    /// `finish_scan` (same request id) still applies. The in-memory scan cache is
    /// deliberately not written here — only the final, complete list is cached.
    /// Returns true when the snapshot was applied (still the active scan).
    pub(crate) fn apply_cached_snapshot(
        &mut self,
        request_id: u64,
        mut rows: Vec<crate::session_manager::SessionMeta>,
    ) -> bool {
        if self.scan_active != Some(request_id) {
            return false;
        }
        if !self.scan_accepts_previews {
            return false;
        }
        rows = bounded_session_preview(rows);
        self.drop_tombstoned_rows(&mut rows);
        self.loaded_once = true;
        self.replace_rows(rows);
        true
    }

    /// Add the first row discovered by a still-running provider only when the
    /// current bounded preview has no row for that provider. This preserves a
    /// 101-row JSON snapshot (and its selection/detail identity) instead of
    /// collapsing it to one arbitrary discovery-order row. `ScanPartial` later
    /// replaces the provider with its true recency top-K.
    pub(crate) fn apply_progressive_preview(
        &mut self,
        request_id: u64,
        provider_id: &str,
        row: crate::session_manager::SessionMeta,
    ) -> bool {
        if self.scan_active != Some(request_id) || !self.scan_accepts_previews {
            return false;
        }
        if self
            .rows
            .iter()
            .any(|existing| existing.provider_id == provider_id)
        {
            return false;
        }
        if self.row_is_tombstoned_for_scope(&row, self.provider_id.as_deref().unwrap_or_default()) {
            return false;
        }

        self.loaded_once = true;
        self.rows.push(row);
        crate::session_manager::sort_by_recent(&mut self.rows);
        self.rows
            .truncate(crate::session_manager::SCAN_CACHE_FIRST_PAINT_LIMIT);
        self.rows_revision = self.rows_revision.wrapping_add(1);
        true
    }

    /// Progressive fill during a revalidating "all providers" scan: replace one
    /// provider's rows with its freshly-scanned list while the other providers
    /// keep their current rows (cached snapshot or earlier partials). Keeps the
    /// refresh indicator on until `finish_scan` (same request id) lands.
    pub(crate) fn apply_partial_scan(
        &mut self,
        request_id: u64,
        provider_id: &str,
        mut rows: Vec<crate::session_manager::SessionMeta>,
    ) -> bool {
        if self.scan_active != Some(request_id) {
            return false;
        }
        if !self.scan_accepts_previews {
            return false;
        }
        rows = bounded_session_preview(rows);
        self.drop_tombstoned_rows(&mut rows);
        self.loaded_once = true;
        // Both sides are bounded before any retain/extend/sort executes. This
        // is a defence-in-depth invariant: even a future worker regression that
        // sends a full provider Vec cannot move O(N) work onto the UI thread.
        if self.rows.len() > crate::session_manager::SCAN_CACHE_FIRST_PAINT_LIMIT {
            return false;
        }
        self.rows.retain(|row| row.provider_id != provider_id);
        self.rows.extend(rows);
        crate::session_manager::sort_by_recent(&mut self.rows);
        self.rows
            .truncate(crate::session_manager::SCAN_CACHE_FIRST_PAINT_LIMIT);
        self.rows_revision = self.rows_revision.wrapping_add(1);
        true
    }

    fn replace_rows(&mut self, rows: Vec<crate::session_manager::SessionMeta>) {
        let old = std::mem::replace(&mut self.rows, rows);
        self.rows_revision = self.rows_revision.wrapping_add(1);
        retire_session_rows(old);
    }

    pub(crate) fn open_detail(&mut self, key: String) {
        if self.detail_key.as_deref() == Some(key.as_str()) {
            return;
        }
        self.detail_key = Some(key);
        self.clear_messages();
    }

    pub(crate) fn message_query_lower(&self) -> Option<String> {
        let trimmed = self.message_filter.value.trim();
        if trimmed.is_empty() {
            return None;
        }
        Some(trimmed.to_lowercase())
    }

    pub(crate) fn clear_detail(&mut self) {
        self.detail_key = None;
        self.clear_messages();
    }

    fn clear_messages(&mut self) {
        self.message_cancel_pending |= self.message_active.is_some();
        self.messages_key = None;
        retire_session_messages(std::mem::take(&mut self.messages));
        self.messages_revision = self.messages_revision.wrapping_add(1);
        self.messages_loading = false;
        self.messages_loaded = false;
        self.messages_truncated = false;
        self.messages_error = None;
        self.message_idx = 0;
        self.message_active = None;
    }

    pub(crate) fn start_message_load(&mut self, key: String) -> u64 {
        self.message_seq = self.message_seq.wrapping_add(1);
        self.message_active = Some(self.message_seq);
        self.messages_key = Some(key);
        retire_session_messages(std::mem::take(&mut self.messages));
        self.messages_revision = self.messages_revision.wrapping_add(1);
        self.messages_loading = true;
        self.messages_loaded = false;
        self.messages_truncated = false;
        self.messages_error = None;
        self.message_idx = 0;
        self.message_seq
    }

    pub(crate) fn fail_message_load(&mut self, request_id: u64, key: &str, error: String) {
        if self.message_active == Some(request_id)
            && self.messages_key.as_deref() == Some(key)
            && self.detail_key.as_deref() == Some(key)
        {
            self.message_active = None;
            self.messages_loading = false;
            self.messages_loaded = true;
            self.messages_error = Some(error);
        }
    }

    pub(crate) fn finish_message_load<B>(&mut self, request_id: u64, key: &str, batch: B) -> bool
    where
        B: Into<crate::session_manager::SessionMessageBatch>,
    {
        if self.message_active != Some(request_id)
            || self.messages_key.as_deref() != Some(key)
            || self.detail_key.as_deref() != Some(key)
        {
            return false;
        }
        self.message_active = None;
        self.messages_loading = false;
        self.messages_loaded = true;
        self.messages_error = None;
        let batch = batch.into();
        self.messages = batch.messages;
        self.messages_truncated = batch.truncated;
        self.messages_revision = self.messages_revision.wrapping_add(1);
        self.message_idx = self.message_idx.min(self.messages.len().saturating_sub(1));
        true
    }

    pub(crate) fn message_load_is_current(&self, request_id: u64, key: &str) -> bool {
        self.message_active == Some(request_id)
            && self.messages_key.as_deref() == Some(key)
            && self.detail_key.as_deref() == Some(key)
    }

    pub(crate) fn take_message_cancel_pending(&mut self) -> bool {
        std::mem::take(&mut self.message_cancel_pending)
    }

    pub(crate) fn clear_deep_search_results(&mut self) {
        retire_session_search_hits(std::mem::take(&mut self.deep_search_results));
    }

    pub(crate) fn replace_deep_search_results(
        &mut self,
        hits: Vec<crate::session_manager::SessionSearchHit>,
    ) {
        let old = std::mem::replace(&mut self.deep_search_results, hits);
        retire_session_search_hits(old);
    }

    pub(crate) fn start_delete(&mut self) -> u64 {
        self.delete_seq = self.delete_seq.wrapping_add(1);
        self.delete_active.insert(self.delete_seq);
        self.delete_seq
    }

    pub(crate) fn finish_delete(&mut self, request_id: u64, key: &str) -> bool {
        if !self.delete_active.remove(&request_id) {
            return false;
        }
        let _ = self.remove_session_by_key(key);
        true
    }

    pub(crate) fn fail_delete(&mut self, request_id: u64) {
        self.delete_active.remove(&request_id);
    }

    pub(crate) fn remove_session_by_key(&mut self, key: &str) -> bool {
        let before = self.rows.len();
        self.rows
            .retain(|session| !crate::cli::tui::app::session_key_matches(session, key));
        let removed_active = self.rows.len() != before;
        let removed_cached = self.remote.remove_by_key(key);
        if !removed_active && !removed_cached {
            return false;
        }
        // Preserve the pinned generation's source total until the post-delete
        // manifest is installed. Shrinking it here can hide a surviving final
        // page whose rows have not yet been repacked into the preceding page.
        self.rows_revision = self.rows_revision.wrapping_add(1);
        self.selected_idx = self.selected_idx.min(self.rows.len().saturating_sub(1));
        self.pagination.sync_len(self.logical_total_rows());
        if self.detail_key.as_deref() == Some(key) {
            self.clear_detail();
        }
        true
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastKind {
    Info,
    Success,
    Warning,
    Error,
}

#[derive(Debug, Clone)]
pub struct Toast {
    pub message: String,
    pub kind: ToastKind,
    pub remaining_ticks: u16,
    pub persistent: bool,
}

impl Toast {
    pub fn new(message: impl Into<String>, kind: ToastKind) -> Self {
        Self {
            message: message.into(),
            kind,
            remaining_ticks: 12,
            persistent: false,
        }
    }

    pub fn persistent(message: impl Into<String>, kind: ToastKind) -> Self {
        Self {
            message: message.into(),
            kind,
            remaining_ticks: 0,
            persistent: true,
        }
    }
}

#[derive(Debug, Clone)]
pub enum ConfirmAction {
    Quit,
    ProviderDelete {
        id: String,
    },
    ProviderCopy {
        id: String,
    },
    ProviderRemoveFromConfig {
        id: String,
    },
    McpDelete {
        id: String,
    },
    PromptDelete {
        id: String,
    },
    PricingDelete {
        model_id: String,
    },
    SessionDelete {
        key: String,
        provider_id: String,
        session_id: String,
        source_path: String,
    },
    SkillsUninstall {
        directory: String,
    },
    SkillsRepoRemove {
        owner: String,
        name: String,
    },
    ConfigImport {
        path: String,
    },
    ConfigRestoreBackup {
        id: String,
    },
    ConfigReset,
    SettingsSetSkipClaudeOnboarding {
        enabled: bool,
    },
    SettingsSetClaudePluginIntegration {
        enabled: bool,
    },
    SettingsSetCodexUnifiedSessionHistory {
        enabled: bool,
    },
    VisibleAppsAutoDetection,
    VisibleAppsSwitchToManual {
        apps: crate::settings::VisibleApps,
        selected: usize,
    },
    ProviderApiFormatProxyNotice,
    CommonConfigNotice,
    UsageQueryNotice,
    ManagedAuthCancelLogin,
    ProxyEnableAndAutoFailover {
        app_type: AppType,
    },
    PromptOpenImportCandidate {
        filename: String,
        content: String,
    },
    OpenClawDailyMemoryDelete {
        filename: String,
    },
    FormSaveBeforeClose,
    #[allow(dead_code)]
    EditorDiscard,
    EditorSaveBeforeClose,
    WebDavMigrateV1ToV2,
    CloudSyncTransfer {
        backend: super::CloudSyncBackend,
        intent: super::CloudSyncTransferIntent,
    },
    CloudSyncReset {
        backend: super::CloudSyncBackend,
    },
    ClaudeModelFillAll {
        source_idx: usize,
    },
}

#[derive(Debug, Clone)]
pub struct ConfirmOverlay {
    pub title: String,
    pub message: String,
    pub action: ConfirmAction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextSubmit {
    ConfigExport,
    ConfigImport,
    ConfigBackupName,
    SettingsProxyListenAddress,
    SettingsProxyListenPort,
    SettingsOpenClawConfigDir,
    SettingsPreferredEditor,
    #[allow(dead_code)]
    SkillsInstallSpec,
    SkillsDiscoverQuery,
    SkillsRepoAdd,
    OpenClawDailyMemoryFilename,
    OpenClawToolsRule {
        section: OpenClawToolsSection,
        row: Option<usize>,
    },
    OpenClawAgentsRuntimeField {
        field: OpenClawAgentsRuntimeField,
    },
    UsageCustomRange,
    ProviderCustomUserAgent,
    CodexModelCatalogField {
        row: Option<usize>,
        field: form::CodexModelCatalogField,
    },
    WebDavJianguoyunUsername,
    WebDavJianguoyunPassword,
}

#[derive(Debug, Clone)]
pub struct TextInputState {
    pub title: String,
    pub prompt: String,
    pub input: TextInput,
    pub submit: TextSubmit,
}

impl TextInputState {
    pub const fn is_editing(&self) -> bool {
        true
    }
}

#[derive(Debug, Clone)]
pub struct SessionProjectPickerState {
    pub input: TextInput,
    pub selected_idx: usize,
    /// UTF-8 byte offset for the selected full-path viewport. `usize::MAX`
    /// pins the viewport to the path suffix without measuring the whole path
    /// on every redraw.
    pub path_scroll: usize,
    /// `None` means the complete catalog. Filtering is recomputed only when
    /// the input changes, never during redraw.
    pub filtered_indices: Option<Vec<usize>>,
    /// Preserve the selected project in a provider scope where it currently
    /// has zero sessions, so the picker never silently falls back to All.
    pub pinned_scope: Option<crate::session_manager::project_scope::SessionProjectScope>,
    pub filter_error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct TextViewState {
    pub title: String,
    pub lines: Vec<String>,
    pub scroll: usize,
    pub action: Option<TextViewAction>,
}

#[derive(Debug, Clone)]
pub enum TextViewAction {
    ProxyToggleManagedRoute,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommonSnippetViewSource {
    Global,
    ProviderForm,
}

#[derive(Debug, Clone)]
pub struct ManagedAuthLoginState {
    pub auth_provider: String,
    pub device_code: String,
    pub expires_at_tick: u64,
    pub poll_interval_ticks: u64,
    pub next_poll_tick: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadingKind {
    Generic,
    Proxy,
    WebDav,
    S3,
    UpdateCheck,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpEnvEditorField {
    Key,
    Value,
}

#[derive(Debug, Clone)]
pub struct McpEnvEntryEditorState {
    pub row: Option<usize>,
    pub return_selected: usize,
    pub field: McpEnvEditorField,
    pub key: crate::cli::tui::form::TextInput,
    pub value: crate::cli::tui::form::TextInput,
}

impl McpEnvEntryEditorState {
    pub fn key_active(&self) -> bool {
        matches!(self.field, McpEnvEditorField::Key)
    }

    pub fn value_active(&self) -> bool {
        matches!(self.field, McpEnvEditorField::Value)
    }

    pub fn is_editing(&self) -> bool {
        true
    }
}

#[derive(Debug, Clone)]
pub enum Overlay {
    None,
    Help(crate::cli::tui::help::HelpState),
    Confirm(ConfirmOverlay),
    TextInput(TextInputState),
    BackupPicker {
        selected: usize,
    },
    TextView(TextViewState),
    #[allow(dead_code)]
    CommonSnippetPicker {
        selected: usize,
    },
    ProviderTestMenu {
        provider_id: String,
        selected: usize,
    },
    FailoverQueueManager {
        selected_provider_id: Option<String>,
    },
    ClaudeModelPicker {
        selected: usize,
        column: ClaudeModelPickerColumn,
        editing: bool,
    },
    ClaudeApiFormatPicker {
        selected: usize,
    },
    UserAgentPicker {
        selected: usize,
    },
    ExternalEditorPicker {
        selected: usize,
        editors: Vec<crate::cli::editor::DetectedEditor>,
    },
    UsageQueryTemplatePicker {
        selected: usize,
    },
    S3PresetPicker {
        selected: usize,
    },
    ManagedAccountPicker {
        auth_provider: String,
        selected: usize,
        binding: bool,
        selected_account_id: Option<String>,
    },
    ManagedAccountActionPicker {
        auth_provider: String,
        account_id: String,
        selected: usize,
    },
    HermesModelsPicker {
        editing: bool,
    },
    ModelFetchPicker {
        request_id: u64,
        field: ProviderAddField,
        claude_idx: Option<usize>,
        input: TextInput,
        query: String,
        fetching: bool,
        models: Vec<String>,
        /// `None` means the unfiltered full model list. Search input updates
        /// this cache explicitly, so periodic redraws never rescan every
        /// fetched model.
        filtered_indices: Option<Vec<usize>>,
        /// Search is deliberately budgeted so a hostile model endpoint cannot
        /// monopolize the TUI thread. When true, the visible matches are only
        /// a bounded subset and the picker asks the user to refine the query.
        filter_incomplete: bool,
        error: Option<String>,
        selected_idx: usize,
        /// Navigation selects a fetched result without copying its complete
        /// model id into the search input. Typing resets this flag so Enter can
        /// still submit a custom model id.
        selection_active: bool,
    },
    SessionProjectPicker(SessionProjectPickerState),
    OpenClawToolsProfilePicker {
        selected: Option<usize>,
    },
    OpenClawAgentsFallbackPicker {
        insert_at: usize,
        selected: usize,
        /// Option that matched the saved value when the picker opened. Keeping
        /// its index avoids comparing arbitrary complete model ids every frame.
        active: Option<usize>,
        options: Vec<OpenClawModelOption>,
    },
    McpAppsPicker {
        id: String,
        name: String,
        selected: usize,
        apps: crate::app_config::McpApps,
    },
    VisibleAppsPicker {
        selected: usize,
        apps: crate::settings::VisibleApps,
    },
    SkillsAppsPicker {
        directory: String,
        name: String,
        selected: usize,
        apps: crate::app_config::SkillApps,
    },
    SkillsImportPicker {
        skills: Vec<crate::services::skill::UnmanagedSkill>,
        selected_idx: usize,
        selected: HashSet<String>,
    },
    #[allow(dead_code)]
    SkillsSyncMethodPicker {
        selected: usize,
    },
    McpEnvPicker {
        selected: usize,
    },
    McpTypePicker {
        selected: usize,
    },
    McpEnvEntryEditor(McpEnvEntryEditorState),
    Loading {
        kind: LoadingKind,
        title: String,
        message: String,
    },
    SpeedtestRunning {
        url: String,
    },
    SpeedtestResult {
        url: String,
        lines: Vec<String>,
        scroll: usize,
    },
    StreamCheckRunning {
        provider_id: String,
        provider_name: String,
    },
    StreamCheckResult {
        provider_name: String,
        lines: Vec<String>,
        scroll: usize,
    },
    UpdateAvailable {
        current: String,
        latest: String,
        selected: usize,
    },
    UpdateDownloading {
        downloaded: u64,
        total: Option<u64>,
    },
    UpdateResult {
        success: bool,
        message: String,
    },
}

pub(crate) const MODEL_FETCH_QUERY_MAX_CHARS: usize = 128;
pub(crate) const MODEL_FETCH_QUERY_MAX_BYTES: usize = MODEL_FETCH_QUERY_MAX_CHARS * 4;
pub(crate) const MODEL_FETCH_FILTER_MAX_MODELS: usize = 16 * 1024;
pub(crate) const MODEL_FETCH_FILTER_MAX_SOURCE_BYTES: usize = 512 * 1024;
pub(crate) const MODEL_FETCH_FILTER_MAX_MATCHES: usize = 2 * 1024;
const MODEL_FETCH_FILTER_MODEL_PREFIX_BYTES: usize = 512;

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct ModelFetchFilterResult {
    pub(crate) indices: Option<Vec<usize>>,
    pub(crate) incomplete: bool,
}

fn model_fetch_bounded_prefix(text: &str, max_bytes: usize) -> &str {
    let mut end = text.len().min(max_bytes);
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

fn model_fetch_prefix_contains(prefix: &str, query_lower: &str) -> bool {
    if prefix.is_ascii() && query_lower.is_ascii() {
        return prefix
            .as_bytes()
            .windows(query_lower.len())
            .any(|window| window.eq_ignore_ascii_case(query_lower.as_bytes()));
    }

    prefix.to_lowercase().contains(query_lower)
}

/// Build a search cache under hard row, byte, per-item, and result budgets.
/// Model endpoints are untrusted: an individual id or the returned collection
/// can be arbitrarily large, while this function runs on the TUI event thread.
pub(crate) fn model_fetch_filter(models: &[String], query: &str) -> ModelFetchFilterResult {
    if query.len() > MODEL_FETCH_QUERY_MAX_BYTES {
        return ModelFetchFilterResult {
            indices: Some(Vec::new()),
            incomplete: true,
        };
    }

    let query = query.trim();
    if query.is_empty() {
        return ModelFetchFilterResult::default();
    }

    let query_lower = query.to_lowercase();
    let mut indices = Vec::new();
    let mut scanned_bytes = 0usize;
    let mut incomplete = false;

    for (index, model) in models
        .iter()
        .enumerate()
        .take(MODEL_FETCH_FILTER_MAX_MODELS)
    {
        let remaining_bytes = MODEL_FETCH_FILTER_MAX_SOURCE_BYTES.saturating_sub(scanned_bytes);
        if remaining_bytes == 0 {
            incomplete = true;
            break;
        }

        let prefix = model_fetch_bounded_prefix(
            model,
            remaining_bytes.min(MODEL_FETCH_FILTER_MODEL_PREFIX_BYTES),
        );
        scanned_bytes = scanned_bytes.saturating_add(prefix.len());
        incomplete |= prefix.len() < model.len();

        if model_fetch_prefix_contains(prefix, &query_lower) {
            indices.push(index);
            if indices.len() == MODEL_FETCH_FILTER_MAX_MATCHES {
                incomplete = true;
                break;
            }
        }
    }

    if models.len() > MODEL_FETCH_FILTER_MAX_MODELS {
        incomplete = true;
    }

    ModelFetchFilterResult {
        indices: Some(indices),
        incomplete,
    }
}

impl Overlay {
    pub fn is_active(&self) -> bool {
        !matches!(self, Overlay::None)
    }

    pub fn can_be_covered_by_help(&self) -> bool {
        matches!(
            self,
            Overlay::BackupPicker { .. }
                | Overlay::TextView(_)
                | Overlay::CommonSnippetPicker { .. }
                | Overlay::ProviderTestMenu { .. }
                | Overlay::FailoverQueueManager { .. }
                | Overlay::ClaudeApiFormatPicker { .. }
                | Overlay::UserAgentPicker { .. }
                | Overlay::ExternalEditorPicker { .. }
                | Overlay::UsageQueryTemplatePicker { .. }
                | Overlay::S3PresetPicker { .. }
                | Overlay::ManagedAccountPicker { .. }
                | Overlay::ManagedAccountActionPicker { .. }
                | Overlay::ClaudeModelPicker { editing: false, .. }
                | Overlay::HermesModelsPicker { editing: false }
                | Overlay::SessionProjectPicker(_)
                | Overlay::OpenClawToolsProfilePicker { .. }
                | Overlay::OpenClawAgentsFallbackPicker { .. }
                | Overlay::McpAppsPicker { .. }
                | Overlay::VisibleAppsPicker { .. }
                | Overlay::SkillsAppsPicker { .. }
                | Overlay::SkillsImportPicker { .. }
                | Overlay::SkillsSyncMethodPicker { .. }
                | Overlay::McpEnvPicker { .. }
                | Overlay::McpTypePicker { .. }
                | Overlay::SpeedtestResult { .. }
                | Overlay::StreamCheckResult { .. }
                | Overlay::UpdateAvailable { .. }
                | Overlay::UpdateResult { .. }
        )
    }

    /// Whether this overlay is actively accepting text input.
    /// This controls whether the main UI should consider itself in "editing mode" and e.g. respond to vim-style navigation.
    pub fn is_editing(&self) -> bool {
        match self {
            Overlay::TextInput(input) => input.is_editing(),
            Overlay::ClaudeModelPicker { editing, .. } => *editing,
            Overlay::HermesModelsPicker { editing } => *editing,
            Overlay::ModelFetchPicker { .. } => true,
            Overlay::SessionProjectPicker(_) => true,
            Overlay::McpEnvEntryEditor(editor) => editor.is_editing(),
            Overlay::None
            | Overlay::Help(_)
            | Overlay::Confirm(_)
            | Overlay::BackupPicker { .. }
            | Overlay::TextView(_)
            | Overlay::CommonSnippetPicker { .. }
            | Overlay::ProviderTestMenu { .. }
            | Overlay::FailoverQueueManager { .. }
            | Overlay::ClaudeApiFormatPicker { .. }
            | Overlay::UserAgentPicker { .. }
            | Overlay::ExternalEditorPicker { .. }
            | Overlay::UsageQueryTemplatePicker { .. }
            | Overlay::S3PresetPicker { .. }
            | Overlay::ManagedAccountPicker { .. }
            | Overlay::ManagedAccountActionPicker { .. }
            | Overlay::OpenClawToolsProfilePicker { .. }
            | Overlay::OpenClawAgentsFallbackPicker { .. }
            | Overlay::McpAppsPicker { .. }
            | Overlay::VisibleAppsPicker { .. }
            | Overlay::SkillsAppsPicker { .. }
            | Overlay::SkillsImportPicker { .. }
            | Overlay::SkillsSyncMethodPicker { .. }
            | Overlay::McpEnvPicker { .. }
            | Overlay::McpTypePicker { .. }
            | Overlay::Loading { .. }
            | Overlay::SpeedtestRunning { .. }
            | Overlay::SpeedtestResult { .. }
            | Overlay::StreamCheckRunning { .. }
            | Overlay::StreamCheckResult { .. }
            | Overlay::UpdateAvailable { .. }
            | Overlay::UpdateDownloading { .. }
            | Overlay::UpdateResult { .. } => false,
        }
    }
}

#[cfg(test)]
mod sessions_state_tests {
    use super::{FilterState, SessionPageSource, SessionsState};
    use crate::app_config::AppType;
    use crate::session_manager::{SessionMessage, SessionMeta};

    fn meta(provider: &str, session: &str, last_active: i64) -> SessionMeta {
        SessionMeta {
            provider_id: provider.to_string(),
            session_id: session.to_string(),
            last_active_at: Some(last_active),
            ..SessionMeta::default()
        }
    }

    fn publish_rows(
        store: &crate::session_manager::paged_manifest::PagedManifestStore,
        scope: &str,
        prefix: &str,
        count: usize,
    ) -> (
        crate::session_manager::paged_manifest::PublishedManifest,
        crate::session_manager::paged_manifest::ManifestReader,
    ) {
        let mut builder = store.begin_build(scope).expect("begin manifest fixture");
        for index in 0..count {
            builder
                .push(meta("claude", &format!("{prefix}-{index}"), index as i64))
                .expect("push manifest fixture row");
        }
        let published = builder.publish().expect("publish manifest fixture");
        let reader = store
            .open_generation(scope, &published.generation)
            .expect("open manifest fixture reader");
        (published, reader)
    }

    #[test]
    fn large_project_values_are_dropped_off_the_event_thread() {
        struct DropProbe(std::sync::mpsc::Sender<std::thread::ThreadId>);

        impl Drop for DropProbe {
            fn drop(&mut self) {
                let _ = self.0.send(std::thread::current().id());
            }
        }

        let caller = std::thread::current().id();
        let (tx, rx) = std::sync::mpsc::channel();
        super::retire_large_project_value(4_096, DropProbe(tx));
        let drop_thread = rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("background project catalog drop");

        assert_ne!(drop_thread, caller);
    }

    #[test]
    fn project_scope_survives_provider_scope_changes() {
        let mut state = SessionsState::default();
        let project = crate::session_manager::project_scope::SessionProjectScope::exact(
            "/workspace/cc-switch",
        )
        .expect("exact project scope");

        assert!(state.set_project_scope(project.clone()));
        state.start_scan("claude".to_string());
        state.start_scan("codex".to_string());

        assert_eq!(state.project_scope, project);
        assert!(state.desired_view_requires_materialization(None));
    }

    #[test]
    fn active_ui_source_reader_leases_pages_across_multiple_publications() {
        let directory = tempfile::tempdir().expect("manifest fixture directory");
        let store =
            crate::session_manager::paged_manifest::PagedManifestStore::open_at(directory.path())
                .expect("manifest fixture store");
        let (published, reader) = publish_rows(&store, "claude", "old", 205);
        let mut state = SessionsState::default();
        let _request_id = state.start_scan("claude".to_string());
        let scope_epoch = state.scope_epoch;
        assert!(state.remember_base_manifest(
            scope_epoch,
            "claude",
            published.generation.clone(),
            published.total_rows,
            reader.clone(),
        ));
        assert!(state.apply_opened_manifest(
            scope_epoch,
            "claude",
            published.generation,
            published.total_rows,
            published.first_page.page_index,
            published.first_page.rows,
            reader,
        ));

        for round in 0..3 {
            publish_rows(&store, "claude", &format!("new-{round}"), 1);
        }

        let leased_page = state
            .remote
            .active_reader
            .as_ref()
            .expect("active UI source reader")
            .load_page(1)
            .expect("leased old UI page remains readable");
        assert_eq!(leased_page.rows.len(), 100);
        assert!(leased_page
            .rows
            .iter()
            .all(|row| row.session_id.starts_with("old-")));
    }

    #[test]
    fn query_view_leases_its_active_source_and_saved_base_across_gc() {
        let directory = tempfile::tempdir().expect("manifest fixture directory");
        let base_store =
            crate::session_manager::paged_manifest::PagedManifestStore::open_at(directory.path())
                .expect("base manifest fixture store");
        let query_store = crate::session_manager::paged_manifest::QueryManifestStore::open_at(
            directory.path(),
            &crate::session_manager::paged_manifest::QueryManifestNamespace::for_test(
                "tui-app-types",
            ),
        )
        .expect("query manifest fixture store");
        let (base, base_reader) = publish_rows(&base_store, "claude", "base-old", 205);
        let mut query_builder = query_store
            .begin_build(&base_reader)
            .expect("query manifest fixture builder");
        for index in 0..101 {
            query_builder
                .push(meta("claude", &format!("query-old-{index}"), index as i64))
                .expect("query manifest fixture row");
        }
        let query = query_builder.publish().expect("publish query fixture");
        let query_reader = query.reader.clone();

        let mut state = SessionsState::default();
        let _request_id = state.start_scan("claude".to_string());
        let scope_epoch = state.scope_epoch;
        assert!(state.remember_base_manifest(
            scope_epoch,
            "claude",
            base.generation.clone(),
            base.total_rows,
            base_reader,
        ));
        assert!(state.apply_query_manifest(
            scope_epoch,
            "claude",
            &base.generation,
            crate::session_manager::project_scope::SessionViewSpec::all_projects("needle"),
            query.generation,
            query.total_rows,
            query.first_page.page_index,
            query.first_page.rows,
            query_reader,
        ));

        for round in 0..3 {
            publish_rows(&base_store, "claude", &format!("base-new-{round}"), 1);
            let current_base = base_store.open_reader("claude").expect("current base");
            let mut next_query = query_store
                .begin_build(&current_base)
                .expect("new query fixture builder");
            next_query
                .push(meta(
                    "claude",
                    &format!("query-new-{round}"),
                    10_000 + round,
                ))
                .expect("new query fixture row");
            next_query.publish().expect("publish newer query fixture");
        }

        let old_base_page = state
            .base_manifest
            .as_ref()
            .expect("saved base reader")
            .reader
            .load_page(1)
            .expect("saved old base remains readable");
        assert_eq!(old_base_page.rows.len(), 100);
        let old_query_page = state
            .remote
            .active_reader
            .as_ref()
            .expect("active query reader")
            .load_page(1)
            .expect("active old query remains readable");
        assert_eq!(old_query_page.rows.len(), 1);
    }

    #[test]
    fn successful_query_supersedes_pending_refresh_without_leaving_loading_stuck() {
        let directory = tempfile::tempdir().expect("manifest fixture directory");
        let base_store =
            crate::session_manager::paged_manifest::PagedManifestStore::open_at(directory.path())
                .expect("base manifest fixture store");
        let query_store = crate::session_manager::paged_manifest::QueryManifestStore::open_at(
            directory.path(),
            &crate::session_manager::paged_manifest::QueryManifestNamespace::for_test(
                "tui-query-supersedes-refresh",
            ),
        )
        .expect("query manifest fixture store");
        let (base, base_reader) = publish_rows(&base_store, "claude", "base", 3);
        let mut query_builder = query_store
            .begin_build(&base_reader)
            .expect("query fixture builder");
        query_builder
            .push(meta("claude", "query-row", 2))
            .expect("query fixture row");
        let query = query_builder.publish().expect("query fixture");
        let query_reader = query.reader.clone();

        let mut state = SessionsState::default();
        let scan_request = state.start_scan("claude".to_string());
        let scope_epoch = state.scope_epoch;
        assert!(state.remember_base_manifest(
            scope_epoch,
            "claude",
            base.generation.clone(),
            base.total_rows,
            base_reader.clone(),
        ));
        assert!(state.stage_manifest(
            scan_request,
            scope_epoch,
            "claude",
            base.generation.clone(),
            base.total_rows,
            base.first_page.rows,
            base_reader.clone(),
        ));
        let (locate_id, _, _, locate_generation, _, _, _) = state
            .next_manifest_reconcile()
            .expect("refresh locate in flight");
        let project =
            crate::session_manager::project_scope::SessionProjectScope::exact("/repo/alpha")
                .expect("exact project scope");
        assert!(state.set_project_scope(project.clone()));
        assert!(state.pending_manifest.is_some());

        assert!(state.apply_query_manifest(
            scope_epoch,
            "claude",
            &base.generation,
            crate::session_manager::project_scope::SessionViewSpec::new(project, "needle"),
            query.generation,
            query.total_rows,
            query.first_page.page_index,
            query.first_page.rows,
            query_reader,
        ));
        assert!(state.pending_manifest.is_none());
        assert!(state.scan_active.is_none());
        assert!(!state.loading);
        assert!(state.rows_authoritative);

        let stale_page = base_reader.load_page(0).expect("stale located page");
        assert!(!state.finish_manifest_reconcile(
            locate_id,
            scope_epoch,
            &locate_generation,
            stale_page.page_index,
            stale_page.rows,
            0,
            base_reader,
        ));
        assert!(state.scan_active.is_none());
        assert!(!state.loading);
    }

    #[test]
    fn derived_view_is_stale_when_a_newer_base_generation_is_saved() {
        let directory = tempfile::tempdir().expect("manifest fixture directory");
        let base_store =
            crate::session_manager::paged_manifest::PagedManifestStore::open_at(directory.path())
                .expect("base manifest fixture store");
        let query_store = crate::session_manager::paged_manifest::QueryManifestStore::open_at(
            directory.path(),
            &crate::session_manager::paged_manifest::QueryManifestNamespace::for_test(
                "tui-derived-base-identity",
            ),
        )
        .expect("query manifest fixture store");
        let (base, base_reader) = publish_rows(&base_store, "claude", "base-old", 2);
        let mut query_builder = query_store
            .begin_build(&base_reader)
            .expect("query fixture builder");
        query_builder
            .push(meta("claude", "query-row", 2))
            .expect("query fixture row");
        let query = query_builder.publish().expect("query fixture");
        let view = crate::session_manager::project_scope::SessionViewSpec::new(
            crate::session_manager::project_scope::SessionProjectScope::exact("/repo/alpha")
                .expect("exact project"),
            "needle",
        );

        let mut state = SessionsState::default();
        let _ = state.start_scan("claude".to_string());
        let scope_epoch = state.scope_epoch;
        assert!(state.remember_base_manifest(
            scope_epoch,
            "claude",
            base.generation.clone(),
            base.total_rows,
            base_reader,
        ));
        assert!(state.apply_query_manifest(
            scope_epoch,
            "claude",
            &base.generation,
            view.clone(),
            query.generation,
            query.total_rows,
            query.first_page.page_index,
            query.first_page.rows,
            query.reader,
        ));
        state.project_scope = view.project.clone();
        assert!(state.materialized_view_is_current(Some("needle")));

        let (newer, newer_reader) = publish_rows(&base_store, "claude", "base-new", 3);
        assert!(state.remember_base_manifest(
            scope_epoch,
            "claude",
            newer.generation,
            newer.total_rows,
            newer_reader,
        ));

        assert!(!state.materialized_view_is_current(Some("needle")));
        assert!(state.stage_base_restore());
        assert_eq!(state.materialized_view.as_ref(), Some(&view));
    }

    #[test]
    fn consecutive_purges_merge_all_tombstone_cleanup_responsibilities() {
        let directory = tempfile::tempdir().expect("manifest fixture directory");
        let store =
            crate::session_manager::paged_manifest::PagedManifestStore::open_at(directory.path())
                .expect("manifest fixture store");
        let (initial, initial_reader) = publish_rows(&store, "claude", "old", 3);
        let mut state = SessionsState::default();
        let _request_id = state.start_scan("claude".to_string());
        let scope_epoch = state.scope_epoch;
        assert!(state.apply_opened_manifest(
            scope_epoch,
            "claude",
            initial.generation,
            initial.total_rows,
            initial.first_page.page_index,
            initial.first_page.rows,
            initial_reader,
        ));
        let first_key = crate::cli::tui::app::session_key(&state.rows[0]);
        let second_key = crate::cli::tui::app::session_key(&state.rows[1]);
        state.register_purge_tombstone(11, first_key.clone());
        state.register_purge_tombstone(12, second_key.clone());

        let (purged, purged_reader) = publish_rows(&store, "claude", "kept", 1);
        assert!(state.stage_purged_manifest(
            SessionPageSource::Base,
            scope_epoch,
            "claude",
            purged.generation.clone(),
            purged.total_rows,
            purged.first_page.rows.clone(),
            purged_reader.clone(),
            11,
            first_key.clone(),
        ));
        assert!(state.stage_purged_manifest(
            SessionPageSource::Base,
            scope_epoch,
            "claude",
            purged.generation.clone(),
            purged.total_rows,
            purged.first_page.rows,
            purged_reader.clone(),
            12,
            second_key.clone(),
        ));
        let (locate_id, _, _, generation, _, _, reader) = state
            .next_manifest_reconcile()
            .expect("merged purge reconcile");
        let page = reader.load_page(0).expect("purged page");
        assert!(state.finish_manifest_reconcile(
            locate_id,
            scope_epoch,
            &generation,
            page.page_index,
            page.rows,
            0,
            reader,
        ));

        assert!(!state.purge_tombstone_applies_to_scope(11, &first_key, "claude"));
        assert!(!state.purge_tombstone_applies_to_scope(12, &second_key, "claude"));
        assert!(state.purge_tombstone_applies_to_scope(11, &first_key, "all"));
        assert!(state.purge_tombstone_applies_to_scope(12, &second_key, "all"));
    }

    #[test]
    fn base_restore_adopts_query_tombstone_cleanup() {
        let directory = tempfile::tempdir().expect("manifest fixture directory");
        let store =
            crate::session_manager::paged_manifest::PagedManifestStore::open_at(directory.path())
                .expect("manifest fixture store");
        let (base, base_reader) = publish_rows(&store, "claude", "kept", 1);
        let mut state = SessionsState::default();
        let _request_id = state.start_scan("claude".to_string());
        let scope_epoch = state.scope_epoch;
        assert!(state.remember_base_manifest(
            scope_epoch,
            "claude",
            base.generation.clone(),
            base.total_rows,
            base_reader.clone(),
        ));
        assert!(state.apply_opened_manifest(
            scope_epoch,
            "claude",
            base.generation,
            base.total_rows,
            base.first_page.page_index,
            base.first_page.rows,
            base_reader,
        ));

        let deleted_key = crate::cli::tui::app::session_key(&meta("claude", "deleted", 2));
        state.register_purge_tombstone(21, deleted_key.clone());
        state.mark_query_tombstone_for_rebuild(21, deleted_key.clone());
        assert!(state.has_query_tombstones_to_clear());
        assert!(state.stage_base_restore());
        assert!(!state.has_query_tombstones_to_clear());

        let (locate_id, _, _, generation, _, _, reader) = state
            .next_manifest_reconcile()
            .expect("base restore reconcile");
        let page = reader.load_page(0).expect("safe base page");
        assert!(state.finish_manifest_reconcile(
            locate_id,
            scope_epoch,
            &generation,
            page.page_index,
            page.rows,
            0,
            reader,
        ));
        assert!(!state.purge_tombstone_applies_to_scope(21, &deleted_key, "claude"));
        assert!(state.purge_tombstone_applies_to_scope(21, &deleted_key, "all"));
    }

    #[test]
    fn failed_purge_reconcile_requests_authoritative_refresh() {
        let directory = tempfile::tempdir().expect("manifest fixture directory");
        let store =
            crate::session_manager::paged_manifest::PagedManifestStore::open_at(directory.path())
                .expect("manifest fixture store");
        let (safe, safe_reader) = publish_rows(&store, "claude", "kept", 1);
        let mut state = SessionsState::default();
        let _request_id = state.start_scan("claude".to_string());
        let scope_epoch = state.scope_epoch;
        let deleted_key = crate::cli::tui::app::session_key(&meta("claude", "deleted", 2));
        state.register_purge_tombstone(31, deleted_key.clone());
        assert!(state.stage_purged_manifest(
            SessionPageSource::Base,
            scope_epoch,
            "claude",
            safe.generation,
            safe.total_rows,
            safe.first_page.rows,
            safe_reader,
            31,
            deleted_key.clone(),
        ));
        let (locate_id, _, _, generation, _, _, _) =
            state.next_manifest_reconcile().expect("purge reconcile");

        assert!(state.fail_manifest_reconcile(
            locate_id,
            scope_epoch,
            &generation,
            "corrupt safe page".to_string(),
        ));
        assert!(state.purge_refresh_required());
        assert!(state.purge_tombstone_applies_to_scope(31, &deleted_key, "claude"));
    }

    #[test]
    fn delete_keeps_surviving_next_source_page_reachable_until_repack() {
        use crate::cli::tui::app::paged_list::{PageDirection, PagedListOutcome};

        let directory = tempfile::tempdir().expect("manifest fixture directory");
        let store =
            crate::session_manager::paged_manifest::PagedManifestStore::open_at(directory.path())
                .expect("manifest fixture store");
        let (published, reader) = publish_rows(&store, "claude", "row", 101);
        let mut state = SessionsState::default();
        let _request_id = state.start_scan("claude".to_string());
        let scope_epoch = state.scope_epoch;
        assert!(state.apply_opened_manifest(
            scope_epoch,
            "claude",
            published.generation,
            published.total_rows,
            published.first_page.page_index,
            published.first_page.rows,
            reader,
        ));

        for _ in 0..10 {
            let deleted_key = crate::cli::tui::app::session_key(&state.rows[0]);
            assert!(state.remove_session_by_key(&deleted_key));
        }
        assert_eq!(state.rows.len(), 90);
        assert_eq!(state.logical_total_rows(), 101);
        assert_eq!(state.pagination.page_count(), 2);

        state.selected_idx = state.rows.len() - 1;
        assert_eq!(
            state.selected_source_absolute(state.rows.len(), false),
            89,
            "moving up must use the collapsed visible position"
        );
        let source_edge = state.selected_source_absolute(state.rows.len(), true);
        assert_eq!(source_edge, 99);
        state.pagination.select(source_edge);
        assert!(matches!(
            state.pagination.line(PageDirection::Next),
            PagedListOutcome::BoundaryFocused { .. }
        ));
        let previous_gate = state.pagination.clone();
        let crossed = state.pagination.line(PageDirection::Next);
        assert!(matches!(
            crossed,
            PagedListOutcome::PageCrossed { to_page: 1, .. }
        ));
        assert!(!state.begin_page_cross(1, previous_gate, None));
        assert!(state.next_page_request(1).is_some());
    }

    #[test]
    fn failed_session_page_load_keeps_the_crossing_gesture_blocked() {
        use crate::cli::tui::app::paged_list::{PageDirection, PagedListOutcome};
        use crate::cli::tui::input::WheelGestureId;

        let directory = tempfile::tempdir().expect("manifest fixture directory");
        let store =
            crate::session_manager::paged_manifest::PagedManifestStore::open_at(directory.path())
                .expect("manifest fixture store");
        let (published, reader) = publish_rows(&store, "claude", "row", 101);
        let mut state = SessionsState::default();
        let _request_id = state.start_scan("claude".to_string());
        let scope_epoch = state.scope_epoch;
        assert!(state.apply_opened_manifest(
            scope_epoch,
            "claude",
            published.generation,
            published.total_rows,
            published.first_page.page_index,
            published.first_page.rows,
            reader,
        ));

        let armed_by = WheelGestureId::from_raw(1);
        assert!(matches!(
            state
                .pagination
                .wheel(PageDirection::Next, armed_by, usize::MAX),
            PagedListOutcome::BoundaryFocused { .. }
        ));

        let crossing = WheelGestureId::from_raw(2);
        let previous_gate = state.pagination.clone();
        assert!(matches!(
            state
                .pagination
                .wheel(PageDirection::Next, crossing, usize::MAX),
            PagedListOutcome::PageCrossed { to_page: 1, .. }
        ));
        assert!(!state.begin_page_cross(1, previous_gate, Some(crossing)));
        let (request_id, token, _reader) = state.next_page_request(1).expect("first page request");
        assert!(state.fail_page_request(
            request_id,
            &token,
            1,
            "transient read failure".to_string(),
        ));

        assert_eq!(
            state
                .pagination
                .wheel(PageDirection::Next, crossing, usize::MAX),
            PagedListOutcome::NoChange,
            "queued reports from the failed crossing gesture must not retry"
        );
        assert!(state.next_page_request(1).is_none());

        let retry = WheelGestureId::from_raw(3);
        let previous_gate = state.pagination.clone();
        assert!(matches!(
            state
                .pagination
                .wheel(PageDirection::Next, retry, usize::MAX),
            PagedListOutcome::PageCrossed { to_page: 1, .. }
        ));
        assert!(!state.begin_page_cross(1, previous_gate, Some(retry)));
        assert!(state.next_page_request(1).is_some());
        assert!(state.next_page_request(1).is_none());
    }

    /// 渐进回传：partial 只替换对应 provider 的行、保留其他 provider 的行，
    /// 结果按最近活跃排序；stale request id 被忽略。
    #[test]
    fn apply_partial_scan_replaces_only_that_provider() {
        let mut state = SessionsState::default();
        let request_id = state.start_scan("all".to_string());
        state.scan_accepts_previews = true;
        state.rows = vec![meta("claude", "c-old", 10), meta("codex", "x-1", 30)];

        assert!(state.apply_partial_scan(request_id, "claude", vec![meta("claude", "c-new", 20)],));
        let ids: Vec<&str> = state.rows.iter().map(|r| r.session_id.as_str()).collect();
        assert_eq!(ids, vec!["x-1", "c-new"]);
        assert!(state.loading, "refresh indicator must stay on");

        // stale request id：不应用
        assert!(!state.apply_partial_scan(request_id + 1, "codex", vec![]));
        assert_eq!(state.rows.len(), 2);
    }

    /// A scan completion is not a delete barrier: the tombstone remains until
    /// the separately repacked manifest is installed.
    #[test]
    fn scan_tombstone_blocks_deleted_session_from_inflight_scan() {
        let mut state = SessionsState::default();
        let request_id = state.start_scan("all".to_string());
        state.scan_accepts_previews = true;
        let a = meta("claude", "a", 10);
        let b = meta("codex", "b", 20);
        state.rows = vec![a.clone(), b.clone()];

        // 模拟"在途扫描期间删除 A"：从 rows 移除并登记 tombstone。键与删除流程
        // 一致（session_key = provider:session:source_path）。
        let key_a = crate::cli::tui::app::session_key(&a);
        state.rows.retain(|row| row.session_id != "a");
        state.scan_tombstones.insert(key_a.clone());

        // 删除前读到旧列表的 partial 把 A 带回 —— 必须被过滤掉。
        assert!(state.apply_partial_scan(request_id, "claude", vec![a.clone()]));
        assert!(
            state.rows.iter().all(|row| row.session_id != "a"),
            "partial 不得复活已删会话"
        );

        // 终态同样带回 A（扫描线程在删除前就读完文件）—— 仍过滤；只有安全
        // manifest 安装后才能释放 tombstone。
        assert!(state.finish_scan(request_id, vec![a.clone(), b.clone()]));
        assert!(
            state.rows.iter().all(|row| row.session_id != "a"),
            "finish_scan 不得复活已删会话"
        );
        assert!(state.scan_tombstones.contains(&key_a));

        // 尚未安装安全 manifest 前，即使下一轮结果带回同 key 也继续过滤。
        let request_id2 = state.start_scan("all".to_string());
        assert!(state.finish_scan(request_id2, vec![a.clone(), b.clone()]));
        assert!(state.rows.iter().all(|row| row.session_id != "a"));
    }

    /// A failed scan must not clear a delete tombstone: its input may predate
    /// the deletion and it proves nothing about manifest repacking.
    #[test]
    fn fail_scan_preserves_delete_tombstones() {
        let mut state = SessionsState::default();
        let request_id = state.start_scan("all".to_string());
        let a = meta("claude", "a", 10);
        state
            .scan_tombstones
            .insert(crate::cli::tui::app::session_key(&a));

        state.fail_scan(request_id, "boom".to_string());
        assert!(!state.scan_tombstones.is_empty());

        // 失败后下一轮结果带回同 key 时仍受 tombstone 保护。
        let request_id2 = state.start_scan("all".to_string());
        assert!(state.finish_scan(request_id2, vec![a.clone()]));
        assert!(state.rows.iter().all(|row| row.session_id != "a"));
    }

    #[test]
    fn failed_scan_with_only_provisional_rows_keeps_terminal_error() {
        let mut state = SessionsState::default();
        let request_id = state.start_scan("claude".to_string());
        let scope_epoch = state.scope_epoch;
        assert!(state.apply_provisional_page(
            request_id,
            scope_epoch,
            "claude",
            vec![meta("claude", "partial", 10)],
        ));

        state.fail_scan(request_id, "manifest publish failed".to_string());

        assert_eq!(state.rows.len(), 1);
        assert!(state.page_token().is_none());
        assert_eq!(state.last_error.as_deref(), Some("manifest publish failed"));
    }

    /// 秒开快照同样按 tombstone 过滤：删除成功后即使 stale 快照带回已删会话，也
    /// 不渲染。
    #[test]
    fn scan_tombstone_filters_cached_snapshot() {
        let mut state = SessionsState::default();
        let request_id = state.start_scan("all".to_string());
        state.scan_accepts_previews = true;
        let a = meta("claude", "a", 10);
        let b = meta("codex", "b", 20);
        state
            .scan_tombstones
            .insert(crate::cli::tui::app::session_key(&a));

        assert!(state.apply_cached_snapshot(request_id, vec![a, b]));
        let ids: Vec<&str> = state.rows.iter().map(|r| r.session_id.as_str()).collect();
        assert_eq!(ids, vec!["b"], "快照应过滤掉 tombstoned 会话");
    }

    #[test]
    fn authoritative_finish_replaces_bounded_partials() {
        let mut state = SessionsState::default();
        let request_id = state.start_scan("all".to_string());
        state.scan_accepts_previews = true;
        assert!(state.apply_partial_scan(request_id, "claude", vec![meta("claude", "a", 10)]));
        assert!(state.apply_partial_scan(request_id, "codex", vec![meta("codex", "b", 20)]));

        assert!(state.finish_scan(
            request_id,
            vec![meta("codex", "b", 20), meta("claude", "a", 10)]
        ));

        let ids: Vec<&str> = state
            .rows
            .iter()
            .map(|row| row.session_id.as_str())
            .collect();
        assert_eq!(ids, vec!["b", "a"]);
        assert!(!state.loading);
        assert!(state.loaded_once);
        assert!(state.scan_active.is_none());
    }

    #[test]
    fn partial_scan_hard_limits_accidentally_unbounded_worker_input() {
        let mut state = SessionsState::default();
        let request_id = state.start_scan("all".to_string());
        state.scan_accepts_previews = true;
        let rows = (0..1_000)
            .map(|index| meta("claude", &format!("s-{index}"), index))
            .collect();

        assert!(state.apply_partial_scan(request_id, "claude", rows));

        assert_eq!(
            state.rows.len(),
            crate::session_manager::SCAN_CACHE_FIRST_PAINT_LIMIT
        );
    }

    #[test]
    fn early_progress_does_not_collapse_existing_provider_snapshot() {
        let mut state = SessionsState::default();
        let request_id = state.start_scan("all".to_string());
        state.scan_accepts_previews = true;
        let cached: Vec<_> = (0..crate::session_manager::SCAN_CACHE_FIRST_PAINT_LIMIT)
            .map(|index| meta("claude", &format!("cached-{index}"), index as i64))
            .collect();
        assert!(state.apply_cached_snapshot(request_id, cached));

        assert!(!state.apply_progressive_preview(
            request_id,
            "claude",
            meta("claude", "arbitrary-first", 9_999)
        ));

        assert_eq!(
            state.rows.len(),
            crate::session_manager::SCAN_CACHE_FIRST_PAINT_LIMIT
        );
        assert!(state
            .rows
            .iter()
            .all(|row| row.session_id != "arbitrary-first"));
    }

    #[test]
    fn early_progress_adds_first_row_for_missing_provider_only() {
        let mut state = SessionsState::default();
        let request_id = state.start_scan("all".to_string());
        state.scan_accepts_previews = true;

        assert!(state.apply_progressive_preview(request_id, "claude", meta("claude", "first", 1)));
        assert!(!state.apply_progressive_preview(
            request_id,
            "claude",
            meta("claude", "second", 2)
        ));
        assert_eq!(state.rows.len(), 1);
        assert_eq!(state.rows[0].session_id, "first");
    }

    #[test]
    fn refresh_of_authoritative_rows_ignores_progressive_previews() {
        let mut state = SessionsState::default();
        let first = state.start_scan("all".to_string());
        let authoritative = (0..200)
            .map(|index| meta("claude", &format!("old-{index}"), index))
            .collect();
        assert!(state.finish_scan(first, authoritative));

        let refresh = state.start_scan("all".to_string());
        assert!(!state.apply_partial_scan(
            refresh,
            "claude",
            vec![meta("claude", "preview", 9_999)]
        ));

        assert_eq!(state.rows.len(), 200);
        assert!(state.rows.iter().all(|row| row.session_id != "preview"));
    }

    #[test]
    fn filtered_session_indices_are_reused_until_rows_change() {
        let mut state = SessionsState::default();
        state.provider_id = Some("claude".to_string());
        state.rows = vec![meta("claude", "match", 2), meta("claude", "other", 1)];
        let mut filter = FilterState::new();
        filter.input.set("match");

        for _ in 0..2 {
            let view = crate::cli::tui::app::visible_sessions_for_state(
                &filter,
                &AppType::Claude,
                state.provider_id.as_deref(),
                &state.project_scope,
                &state.rows,
                state.detail_key.as_deref(),
                state.messages_loaded,
                &state.messages,
                state.deep_search_query.as_deref(),
                &state.deep_search_results,
                state.materialized_view_is_current(filter.query_lower().as_deref()),
                state.rows_revision,
                state.messages_revision,
                state.deep_search_seq,
                &state.visibility_cache,
            );
            assert_eq!(view.len(), 1);
        }
        assert_eq!(state.visibility_cache.borrow().rebuilds(), 1);

        state.rows[1].session_id = "match-too".to_string();
        state.rows_revision = state.rows_revision.wrapping_add(1);
        let view = crate::cli::tui::app::visible_sessions_for_state(
            &filter,
            &AppType::Claude,
            state.provider_id.as_deref(),
            &state.project_scope,
            &state.rows,
            state.detail_key.as_deref(),
            state.messages_loaded,
            &state.messages,
            state.deep_search_query.as_deref(),
            &state.deep_search_results,
            state.materialized_view_is_current(filter.query_lower().as_deref()),
            state.rows_revision,
            state.messages_revision,
            state.deep_search_seq,
            &state.visibility_cache,
        );
        assert_eq!(view.len(), 2);
        assert_eq!(state.visibility_cache.borrow().rebuilds(), 2);
    }

    #[test]
    fn filtered_message_indices_are_reused_until_messages_change() {
        let mut state = SessionsState::default();
        state.message_filter.set("needle");
        state.messages = vec![
            SessionMessage {
                role: "user".to_string(),
                content: "needle one".to_string(),
                ts: None,
            },
            SessionMessage {
                role: "assistant".to_string(),
                content: "other".to_string(),
                ts: None,
            },
        ];

        for _ in 0..2 {
            assert_eq!(
                crate::cli::tui::app::visible_session_messages(&state).len(),
                1
            );
        }
        assert_eq!(state.message_visibility_cache.borrow().rebuilds(), 1);

        state.messages[1].content = "needle two".to_string();
        state.messages_revision = state.messages_revision.wrapping_add(1);
        assert_eq!(
            crate::cli::tui::app::visible_session_messages(&state).len(),
            2
        );
        assert_eq!(state.message_visibility_cache.borrow().rebuilds(), 2);
    }
}
