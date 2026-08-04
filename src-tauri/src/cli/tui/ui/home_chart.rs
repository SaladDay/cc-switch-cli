//! Home page usage card: a stacked "tokens per day" chart on the left and a
//! "models by cost" list on the right, wrapped in the shared home card chrome.

use std::cell::RefCell;
use std::rc::Rc;

use crate::cli::tui::data::{
    UsageDailyChartDay, UsageDailyChartSeries, UsageModelTokenBreakdown, UsageRangePreset,
    UsageSnapshot, UsageSnapshotGeneration,
};

use super::*;

/// Models drawn individually before the rest collapses into "Other".
pub(super) const HOME_CHART_MAX_MODELS: usize = crate::cli::tui::data::USAGE_DAILY_MODEL_LIMIT;
/// Minimum columns reserved for the y-axis label plus its `│` separator.
/// Larger compact values (for example `1494.9M`) expand this dynamically.
const MIN_Y_AXIS_WIDTH: u16 = 7;
/// Narrowest chart column that can still host the full 30-day bars.
const MIN_BAR_CHART_WIDTH: u16 = 44;
/// Below the split-chart threshold the model list owns the card. At widths
/// below this floor even its compact name/share/cost row is no longer useful.
const MIN_LIST_ONLY_WIDTH: u16 = 24;
/// Optical breathing room between bars and axes. One blank column separates
/// the first bar from the y-axis. The x-axis needs no whole blank row because
/// its centered stroke already leaves roughly half a terminal row below the
/// bars, which is visually comparable on cells that are taller than wide.
const BAR_AXIS_INSET_COLUMNS: u16 = 1;
const BAR_AXIS_INSET_ROWS: u16 = 0;
/// Rows a bar chart needs besides the bars: optional inset, axis, and labels.
const BAR_CHART_FIXED_ROWS: u16 = BAR_AXIS_INSET_ROWS + 2;
/// Blank columns kept between each card rail and the content.
const CARD_PAD_X: u16 = 1;
/// Blank rows kept under the title rail. The bottom rail stays flush, so the
/// card only ever spends one row on breathing space.
const CARD_PAD_TOP: u16 = 1;
/// The dim rule between the chart column and the list column.
const LIST_RULE_WIDTH: u16 = 1;
/// Share of card content requested by the model-cost list. This is the former
/// 46% allocation widened by 20%, then trimmed by 5% relative to that result.
const LIST_WIDTH_PERCENT: u16 = 52;
/// The former 34..=52 list-width clamp follows the same net 14% expansion.
const LIST_MIN_WIDTH: u16 = 39;
const LIST_MAX_WIDTH: u16 = 59;
/// Narrowest content width that can hold both minimum columns without overlap.
const LIST_MIN_CONTENT_WIDTH: u16 = MIN_BAR_CHART_WIDTH + LIST_RULE_WIDTH + LIST_MIN_WIDTH;
/// Columns the list reserves for the share percentage (` 42%`).
const LIST_SHARE_WIDTH: usize = 4;
/// Columns the detail line is indented by. The model name starts at column 3
/// (` ● `); one more column reads as a hanging indent and still leaves a
/// realistic six-character counter set (`In: 12.5M • Out: 16.1M • CR: 975.9M •
/// CW: 109.7M` = 52) fitting comfortably in the expanded list.
const LIST_DETAIL_INDENT: usize = 4;
/// Narrowest list column that can host the `In/Out/CR/CW` detail line: the
/// indent plus four `Label: 12.3M` chunks plus their three separators.
/// Below this the rows fall back to one line each.
const LIST_DETAIL_MIN_WIDTH: u16 = 44;
/// From this column width on, bars keep a one-column gutter between days.
const BAR_GUTTER_MIN_SPAN: usize = 3;

const BLOCKS_UNICODE: [&str; 9] = [" ", "▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"];
const BLOCKS_ASCII: [&str; 9] = [" ", ".", ":", "-", "=", "+", "*", "%", "#"];

/// Categorical series palette. Muted Dracula-adjacent hues, deliberately less
/// saturated than the status colors: ok/warn/err stay reserved for status, so
/// a model can never read as "healthy" or "failing". The four entries survive
/// 256-color quantization as distinct indices (104 / 73 / 175 / 180) and are
/// routed through [`Theme::shade`] so NoColor degrades with everything else.
const SERIES_PALETTE: [(u8, u8, u8); 4] = [
    (122, 148, 205), // periwinkle
    (99, 170, 165),  // teal
    (198, 145, 172), // dusty rose
    (196, 166, 124), // sand
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum HomeChartMode {
    Hidden,
    /// Responsive fallback: preserve the actionable model list and drop the
    /// chart entirely.
    ListOnly,
    /// Full stacked bar chart.
    Bars,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct HomeChartGeometry {
    pub mode: HomeChartMode,
    /// Rows available to the bars themselves (0 outside [`HomeChartMode::Bars`]).
    pub bar_rows: u16,
    /// Columns owned by the chart column, y-axis labels included.
    pub chart_width: u16,
    /// Columns owned by the model list: the full card in list-only mode.
    pub list_width: u16,
    /// Columns reserved for the maximum-value label plus its axis separator.
    pub y_axis_width: u16,
}

impl HomeChartGeometry {
    const HIDDEN: Self = Self {
        mode: HomeChartMode::Hidden,
        bar_rows: 0,
        chart_width: 0,
        list_width: 0,
        y_axis_width: 0,
    };
}

/// Content rows left inside a card body of `inner_height` rows.
///
/// The top pad is the first thing to go: a body of a single row spends it on
/// content rather than on breathing space.
pub(super) fn card_content_height(inner_height: u16) -> u16 {
    inner_height.saturating_sub(CARD_PAD_TOP.min(inner_height.saturating_sub(1)))
}

/// Content columns left inside a card body of `inner_width` columns.
pub(super) fn card_content_width(inner_width: u16) -> u16 {
    inner_width.saturating_sub(CARD_PAD_X.saturating_mul(2))
}

/// The padded content rect of a card body.
fn card_content_area(inner: Rect) -> Rect {
    let height = card_content_height(inner.height);
    let width = card_content_width(inner.width);
    Rect {
        x: inner.x.saturating_add(CARD_PAD_X),
        y: inner.y.saturating_add(inner.height.saturating_sub(height)),
        width,
        height,
    }
}

/// Width of the maximum-value label plus the axis separator. The previous
/// fixed six-character label slot made `1494.9M│` one column wider than every
/// other row, visibly shifting only the top bar line.
pub(super) fn y_axis_width(max_total: u64) -> u16 {
    let label = format_token_compact(max_total);
    let required = UnicodeWidthStr::width(label.as_str()).saturating_add(1);
    u16::try_from(required)
        .unwrap_or(u16::MAX)
        .max(MIN_Y_AXIS_WIDTH)
}

/// Pick the richest presentation that fits the card's padded `width` x
/// `height` content area for `days` columns and `model_rows` named/residual
/// series.
///
/// The model list is the responsive invariant: shrinking either dimension
/// removes the chart first. A split view is used only when the complete list
/// and the full multi-series bar chart both fit; all smaller useful areas
/// render the list alone. There is deliberately no single-color sparkline
/// fallback because it discards the model comparison the card exists to show.
///
/// Width ladder: from [`LIST_MIN_CONTENT_WIDTH`] on, the card splits into a
/// chart column and a model list when its header plus every model name row also
/// fit vertically. Below either threshold the list owns the full width.
pub(super) fn home_chart_geometry(
    width: u16,
    height: u16,
    days: usize,
    model_rows: usize,
    max_total: u64,
) -> HomeChartGeometry {
    if height == 0 || width < MIN_LIST_ONLY_WIDTH || days == 0 {
        return HomeChartGeometry::HIDDEN;
    }

    let y_axis_width = y_axis_width(max_total);
    let list_only = HomeChartGeometry {
        mode: HomeChartMode::ListOnly,
        list_width: width,
        y_axis_width,
        ..HomeChartGeometry::HIDDEN
    };

    // A split view only earns chart columns when its list can keep every model
    // name (one header + one row per series). Otherwise the list takes the card
    // and marks any vertically hidden rows in its header.
    let list_rows_fit = model_rows > 0 && usize::from(height).saturating_sub(1) >= model_rows;
    if width < LIST_MIN_CONTENT_WIDTH || height <= BAR_CHART_FIXED_ROWS || !list_rows_fit {
        return list_only;
    }

    let list_width = list_width_for_split(width);
    let chart_width = width
        .saturating_sub(list_width)
        .saturating_sub(LIST_RULE_WIDTH);
    if chart_width < MIN_BAR_CHART_WIDTH {
        return list_only;
    }

    let Some(bar_rows) = height
        .checked_sub(BAR_CHART_FIXED_ROWS)
        .filter(|rows| *rows > 0)
    else {
        return list_only;
    };

    let axis_body_width = usize::from(chart_width.saturating_sub(y_axis_width));
    if bar_plot_width(axis_body_width) < days {
        return list_only;
    }

    HomeChartGeometry {
        mode: HomeChartMode::Bars,
        bar_rows,
        chart_width,
        list_width,
        y_axis_width,
    }
}

/// Responsive model-list width while preserving the chart's readable floor.
///
/// The list receives its widened percentage on roomy terminals. Near the split
/// threshold it yields only the excess columns, keeping
/// [`MIN_BAR_CHART_WIDTH`] intact instead of making the whole chart disappear.
fn list_width_for_split(content_width: u16) -> u16 {
    let desired = (content_width.saturating_mul(LIST_WIDTH_PERCENT) / 100)
        .clamp(LIST_MIN_WIDTH, LIST_MAX_WIDTH);
    let chart_safe_max = content_width.saturating_sub(MIN_BAR_CHART_WIDTH + LIST_RULE_WIDTH);
    desired.min(chart_safe_max)
}

/// Columns available to the day bars after the visual axis inset.
fn bar_plot_width(axis_body_width: usize) -> usize {
    axis_body_width.saturating_sub(usize::from(BAR_AXIS_INSET_COLUMNS))
}

/// Start column and width of day `index` inside a `chart_width`-wide body.
///
/// Every body column belongs to exactly one day, so the bars reach both edges
/// of the card no matter how the remainder falls.
pub(super) fn bar_span(index: usize, day_count: usize, chart_width: usize) -> (usize, usize) {
    if day_count == 0 || chart_width == 0 {
        return (0, 0);
    }
    let start = index.saturating_mul(chart_width) / day_count;
    let end = index.saturating_add(1).saturating_mul(chart_width) / day_count;
    (start, end.saturating_sub(start))
}

/// Whether day columns are wide enough to keep a one-column gutter.
///
/// The decision is taken once for the whole chart: remainder distribution
/// makes individual columns differ by one, and gapping only the wide ones
/// would scatter gutters at irregular intervals.
pub(super) fn use_bar_gutter(day_count: usize, body_width: usize) -> bool {
    day_count > 0 && body_width / day_count >= BAR_GUTTER_MIN_SPAN
}

/// Cells actually painted inside a `span`-wide column.
pub(super) fn bar_fill_width(span: usize, gutter: bool) -> usize {
    if gutter {
        span.saturating_sub(1).max(1)
    } else {
        span
    }
}

/// Everything the card derives from one usage snapshot, built once per
/// snapshot instead of once per frame: the stacked series, the entity→palette
/// mapping, and the rank-ordered list rows. All three come from the same
/// cost-ranked entity set, so they cannot drift apart.
#[derive(Debug, Default)]
pub(super) struct HomeChartProjection {
    pub series: UsageDailyChartSeries,
    pub slots: Vec<Option<usize>>,
    pub rows: Vec<ModelCostRow>,
}

/// What makes a cached projection valid: the snapshot it was built from, plus
/// the two build parameters that are not part of the snapshot. The label is
/// locale-dependent, so a language switch has to invalidate too.
#[derive(Debug, PartialEq, Eq)]
struct ProjectionKey {
    generation: UsageSnapshotGeneration,
    max_models: usize,
    other_label: &'static str,
}

thread_local! {
    /// One slot: the home card renders one snapshot at a time, and the tick
    /// loop asks for the same one ~5x/s. Rendering is single-threaded, so a
    /// thread-local beats a lock; `render` only ever sees `&App`/`&UiData`,
    /// which is why the memo lives here rather than in app state.
    static PROJECTION_CACHE: RefCell<Option<(ProjectionKey, Rc<HomeChartProjection>)>> =
        const { RefCell::new(None) };
}

#[cfg(test)]
thread_local! {
    static PROJECTION_BUILDS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// How many times [`home_chart_projection`] actually rebuilt on this thread.
#[cfg(test)]
pub(super) fn projection_build_count() -> u64 {
    PROJECTION_BUILDS.with(std::cell::Cell::get)
}

/// Drop the memo so a test can count builds from a known baseline.
#[cfg(test)]
pub(super) fn reset_projection_cache() {
    PROJECTION_CACHE.with(|cache| *cache.borrow_mut() = None);
    PROJECTION_BUILDS.with(|builds| builds.set(0));
}

/// The card's derived state for `usage`, rebuilt only when the snapshot (or the
/// locale that names the residual bucket) changes.
pub(super) fn home_chart_projection(usage: &UsageSnapshot) -> Rc<HomeChartProjection> {
    let key = ProjectionKey {
        generation: usage.generation,
        max_models: HOME_CHART_MAX_MODELS,
        other_label: texts::tui_home_chart_other(),
    };

    PROJECTION_CACHE.with(|cache| {
        if let Some((cached_key, projection)) = cache.borrow().as_ref() {
            if *cached_key == key {
                return Rc::clone(projection);
            }
        }

        #[cfg(test)]
        PROJECTION_BUILDS.with(|builds| builds.set(builds.get().saturating_add(1)));

        let series = crate::cli::tui::data::build_usage_daily_chart_series(
            usage.trend_for(UsageRangePreset::ThirtyDays),
            &usage.daily_models,
            key.max_models,
            key.other_label,
        );
        let slots = entity_palette_slots(&series);
        let rows = model_cost_rows(&series, &slots);
        let projection = Rc::new(HomeChartProjection {
            series,
            slots,
            rows,
        });
        *cache.borrow_mut() = Some((key, Rc::clone(&projection)));
        projection
    })
}

pub(super) fn render_home_usage_chart(
    frame: &mut Frame<'_>,
    app: &App,
    data: &UiData,
    area: Rect,
    theme: &super::theme::Theme,
    card_border: Style,
) {
    // A bordered card needs both rails plus at least one content row.
    if area.width < 4 || area.height < 3 {
        return;
    }

    let projection = home_chart_projection(&data.usage);
    let series = &projection.series;
    let slots = projection.slots.as_slice();
    // The block is built from the geometry, so the content dims are derived
    // from `area` here rather than read back off `block.inner()`.
    let geometry = home_chart_geometry(
        card_content_width(area.width.saturating_sub(2)),
        card_content_height(area.height.saturating_sub(2)),
        series.days.len().max(1),
        series.models.len(),
        series.max_total,
    );

    let title = card_title();
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(card_border)
        .title(title.clone());

    let loading = app
        .usage
        .is_loading_for(&app.app_type, UsageRangePreset::ThirtyDays);
    // One card, one indicator. The rail and the empty body are different rows,
    // but they are the same section: when the body is about to spin for the
    // first aggregate, the rail drops back to its resting status instead of
    // spinning alongside it. `home_chart_geometry` returns `Hidden` for exactly
    // the sizes where the body draws nothing, so the rail keeps the indicator
    // there — otherwise the card would go silent mid-import.
    let body_owns_indicator =
        !matches!(geometry.mode, HomeChartMode::Hidden) && !series.has_data() && loading;

    // Secondary header info shares the title rail, like the proxy card does.
    let title_width = UnicodeWidthStr::width(title.as_str()) as u16;
    let status = title_status_spans(
        app,
        data,
        theme,
        body_owns_indicator,
        loading,
        area.width.saturating_sub(title_width),
    );
    let status_width = spans_display_width(&status) as u16;
    if status_width > 0 && title_width.saturating_add(status_width) <= area.width {
        block = block.title_top(Line::from(status).alignment(Alignment::Right));
    }

    let content = card_content_area(block.inner(area));
    frame.render_widget(block, area);
    if content.width == 0 || content.height == 0 || matches!(geometry.mode, HomeChartMode::Hidden) {
        return;
    }

    if !series.has_data() {
        if loading {
            render_centered_line(frame, content, shared_loading_line(app, theme));
        } else {
            render_centered_line(
                frame,
                content,
                Line::styled(
                    home_chart_empty_hint(&app.app_type),
                    Style::default().fg(theme.comment),
                ),
            );
        }
        return;
    }

    match geometry.mode {
        HomeChartMode::Hidden => {}
        HomeChartMode::ListOnly => {
            render_model_cost_list(frame, content, theme, series, &projection.rows);
        }
        HomeChartMode::Bars => {
            let chart_area = Rect {
                width: geometry.chart_width.min(content.width),
                ..content
            };
            render_bars(frame, chart_area, theme, series, slots, &geometry);

            if geometry.list_width > 0 && content.width > geometry.chart_width {
                render_list_rule(
                    frame,
                    Rect {
                        x: content.x + geometry.chart_width,
                        width: LIST_RULE_WIDTH,
                        ..content
                    },
                    theme,
                );
                let list_x = content.x + geometry.chart_width + LIST_RULE_WIDTH;
                let available = (content.x + content.width).saturating_sub(list_x);
                let list_width = geometry.list_width.min(available);
                if list_width > 0 {
                    render_model_cost_list(
                        frame,
                        Rect {
                            x: list_x,
                            width: list_width,
                            ..content
                        },
                        theme,
                        series,
                        &projection.rows,
                    );
                }
            }
        }
    }
}

fn card_title() -> String {
    format!(
        " {}{}{} ",
        texts::tui_home_chart_card_title(),
        separator(),
        texts::tui_home_chart_card_range()
    )
}

fn shared_loading_line(app: &App, theme: &super::theme::Theme) -> Line<'static> {
    loading_indicator_line(
        app.tick,
        theme,
        texts::tui_refreshing(),
        sync_escalation(app),
    )
}

fn render_centered_line(frame: &mut Frame<'_>, area: Rect, line: Line<'static>) {
    let top = area.height / 2;
    let target = Rect {
        y: area.y + top,
        height: 1,
        ..area
    };
    frame.render_widget(Paragraph::new(line).alignment(Alignment::Center), target);
}

/// Right side of the card title rail: an in-flight import wins over the
/// live-proxy badge, which wins over the last local-import timestamp — see
/// `sync_escalation` for the one case that earns a number.
///
/// `body_owns_indicator` hands the import over to the card body, which is
/// already spinning for the same wait. A rail too narrow for the label falls
/// back to the glyph alone rather than dropping the signal.
fn title_status_spans(
    app: &App,
    data: &UiData,
    theme: &super::theme::Theme,
    body_owns_indicator: bool,
    usage_refreshing: bool,
    available: u16,
) -> Vec<Span<'static>> {
    // The two pad spaces below come out of the same budget.
    let indicator = (!body_owns_indicator)
        .then(|| {
            session_sync_indicator_spans(app, theme).or_else(|| {
                usage_refreshing.then(|| refresh_indicator_spans(app.tick, theme, None))
            })
        })
        .flatten()
        .map(|spans| {
            if spans_display_width(&spans).saturating_add(2) <= available as usize {
                spans
            } else {
                vec![refresh_spinner_span(app.tick, theme)]
            }
        });

    let mut spans = if let Some(indicator) = indicator {
        indicator
    } else if data
        .proxy
        .routes_current_app_through_proxy(&app.app_type)
        .unwrap_or(false)
    {
        vec![Span::styled(
            format!("{} {}", status_dot(), texts::tui_home_chart_live()),
            Style::default().fg(theme.ok),
        )]
    } else {
        let text = match data.usage.last_synced_at {
            Some(ts) => texts::tui_home_chart_last_updated(&format_relative_since(
                Local::now().timestamp(),
                ts,
            )),
            None => texts::tui_home_chart_never_synced().to_string(),
        };
        vec![Span::styled(
            format!("{} {text}", status_dot()),
            Style::default().fg(theme.comment),
        )]
    };

    // Keep the rail text off the border corners.
    spans.insert(0, Span::raw(" "));
    spans.push(Span::raw(" "));
    spans
}

fn status_dot() -> &'static str {
    if icons::use_emoji() {
        "•"
    } else {
        "*"
    }
}

/// Per-slot fallback glyphs for the legend and list dots.
///
/// `●` only tells series apart by *color*. In ASCII icon mode there is no `●`,
/// and in NoColor mode there is no color — either way a single shared glyph
/// would leave four identical dots and no way to match a legend entry to a list
/// row. These four are visually distinct at one cell and share no shape.
const SERIES_ASCII_GLYPHS: [&str; 4] = ["*", "#", "%", "@"];
/// The residual "Other" bucket: quieter than any named series, like the dim ink
/// it is drawn in.
const SERIES_ASCII_OTHER_GLYPH: &str = ".";

/// Legend/list dot for a palette slot. `None` is the residual bucket.
///
/// Falls back to [`SERIES_ASCII_GLYPHS`] whenever the round dot could not be
/// told apart: ASCII icon mode has no `●` to draw, and NoColor mode draws every
/// `●` in the same ink.
fn legend_dot(theme: &super::theme::Theme, slot: Option<usize>) -> &'static str {
    if icons::use_emoji() && !theme.no_color {
        return "●";
    }
    match slot {
        Some(slot) => SERIES_ASCII_GLYPHS[slot % SERIES_ASCII_GLYPHS.len()],
        None => SERIES_ASCII_OTHER_GLYPH,
    }
}

/// Widest legend/list dot, for layouts that reserve the column before they know
/// which slot fills it. Every candidate glyph is one cell wide today; the max
/// keeps that assumption honest if one ever is not.
fn legend_dot_width() -> usize {
    SERIES_ASCII_GLYPHS
        .iter()
        .chain(std::iter::once(&SERIES_ASCII_OTHER_GLYPH))
        .chain(std::iter::once(&"●"))
        .map(|glyph| UnicodeWidthStr::width(*glyph))
        .max()
        .unwrap_or(1)
}

fn separator() -> &'static str {
    if icons::use_emoji() {
        " · "
    } else {
        " - "
    }
}

/// Coarse "x ago" label. Anything in the future (clock skew) reads as "just now".
pub(super) fn format_relative_since(now: i64, timestamp: i64) -> String {
    let delta = now.saturating_sub(timestamp);
    if delta < 60 {
        return texts::tui_home_chart_just_now().to_string();
    }
    let minutes = delta / 60;
    if minutes < 60 {
        return texts::tui_home_chart_minutes_ago(minutes as u64);
    }
    let hours = minutes / 60;
    if hours < 24 {
        return texts::tui_home_chart_hours_ago(hours as u64);
    }
    texts::tui_home_chart_days_ago((hours / 24) as u64)
}

fn home_chart_empty_hint(app_type: &AppType) -> String {
    if matches!(app_type, AppType::Hermes | AppType::OpenClaw) {
        texts::tui_home_chart_empty_proxy_only().to_string()
    } else {
        texts::tui_home_chart_empty_pending().to_string()
    }
}

fn blocks() -> &'static [&'static str; 9] {
    if icons::use_emoji() {
        &BLOCKS_UNICODE
    } else {
        &BLOCKS_ASCII
    }
}

/// Palette slot per model, parallel to [`UsageDailyChartSeries::models`] — the
/// cost-ranked top-N plus the residual bucket, i.e. the one entity set the
/// bars and the list both draw from.
///
/// Slots follow the *entity*, not the rank: they are handed out along the
/// alphabetically sorted list of top-model names, so a model keeps its color
/// when refreshes reorder the ranking. `None` marks the residual bucket.
///
/// KNOWN LIMIT: stability covers re-ordering only. When the top-N *membership*
/// changes — a model enters or leaves the kept set — the alphabetical run
/// shifts and the surviving models can be repainted. Pinning colors across
/// membership changes would need a palette assignment that outlives the series,
/// which is more state than a 30-day card is worth.
pub(super) fn entity_palette_slots(series: &UsageDailyChartSeries) -> Vec<Option<usize>> {
    let mut named = series
        .models
        .iter()
        .enumerate()
        .filter(|(index, _)| series.other_index != Some(*index))
        .map(|(_, model)| model.as_str())
        .collect::<Vec<_>>();
    named.sort_unstable();

    series
        .models
        .iter()
        .enumerate()
        .map(|(index, model)| {
            if series.other_index == Some(index) {
                None
            } else {
                named.binary_search(&model.as_str()).ok()
            }
        })
        .collect()
}

fn series_style(theme: &super::theme::Theme, slot: Option<usize>) -> Style {
    if theme.no_color {
        return Style::default();
    }
    let color = match slot {
        Some(slot) => theme.shade(SERIES_PALETTE[slot % SERIES_PALETTE.len()]),
        None => theme.comment,
    };
    Style::default().fg(color)
}

/// Glyph for one occupied bar cell.
///
/// Color mode uses the eight-level block ramp so the top row keeps its
/// sub-cell precision. With color disabled, height is already encoded by the
/// occupied rows; using the same per-series glyph as the model list preserves
/// the model identity that color would otherwise carry.
fn bar_cell_glyph(
    theme: &super::theme::Theme,
    level: usize,
    palette_slot: Option<usize>,
) -> &'static str {
    let ramp = blocks();
    let level = level.min(ramp.len() - 1);
    if level == 0 {
        ramp[0]
    } else if theme.no_color {
        legend_dot(theme, palette_slot)
    } else {
        ramp[level]
    }
}

fn slot_at(slots: &[Option<usize>], index: usize) -> Option<usize> {
    slots.get(index).copied().flatten()
}

/// Bound a legend/list label. The shared truncation marks cuts with `…`, which
/// must not leak into ASCII mode.
fn truncate_label(text: &str, width: u16) -> String {
    let label = truncate_to_display_width(text, width);
    if icons::use_emoji() {
        label
    } else {
        label.replace('…', "~")
    }
}

fn render_bars(
    frame: &mut Frame<'_>,
    area: Rect,
    theme: &super::theme::Theme,
    series: &UsageDailyChartSeries,
    slots: &[Option<usize>],
    geometry: &HomeChartGeometry,
) {
    let bar_rows = geometry
        .bar_rows
        .min(area.height.saturating_sub(BAR_CHART_FIXED_ROWS));
    if bar_rows == 0 || series.days.is_empty() {
        return;
    }

    let day_count = series.days.len();
    let axis_body_width = area.width.saturating_sub(geometry.y_axis_width) as usize;
    let plot_width = bar_plot_width(axis_body_width);
    if plot_width == 0 {
        return;
    }
    let gutter = use_bar_gutter(day_count, plot_width);
    let axis_style = Style::default().fg(theme.dim);
    let label_style = Style::default().fg(theme.comment);
    let max_value = series.max_total.max(1) as f64;
    let chart_height = bar_rows as usize;

    let mut lines: Vec<Line<'static>> =
        Vec::with_capacity(chart_height + usize::from(BAR_CHART_FIXED_ROWS));
    for row_index in 0..chart_height {
        // Rows are emitted top-down; `row_from_bottom` counts up from the axis.
        let row_from_bottom = chart_height - 1 - row_index;
        let label = if row_index == 0 {
            format_token_compact(series.max_total)
        } else {
            String::new()
        };
        let mut spans = vec![
            Span::styled(
                format!(
                    "{label:>width$}",
                    width = geometry.y_axis_width.saturating_sub(1) as usize
                ),
                label_style,
            ),
            Span::styled(axis_vertical().to_string(), axis_style),
            Span::raw(" ".repeat(usize::from(BAR_AXIS_INSET_COLUMNS))),
        ];

        let row_top = ((row_from_bottom + 1) as f64 / chart_height as f64) * max_value;
        let row_bottom = (row_from_bottom as f64 / chart_height as f64) * max_value;
        for (index, day) in series.days.iter().enumerate() {
            let (_, span) = bar_span(index, day_count, plot_width);
            if span == 0 {
                continue;
            }
            let fill = bar_fill_width(span, gutter);
            let (level, slot) = stacked_cell(day, row_bottom, row_top);
            let palette_slot = slot.and_then(|slot| slot_at(slots, slot));
            let glyph = bar_cell_glyph(theme, level, palette_slot);
            spans.push(Span::styled(
                glyph.repeat(fill),
                series_style(theme, palette_slot),
            ));
            if span > fill {
                spans.push(Span::raw(" ".repeat(span - fill)));
            }
        }
        lines.push(Line::from(spans));
    }

    for _ in 0..BAR_AXIS_INSET_ROWS {
        lines.push(Line::from(vec![
            Span::raw(" ".repeat(geometry.y_axis_width.saturating_sub(1) as usize)),
            Span::styled(axis_vertical().to_string(), axis_style),
        ]));
    }

    lines.push(Line::styled(
        format!(
            "{:>width$}{}{}",
            "0",
            axis_corner(),
            axis_horizontal().repeat(axis_body_width),
            width = geometry.y_axis_width.saturating_sub(1) as usize
        ),
        axis_style,
    ));
    lines.push(Line::styled(
        date_label_row(series, geometry.y_axis_width, axis_body_width),
        label_style,
    ));

    frame.render_widget(Paragraph::new(lines), area);
}

fn render_list_rule(frame: &mut Frame<'_>, area: Rect, theme: &super::theme::Theme) {
    let rule = axis_vertical();
    let lines = (0..area.height)
        .map(|_| Line::styled(rule.to_string(), Style::default().fg(theme.dim)))
        .collect::<Vec<_>>();
    frame.render_widget(Paragraph::new(lines), area);
}

/// One list row: the same cost-ranked models the chart stacks, in rank order.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct ModelCostRow {
    pub name: String,
    /// Real tokens (In + Out + CR + CW), summed from the chart's own segments.
    pub tokens: u64,
    pub cost_usd: f64,
    /// Cost share of the window, 0..=100 — or the real-token share when the
    /// window has no cost at all. See [`model_cost_rows`].
    pub share_percent: f64,
    /// Palette slot; `None` is the residual "Other" bucket.
    pub slot: Option<usize>,
    /// In/Out/CR/CW counters behind the row's detail line. Already folded for
    /// the residual bucket, which aggregates every model the chart dropped.
    pub breakdown: UsageModelTokenBreakdown,
}

/// Rank-ordered rows for the "Models by Cost" list. Token totals are summed
/// from the very segments the chart stacks, so the two halves can never
/// disagree.
///
/// The share column is a *cost* share, matching the heading and the ranking.
/// When the whole window costs nothing — no pricing configured for these
/// models, which is the common case for self-hosted and gateway setups — every
/// row would read `0%` and the column would carry no information at all. So it
/// falls back to the real-token share instead of going blank: the heading still
/// describes the ranking (all-zero costs tie, and the tiebreak is tokens), and
/// a proportion the user can act on beats four zeroes.
pub(super) fn model_cost_rows(
    series: &UsageDailyChartSeries,
    slots: &[Option<usize>],
) -> Vec<ModelCostRow> {
    let total_cost = series.total_cost_usd;
    let cost_shares = total_cost.is_finite() && total_cost > 0.0;
    let total_tokens = series.total_tokens as f64;
    series
        .models
        .iter()
        .enumerate()
        .map(|(index, name)| {
            let tokens = series
                .days
                .iter()
                .filter_map(|day| day.segments.get(index).copied())
                .fold(0u64, |acc, value| acc.saturating_add(value));
            let cost_usd = series.model_cost_usd.get(index).copied().unwrap_or(0.0);
            let share_percent = if cost_shares {
                (cost_usd / total_cost) * 100.0
            } else if total_tokens > 0.0 {
                (tokens as f64 / total_tokens) * 100.0
            } else {
                0.0
            };
            ModelCostRow {
                name: name.clone(),
                tokens,
                cost_usd,
                share_percent,
                slot: slot_at(slots, index),
                breakdown: series.model_tokens.get(index).copied().unwrap_or_default(),
            }
        })
        .collect()
}

fn format_share(share: f64) -> String {
    format!("{:.0}%", share.clamp(0.0, 100.0))
}

/// Separator between the four counters of a model's detail line.
#[cfg(test)]
fn detail_separator() -> &'static str {
    super::usage::token_breakdown_separator()
}

/// `In: 2.5M • Out: 6.1M • CR: 975.9M • CW: 109.7M`, indented under the name.
///
/// The labels stay locale-neutral: they abbreviate the Usage page's
/// Input/Output/Cache Read/Cache Write metrics, which the list has no room to
/// spell out and which read the same in every language.
pub(super) fn model_detail_text(breakdown: &UsageModelTokenBreakdown) -> String {
    format!(
        "{}{}",
        " ".repeat(LIST_DETAIL_INDENT),
        super::usage::format_token_breakdown_compact(
            breakdown.input_tokens,
            breakdown.output_tokens,
            breakdown.cache_read_tokens,
            breakdown.cache_creation_tokens,
        )
    )
}

/// Whether every row can carry its detail line.
///
/// Degradation only ever drops the detail lines, never a model's name row: too
/// narrow for the counters, or too short for two rows per model, and the list
/// falls back to the one-line form.
pub(super) fn list_shows_detail(area: Rect, row_count: usize) -> bool {
    let needed = row_count.saturating_mul(2).saturating_add(1);
    area.width >= LIST_DETAIL_MIN_WIDTH && usize::from(area.height) >= needed
}

fn render_model_cost_list(
    frame: &mut Frame<'_>,
    area: Rect,
    theme: &super::theme::Theme,
    series: &UsageDailyChartSeries,
    rows: &[ModelCostRow],
) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let total_cost = format_money(series.total_cost_usd);
    let cost_width = rows
        .iter()
        .map(|row| UnicodeWidthStr::width(format_money(row.cost_usd).as_str()))
        .max()
        .unwrap_or(0)
        .max(UnicodeWidthStr::width(total_cost.as_str()));
    let width = area.width as usize;
    let show_detail = list_shows_detail(area, rows.len());
    let visible_rows = if show_detail {
        rows.len()
    } else {
        rows.len().min(usize::from(area.height.saturating_sub(1)))
    };
    let hidden_rows = rows.len().saturating_sub(visible_rows);

    // ` ● ` + name + share + gap + cost, with the name column absorbing the rest.
    let share_width = LIST_SHARE_WIDTH;
    let fixed = 1 + legend_dot_width() + 1 + share_width + 1 + cost_width;
    let name_width = width.saturating_sub(fixed);

    let mut lines = vec![list_header_line(
        theme,
        width,
        &total_cost,
        cost_width,
        hidden_rows,
    )];
    if name_width > 0 {
        for row in rows.iter().take(visible_rows) {
            let dot = legend_dot(theme, row.slot);
            let name = truncate_label(&row.name, name_width as u16);
            let share = format_share(row.share_percent);
            let cost = format_money(row.cost_usd);
            lines.push(Line::from(vec![
                Span::raw(" "),
                Span::styled(format!("{dot} "), series_style(theme, row.slot)),
                Span::raw(format!(
                    "{name}{}",
                    " ".repeat(name_width.saturating_sub(UnicodeWidthStr::width(name.as_str())))
                )),
                Span::styled(
                    format!("{share:>share_width$} "),
                    Style::default().fg(theme.comment),
                ),
                Span::styled(
                    format!("{cost:>cost_width$}"),
                    Style::default().fg(theme.fg_strong),
                ),
            ]));
            if show_detail {
                lines.push(Line::styled(
                    truncate_label(&model_detail_text(&row.breakdown), area.width),
                    Style::default().fg(theme.comment),
                ));
            }
        }
    }

    frame.render_widget(Paragraph::new(lines), area);
}

fn list_header_line(
    theme: &super::theme::Theme,
    width: usize,
    total_cost: &str,
    cost_width: usize,
    hidden_rows: usize,
) -> Line<'static> {
    let title_budget = width.saturating_sub(cost_width + 2);
    let title = if hidden_rows > 0 {
        format!("{} (+{hidden_rows})", texts::tui_home_chart_list_title())
    } else {
        texts::tui_home_chart_list_title().to_string()
    };
    let title = truncate_label(&title, title_budget as u16);
    let gap = width
        .saturating_sub(1)
        .saturating_sub(UnicodeWidthStr::width(title.as_str()))
        .saturating_sub(cost_width);
    Line::from(vec![
        Span::raw(" "),
        Span::styled(
            title,
            Style::default()
                .fg(theme.comment)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" ".repeat(gap)),
        Span::styled(
            format!("{total_cost:>cost_width$}"),
            Style::default().fg(theme.fg_strong),
        ),
    ])
}

fn axis_vertical() -> &'static str {
    if icons::use_emoji() {
        "│"
    } else {
        "|"
    }
}

fn axis_corner() -> &'static str {
    if icons::use_emoji() {
        "└"
    } else {
        "+"
    }
}

fn axis_horizontal() -> &'static str {
    if icons::use_emoji() {
        "─"
    } else {
        "-"
    }
}

/// Block level (0..=8) and stacked-series slot for one chart cell.
fn stacked_cell(day: &UsageDailyChartDay, row_bottom: f64, row_top: f64) -> (usize, Option<usize>) {
    let total = day.total as f64;
    if total <= row_bottom || day.segments.is_empty() {
        return (0, None);
    }

    // The slot that covers most of this row's value band owns the cell color.
    let mut cursor = 0.0f64;
    let mut best_overlap = 0.0f64;
    let mut best_slot = 0usize;
    for (slot, tokens) in day.segments.iter().enumerate() {
        let start = cursor;
        let end = cursor + *tokens as f64;
        cursor = end;
        let overlap = end.min(row_top) - start.max(row_bottom);
        if overlap > best_overlap {
            best_overlap = overlap;
            best_slot = slot;
        }
    }

    if total >= row_top {
        return (8, Some(best_slot));
    }

    let band = row_top - row_bottom;
    let ratio = if band > 0.0 {
        (total - row_bottom) / band
    } else {
        1.0
    };
    let level = (ratio * 8.0).floor().clamp(1.0, 8.0) as usize;
    (level, Some(best_slot))
}

/// Three date labels under the bars: first, middle, and last day.
///
/// The trailing label is right-aligned to the chart edge so the newest day is
/// always named; labels that would collide with an earlier one are dropped.
pub(super) fn date_label_row(
    series: &UsageDailyChartSeries,
    y_axis_width: u16,
    axis_body_width: usize,
) -> String {
    let total_width = y_axis_width as usize + axis_body_width;
    let mut row = vec![b' '; total_width];
    let day_count = series.days.len();
    if day_count == 0 {
        return String::from_utf8(row).unwrap_or_default();
    }
    let plot_width = bar_plot_width(axis_body_width);

    let mut indices = vec![0usize, day_count / 2, day_count - 1];
    indices.dedup();
    let last_index = day_count - 1;

    let mut next_free = 0usize;
    for index in indices {
        let label = &series.days[index].label;
        // The row buffer is ASCII, so byte and column offsets stay aligned.
        if label.is_empty() || !label.is_ascii() {
            continue;
        }
        let width = label.len();
        let ideal = if index == last_index {
            total_width.saturating_sub(width)
        } else {
            y_axis_width as usize
                + usize::from(BAR_AXIS_INSET_COLUMNS)
                + bar_span(index, day_count, plot_width).0
        };
        if ideal < next_free || ideal + width > total_width {
            continue;
        }
        row[ideal..ideal + width].copy_from_slice(label.as_bytes());
        next_free = ideal + width + 1;
    }

    String::from_utf8(row).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::tui::data::UsageDailyModelBucket;
    use crate::cli::tui::ui::tests::{lock_env, EnvGuard};

    fn day(total: u64, segments: Vec<u64>) -> UsageDailyChartDay {
        UsageDailyChartDay {
            date_key: "2026-07-01".to_string(),
            label: "07/01".to_string(),
            segments,
            total,
        }
    }

    #[test]
    fn geometry_ladder_matches_documented_thresholds() {
        // Heights are the card's inner rows: the border owns two more.
        assert_eq!(
            home_chart_geometry(120, 0, 30, 5, 1_000).mode,
            HomeChartMode::Hidden,
            "an empty card body renders nothing"
        );
        assert_eq!(
            home_chart_geometry(120, 1, 30, 5, 1_000).mode,
            HomeChartMode::ListOnly
        );
        assert_eq!(
            home_chart_geometry(120, 2, 30, 5, 1_000).mode,
            HomeChartMode::ListOnly
        );

        // A wide but short card keeps the list and drops the graph.
        let wide = home_chart_geometry(120, 3, 30, 5, 1_000);
        assert_eq!(wide.mode, HomeChartMode::ListOnly);
        assert_eq!(wide.chart_width, 0);
        assert_eq!(wide.list_width, 120);

        // Once the header plus all five names fit, the graph earns its column.
        let wide_with_list = home_chart_geometry(120, 8, 30, 5, 1_000);
        assert_eq!(wide_with_list.mode, HomeChartMode::Bars);
        assert!(wide_with_list.list_width > 0);
        assert_eq!(wide_with_list.bar_rows, 6);

        // Narrow cards never fall back to a monochrome graph: the list owns
        // every useful row regardless of height.
        let compact = home_chart_geometry(70, 3, 30, 5, 1_000);
        assert_eq!(compact.mode, HomeChartMode::ListOnly);
        assert_eq!(compact.list_width, 70);
        let tall_narrow = home_chart_geometry(70, 20, 30, 5, 1_000);
        assert_eq!(tall_narrow.mode, HomeChartMode::ListOnly);
        assert_eq!(tall_narrow.list_width, 70);

        // Small entity sets can use the list at the same compact height
        // and leave the remaining columns to the full graph.
        let one_model = home_chart_geometry(120, 3, 30, 1, 1_000);
        assert_eq!(one_model.mode, HomeChartMode::Bars);
        assert!(one_model.list_width > 0);
        assert_eq!(one_model.bar_rows, 1);
    }

    #[test]
    fn geometry_splits_off_the_model_list_only_on_wide_cards() {
        // One column below the threshold the list owns everything.
        let narrow = home_chart_geometry(LIST_MIN_CONTENT_WIDTH - 1, 10, 30, 5, 1_000);
        assert_eq!(narrow.mode, HomeChartMode::ListOnly);
        assert_eq!(narrow.list_width, LIST_MIN_CONTENT_WIDTH - 1);
        assert_eq!(narrow.chart_width, 0);

        // At the derived threshold both sides keep their readable minimum.
        let edge = home_chart_geometry(LIST_MIN_CONTENT_WIDTH, 10, 30, 5, 1_000);
        assert_eq!(LIST_MIN_CONTENT_WIDTH, 84);
        assert_eq!(edge.list_width, LIST_MIN_WIDTH);
        assert_eq!(edge.chart_width, MIN_BAR_CHART_WIDTH);

        // 87 columns is what a 120-column terminal leaves as card content.
        let split = home_chart_geometry(87, 10, 30, 5, 1_000);
        assert_eq!(split.list_width, 42);
        assert_eq!(split.chart_width, MIN_BAR_CHART_WIDTH);
        assert!(
            split.list_width < LIST_DETAIL_MIN_WIDTH,
            "a 120-column terminal is too narrow for the detail lines"
        );

        // 127 columns is what a 160-column terminal leaves: the list caps out
        // and is wide enough for the In/Out/CR/CW detail line.
        let wide = home_chart_geometry(127, 10, 30, 5, 1_000);
        assert_eq!(wide.list_width, LIST_MAX_WIDTH);
        assert_eq!(wide.chart_width, 127 - LIST_MAX_WIDTH - LIST_RULE_WIDTH);
        assert!(wide.list_width >= LIST_DETAIL_MIN_WIDTH);
    }

    #[test]
    fn model_list_applies_revised_width_without_starving_the_chart() {
        assert_eq!(LIST_WIDTH_PERCENT, 52);
        assert_eq!((LIST_MIN_WIDTH, LIST_MAX_WIDTH), (39, 59));

        assert_eq!(
            list_width_for_split(LIST_MIN_CONTENT_WIDTH),
            LIST_MIN_WIDTH,
            "the list yields to the chart at the split floor"
        );
        assert_eq!(
            list_width_for_split(127),
            LIST_MAX_WIDTH,
            "roomy cards receive the widened cap"
        );
    }

    #[test]
    fn geometry_never_starves_the_chart_to_feed_the_list() {
        for width in LIST_MIN_CONTENT_WIDTH..400 {
            let geometry = home_chart_geometry(width, 10, 30, 5, 1_000);
            assert!(
                geometry.chart_width >= MIN_BAR_CHART_WIDTH,
                "content width {width} left the chart {} columns",
                geometry.chart_width
            );
            assert_eq!(
                geometry.chart_width + geometry.list_width + LIST_RULE_WIDTH,
                width,
                "the split must tile the content width"
            );
        }
    }

    #[test]
    fn card_padding_costs_two_columns_and_at_most_one_row() {
        assert_eq!(card_content_width(129), 127);
        assert_eq!(card_content_width(1), 0);

        assert_eq!(card_content_height(23), 22);
        assert_eq!(card_content_height(2), 1);
        // A single-row body spends it on content, not on breathing space.
        assert_eq!(card_content_height(1), 1);
        assert_eq!(card_content_height(0), 0);

        let inner = Rect {
            x: 4,
            y: 9,
            width: 20,
            height: 5,
        };
        let content = card_content_area(inner);
        assert_eq!((content.x, content.y), (5, 10));
        assert_eq!((content.width, content.height), (18, 4));
    }

    #[test]
    fn list_detail_lines_are_the_only_thing_degradation_drops() {
        let area = |width, height| Rect {
            x: 0,
            y: 0,
            width,
            height,
        };
        // Header plus two rows per model.
        assert!(list_shows_detail(area(LIST_DETAIL_MIN_WIDTH, 11), 5));
        // One row short: back to one line per model.
        assert!(!list_shows_detail(area(LIST_DETAIL_MIN_WIDTH, 10), 5));
        // Wide enough vertically, one column too narrow.
        assert!(!list_shows_detail(area(LIST_DETAIL_MIN_WIDTH - 1, 40), 5));
    }

    #[test]
    fn short_list_marks_how_many_model_rows_are_hidden() {
        let _lock = lock_env();
        let _no_color = EnvGuard::remove("NO_COLOR");
        let theme = crate::cli::tui::theme::theme_for_mode(
            &AppType::Claude,
            crate::cli::tui::theme::ThemeMode::Dark,
        );
        let rows = (0..5)
            .map(|index| ModelCostRow {
                name: format!("model-{index}"),
                tokens: 100,
                cost_usd: (index + 1) as f64,
                share_percent: 20.0,
                slot: Some(index),
                breakdown: UsageModelTokenBreakdown::default(),
            })
            .collect::<Vec<_>>();
        let series = UsageDailyChartSeries {
            total_cost_usd: 15.0,
            ..UsageDailyChartSeries::default()
        };
        let backend = ratatui::backend::TestBackend::new(50, 3);
        let mut terminal = ratatui::Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| {
                render_model_cost_list(
                    frame,
                    Rect {
                        x: 0,
                        y: 0,
                        width: 50,
                        height: 3,
                    },
                    &theme,
                    &series,
                    &rows,
                )
            })
            .expect("render compact list");

        let buffer = terminal.backend().buffer();
        let line = |y| {
            (0..50)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<Vec<_>>()
                .concat()
        };
        assert!(line(0).contains("(+3)"), "{:?}", line(0));
        assert!(line(1).contains("model-0"), "{:?}", line(1));
        assert!(line(2).contains("model-1"), "{:?}", line(2));
    }

    #[test]
    fn model_detail_text_labels_the_four_counters() {
        let breakdown = UsageModelTokenBreakdown {
            input_tokens: 2_500_000,
            output_tokens: 6_100_000,
            cache_read_tokens: 975_900_000,
            cache_creation_tokens: 109_700_000,
        };
        let text = model_detail_text(&breakdown);
        // The separator follows the icon mode, which is process-global here.
        let separator = detail_separator();
        assert_eq!(
            text,
            format!("    In: 2.5M{separator}Out: 6.1M{separator}CR: 975.9M{separator}CW: 109.7M"),
            "the detail line mirrors the Usage page's abbreviations"
        );

        // The worst realistic case — six characters per counter — still has to
        // fit the widest list column, or the line would truncate for anyone
        // with double-digit millions of input.
        let widest = model_detail_text(&UsageModelTokenBreakdown {
            input_tokens: 12_500_000,
            output_tokens: 16_100_000,
            cache_read_tokens: 975_900_000,
            cache_creation_tokens: 109_700_000,
        });
        let detail_width = UnicodeWidthStr::width(widest.as_str());
        assert_eq!(detail_width, 52, "{widest}");
        assert!(detail_width <= LIST_MAX_WIDTH as usize, "{widest}");
    }

    #[test]
    fn geometry_prioritizes_the_list_when_columns_do_not_fit() {
        // Even when 44 columns could draw 30 tiny bars, the list wins because
        // there is no room to show both views.
        assert_eq!(
            home_chart_geometry(44, 12, 30, 5, 1_000).mode,
            HomeChartMode::ListOnly
        );
        assert_eq!(
            home_chart_geometry(43, 12, 30, 5, 1_000).mode,
            HomeChartMode::ListOnly
        );
        assert_eq!(
            home_chart_geometry(23, 12, 30, 5, 1_000).mode,
            HomeChartMode::Hidden
        );
        // A split wide enough in total still falls back to the list if its
        // chart column cannot host every day.
        assert_eq!(
            home_chart_geometry(82, 12, 200, 5, 1_000).mode,
            HomeChartMode::ListOnly
        );
    }

    #[test]
    fn geometry_hides_when_there_are_no_days() {
        assert_eq!(
            home_chart_geometry(120, 40, 0, 5, 1_000).mode,
            HomeChartMode::Hidden
        );
    }

    #[test]
    fn y_axis_expands_for_a_seven_character_maximum() {
        assert_eq!(format_token_compact(1_494_900_000), "1494.9M");
        assert_eq!(y_axis_width(999_900_000), MIN_Y_AXIS_WIDTH);
        assert_eq!(y_axis_width(1_494_900_000), 8);
    }

    #[test]
    fn large_maximum_keeps_axes_aligned_and_first_bar_inset_visually_balanced() {
        let _lock = lock_env();
        let _no_color = EnvGuard::remove("NO_COLOR");
        let theme = crate::cli::tui::theme::theme_for_mode(
            &AppType::Claude,
            crate::cli::tui::theme::ThemeMode::Dark,
        );
        let max_total = 1_494_900_000;
        let days = (1..=30)
            .map(|index| UsageDailyChartDay {
                date_key: format!("2026-07-{index:02}"),
                label: format!("07/{index:02}"),
                segments: vec![max_total],
                total: max_total,
            })
            .collect::<Vec<_>>();
        let series = UsageDailyChartSeries {
            days,
            max_total,
            total_tokens: max_total,
            ..UsageDailyChartSeries::default()
        };
        let geometry = home_chart_geometry(120, 8, 30, 1, max_total);
        assert_eq!(geometry.mode, HomeChartMode::Bars);
        assert_eq!(geometry.y_axis_width, 8);

        let backend = ratatui::backend::TestBackend::new(
            geometry.chart_width,
            geometry.bar_rows + BAR_CHART_FIXED_ROWS,
        );
        let mut terminal = ratatui::Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| {
                render_bars(
                    frame,
                    Rect {
                        x: 0,
                        y: 0,
                        width: geometry.chart_width,
                        height: geometry.bar_rows + BAR_CHART_FIXED_ROWS,
                    },
                    &theme,
                    &series,
                    &[Some(0)],
                    &geometry,
                )
            })
            .expect("render bars");

        let buffer = terminal.backend().buffer();
        let top_axis = (0..geometry.chart_width)
            .find(|x| buffer[(*x, 0)].symbol() == axis_vertical())
            .expect("top-row axis");
        let x_axis_row = geometry.bar_rows + BAR_AXIS_INSET_ROWS;
        let zero_axis = (0..geometry.chart_width)
            .find(|x| buffer[(*x, x_axis_row)].symbol() == axis_corner())
            .expect("zero-row axis");
        assert_eq!(top_axis, 7);
        assert_eq!(zero_axis, top_axis, "every axis row starts in one column");

        let bottom_bar_row = geometry.bar_rows - 1;
        let first_bar_column = top_axis + 1 + BAR_AXIS_INSET_COLUMNS;
        assert_eq!(
            buffer[(first_bar_column, bottom_bar_row)].symbol(),
            bar_cell_glyph(&theme, 8, Some(0))
        );
        assert_eq!(
            first_bar_column - top_axis - 1,
            BAR_AXIS_INSET_COLUMNS,
            "horizontal inset from y-axis to first bar"
        );
        assert_eq!(
            x_axis_row - bottom_bar_row - 1,
            BAR_AXIS_INSET_ROWS,
            "vertical inset from bottom bar to x-axis"
        );
        assert_eq!(BAR_AXIS_INSET_COLUMNS, 1);
        assert_eq!(BAR_AXIS_INSET_ROWS, 0);
        for x in (top_axis + 1)..first_bar_column {
            assert_eq!(
                buffer[(x, bottom_bar_row)].symbol(),
                " ",
                "the horizontal inset stays blank"
            );
        }
        assert_eq!(
            buffer[(first_bar_column, bottom_bar_row + 1)].symbol(),
            axis_horizontal(),
            "the centered x-axis follows the bars without a whole blank row"
        );
    }

    #[test]
    fn bar_spans_tile_the_whole_chart_body() {
        for (body, days) in [(44usize, 30usize), (84, 30), (37, 30), (100, 7), (31, 30)] {
            let mut cursor = 0usize;
            for index in 0..days {
                let (start, width) = bar_span(index, days, body);
                assert_eq!(start, cursor, "columns must be contiguous");
                cursor += width;
            }
            assert_eq!(cursor, body, "bars must reach the right edge");
        }
    }

    #[test]
    fn wide_columns_keep_a_one_column_gutter() {
        // 30 days over a 44-column body: one to two columns each, contiguous.
        assert!(!use_bar_gutter(30, 44));
        // Seven days over the same body: room to breathe.
        assert!(use_bar_gutter(7, 44));

        assert_eq!(bar_fill_width(2, false), 2);
        assert_eq!(bar_fill_width(6, false), 6);
        assert_eq!(bar_fill_width(6, true), 5);
        assert_eq!(bar_fill_width(1, true), 1, "a lone column is never blanked");
    }

    #[test]
    fn palette_slots_follow_the_model_not_its_rank() {
        let ranked = UsageDailyChartSeries {
            models: vec![
                "sonnet".to_string(),
                "opus".to_string(),
                "haiku".to_string(),
                "Other".to_string(),
            ],
            other_index: Some(3),
            ..UsageDailyChartSeries::default()
        };
        let shuffled = UsageDailyChartSeries {
            models: vec![
                "haiku".to_string(),
                "sonnet".to_string(),
                "opus".to_string(),
                "Other".to_string(),
            ],
            other_index: Some(3),
            ..UsageDailyChartSeries::default()
        };

        let ranked_slots = entity_palette_slots(&ranked);
        let shuffled_slots = entity_palette_slots(&shuffled);

        // Alphabetical order hands out the slots: haiku=0, opus=1, sonnet=2.
        assert_eq!(ranked_slots, vec![Some(2), Some(1), Some(0), None]);
        assert_eq!(shuffled_slots, vec![Some(0), Some(2), Some(1), None]);
        for model in ["haiku", "opus", "sonnet"] {
            let before = ranked.models.iter().position(|m| m == model).unwrap();
            let after = shuffled.models.iter().position(|m| m == model).unwrap();
            assert_eq!(
                ranked_slots[before], shuffled_slots[after],
                "{model} must keep its color when the ranking moves"
            );
        }
    }

    #[test]
    fn series_palette_stays_clear_of_the_status_colors() {
        let _lock = lock_env();
        let _no_color = EnvGuard::remove("NO_COLOR");
        let _color_mode = EnvGuard::set("CC_SWITCH_COLOR_MODE", "truecolor");
        let theme = crate::cli::tui::theme::theme_for_mode(
            &AppType::Claude,
            crate::cli::tui::theme::ThemeMode::Dark,
        );
        let status = [theme.ok, theme.warn, theme.err];
        let mut seen = Vec::new();
        for rgb in SERIES_PALETTE {
            let color = theme.shade(rgb);
            assert!(
                !status.contains(&color),
                "{rgb:?} collides with a status color"
            );
            assert!(!seen.contains(&color), "{rgb:?} duplicates another series");
            seen.push(color);
        }
    }

    /// Every slot has to be tellable from every other one — that is the whole
    /// point of the fallback — and from the residual bucket.
    #[test]
    fn series_glyphs_stay_distinct_without_color() {
        let mut mono = crate::cli::tui::theme::theme_for_mode(
            &AppType::Claude,
            crate::cli::tui::theme::ThemeMode::Dark,
        );
        mono.no_color = true;

        let glyphs = (0..HOME_CHART_MAX_MODELS)
            .map(|slot| legend_dot(&mono, Some(slot)))
            .chain(std::iter::once(legend_dot(&mono, None)))
            .collect::<Vec<_>>();

        assert_eq!(glyphs.len(), HOME_CHART_MAX_MODELS + 1);
        for (index, glyph) in glyphs.iter().enumerate() {
            assert!(glyph.is_ascii(), "{glyph:?} must survive an ascii terminal");
            assert_eq!(UnicodeWidthStr::width(*glyph), 1, "{glyph:?}");
            assert!(
                !glyphs[..index].contains(glyph),
                "slot {index} reuses {glyph:?}: {glyphs:?}"
            );
        }
        assert!(legend_dot_width() >= 1);
    }

    #[test]
    fn no_color_bar_cells_match_the_model_list_glyphs() {
        let mut mono = crate::cli::tui::theme::theme_for_mode(
            &AppType::Claude,
            crate::cli::tui::theme::ThemeMode::Dark,
        );
        mono.no_color = true;

        assert_eq!(bar_cell_glyph(&mono, 0, Some(0)), " ");
        for slot in 0..HOME_CHART_MAX_MODELS {
            assert_eq!(
                bar_cell_glyph(&mono, 8, Some(slot)),
                legend_dot(&mono, Some(slot)),
                "slot {slot} must use one glyph in the chart and list"
            );
        }
        assert_eq!(
            bar_cell_glyph(&mono, 3, None),
            legend_dot(&mono, None),
            "the residual bucket must keep its own pattern"
        );

        mono.no_color = false;
        assert_eq!(
            bar_cell_glyph(&mono, 3, Some(0)),
            blocks()[3],
            "color mode keeps the fractional-height ramp"
        );
    }

    #[test]
    fn series_glyphs_stay_round_when_color_can_tell_them_apart() {
        let _lock = lock_env();
        let _no_color = EnvGuard::remove("NO_COLOR");
        let _color_mode = EnvGuard::set("CC_SWITCH_COLOR_MODE", "truecolor");
        let theme = crate::cli::tui::theme::theme_for_mode(
            &AppType::Claude,
            crate::cli::tui::theme::ThemeMode::Dark,
        );
        // The unicode branch only applies in emoji icon mode, which is
        // process-global here; assert against whichever mode is active.
        let expected = if icons::use_emoji() { "●" } else { "*" };
        assert_eq!(legend_dot(&theme, Some(0)), expected);
    }

    fn chart_bucket(model: &str, cost: f64, tokens: u64) -> UsageDailyModelBucket {
        UsageDailyModelBucket {
            date_key: "2026-07-01".to_string(),
            model: model.to_string(),
            total_tokens: tokens,
            total_cost_usd: cost,
            input_tokens: tokens,
            ..UsageDailyModelBucket::default()
        }
    }

    #[test]
    fn projection_is_rebuilt_only_when_the_snapshot_generation_moves() {
        reset_projection_cache();

        let usage = UsageSnapshot {
            daily_models: vec![chart_bucket("opus", 1.0, 100)],
            ..UsageSnapshot::default()
        };

        let first = home_chart_projection(&usage);
        assert_eq!(projection_build_count(), 1);

        // Same snapshot, next frame: no rebuild, and the very same allocation.
        let second = home_chart_projection(&usage);
        assert_eq!(projection_build_count(), 1, "a frame must not rebuild");
        assert!(Rc::ptr_eq(&first, &second));

        // A clone is the same data, so it must hit the same entry.
        let cloned = usage.clone();
        let third = home_chart_projection(&cloned);
        assert_eq!(projection_build_count(), 1, "a clone is the same snapshot");
        assert!(Rc::ptr_eq(&first, &third));

        // A fresh load carries a fresh generation.
        let reloaded = UsageSnapshot {
            daily_models: vec![chart_bucket("opus", 1.0, 200)],
            ..UsageSnapshot::default()
        };
        let fourth = home_chart_projection(&reloaded);
        assert_eq!(projection_build_count(), 2);
        assert!(!Rc::ptr_eq(&first, &fourth));
        assert_eq!(fourth.series.total_tokens, 200);

        reset_projection_cache();
    }

    #[test]
    fn projection_carries_the_series_slots_and_rows_from_one_entity_set() {
        reset_projection_cache();

        let usage = UsageSnapshot {
            daily_models: vec![
                chart_bucket("expensive", 9.0, 10),
                chart_bucket("cheap", 0.1, 10_000),
            ],
            ..UsageSnapshot::default()
        };
        let projection = home_chart_projection(&usage);

        assert_eq!(projection.series.models, vec!["expensive", "cheap"]);
        assert_eq!(projection.slots.len(), projection.series.models.len());
        assert_eq!(
            projection
                .rows
                .iter()
                .map(|row| row.name.as_str())
                .collect::<Vec<_>>(),
            projection
                .series
                .models
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            "the list draws the same entities, in the same order, as the bars"
        );
        assert_eq!(
            projection
                .rows
                .iter()
                .map(|row| row.slot)
                .collect::<Vec<_>>(),
            projection.slots,
            "one color mapping for the legend and the list"
        );

        reset_projection_cache();
    }

    fn breakdown(input: u64, output: u64, read: u64, write: u64) -> UsageModelTokenBreakdown {
        UsageModelTokenBreakdown {
            input_tokens: input,
            output_tokens: output,
            cache_read_tokens: read,
            cache_creation_tokens: write,
        }
    }

    #[test]
    fn model_cost_rows_share_cost_against_the_window_total() {
        let series = UsageDailyChartSeries {
            models: vec!["opus".to_string(), "haiku".to_string(), "Other".to_string()],
            other_index: Some(2),
            days: vec![day(100, vec![50, 30, 20]), day(100, vec![50, 10, 40])],
            max_total: 100,
            total_tokens: 200,
            total_cost_usd: 6.0,
            model_cost_usd: vec![3.0, 2.0, 1.0],
            model_tokens: vec![
                breakdown(60, 40, 900, 100),
                breakdown(30, 10, 5, 1),
                breakdown(50, 10, 7, 3),
            ],
        };
        let slots = entity_palette_slots(&series);
        let rows = model_cost_rows(&series, &slots);

        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].name, "opus");
        assert_eq!(rows[0].tokens, 100);
        // Cost share, matching the "Models by Cost" heading — the token share
        // here would be 50/40/60 out of 200, i.e. a different column.
        assert!((rows[0].share_percent - 50.0).abs() < 1e-9);
        assert!((rows[0].cost_usd - 3.0).abs() < f64::EPSILON);
        assert_eq!(rows[0].breakdown, breakdown(60, 40, 900, 100));
        assert_eq!(rows[1].tokens, 40);
        assert!((rows[1].share_percent - (2.0 / 6.0) * 100.0).abs() < 1e-9);
        assert_eq!(rows[2].tokens, 60);
        assert!((rows[2].share_percent - (1.0 / 6.0) * 100.0).abs() < 1e-9);
        assert_eq!(rows[2].slot, None, "the residual bucket is never colored");
        assert_eq!(
            rows[2].breakdown,
            breakdown(50, 10, 7, 3),
            "the residual bucket carries its own folded counters"
        );
    }

    /// With no pricing configured the cost column is all zeroes; the share
    /// column falls back to real tokens instead of printing four `0%`.
    #[test]
    fn model_cost_rows_fall_back_to_token_share_without_pricing() {
        let series = UsageDailyChartSeries {
            models: vec!["opus".to_string(), "haiku".to_string()],
            other_index: None,
            days: vec![day(100, vec![75, 25])],
            max_total: 100,
            total_tokens: 100,
            total_cost_usd: 0.0,
            model_cost_usd: vec![0.0, 0.0],
            model_tokens: vec![breakdown(25, 50, 0, 0), breakdown(5, 20, 0, 0)],
        };
        let rows = model_cost_rows(&series, &entity_palette_slots(&series));

        assert!((rows[0].share_percent - 75.0).abs() < 1e-9, "{rows:?}");
        assert!((rows[1].share_percent - 25.0).abs() < 1e-9, "{rows:?}");
    }

    /// End to end from raw cells: the counters the list prints are the ones the
    /// series folded, "Other" included.
    #[test]
    fn model_cost_rows_report_the_folded_detail_counters() {
        let cell = |model: &str, cost: f64, factor: u64| UsageDailyModelBucket {
            date_key: "2026-07-01".to_string(),
            model: model.to_string(),
            is_other: false,
            total_tokens: factor * 3,
            total_cost_usd: cost,
            input_tokens: factor,
            output_tokens: factor * 2,
            cache_read_tokens: factor * 3,
            cache_creation_tokens: factor * 4,
        };
        // Ranked by cost, so m1 leads despite carrying the fewest tokens.
        let buckets = vec![
            cell("m1", 5.0, 10),
            cell("m2", 4.0, 20),
            cell("m3", 3.0, 30),
            cell("m4", 2.0, 40),
            // Folded into "Other".
            cell("m5", 1.0, 50),
            cell("m6", 0.5, 60),
        ];

        let series = crate::cli::tui::data::build_usage_daily_chart_series(
            &[],
            &buckets,
            HOME_CHART_MAX_MODELS,
            "Other",
        );
        let slots = entity_palette_slots(&series);
        let rows = model_cost_rows(&series, &slots);

        assert_eq!(rows.len(), 5);
        assert_eq!(rows[0].name, "m1");
        assert_eq!(rows[0].breakdown, breakdown(10, 20, 30, 40));
        assert_eq!(
            rows[0].tokens,
            10 + 20 + 30 + 40,
            "the list totals real tokens, cache included"
        );
        let other = rows.last().expect("the residual row");
        assert_eq!(other.name, "Other");
        assert_eq!(
            other.breakdown,
            breakdown(50 + 60, 100 + 120, 150 + 180, 200 + 240)
        );
    }

    #[test]
    fn model_cost_rows_saturate_instead_of_wrapping() {
        let cell = |model: &str| UsageDailyModelBucket {
            date_key: "2026-07-01".to_string(),
            model: model.to_string(),
            is_other: false,
            total_tokens: 1,
            total_cost_usd: 0.0,
            input_tokens: u64::MAX,
            output_tokens: u64::MAX,
            cache_read_tokens: u64::MAX,
            cache_creation_tokens: u64::MAX,
        };
        // Six models: two of them fold into the same "Other" bucket, so the
        // residual row is where the overflow would show up first.
        let buckets = ["m1", "m2", "m3", "m4", "m5", "m6"]
            .into_iter()
            .map(cell)
            .collect::<Vec<_>>();

        let series = crate::cli::tui::data::build_usage_daily_chart_series(
            &[],
            &buckets,
            HOME_CHART_MAX_MODELS,
            "Other",
        );
        let rows = model_cost_rows(&series, &entity_palette_slots(&series));
        let other = rows.last().expect("the residual row");
        assert_eq!(
            other.breakdown,
            breakdown(u64::MAX, u64::MAX, u64::MAX, u64::MAX)
        );
    }

    #[test]
    fn stacked_cell_picks_the_dominant_series_in_the_band() {
        let day = day(100, vec![80, 20]);
        // Band 0..50 lies entirely in the first segment.
        assert_eq!(stacked_cell(&day, 0.0, 50.0), (8, Some(0)));
        // Band 80..100 is all second segment.
        assert_eq!(stacked_cell(&day, 80.0, 100.0), (8, Some(1)));
        // Above the total: empty cell.
        assert_eq!(stacked_cell(&day, 100.0, 150.0), (0, None));
    }

    #[test]
    fn stacked_cell_uses_partial_blocks_for_the_top_row() {
        let day = day(25, vec![25]);
        let (level, slot) = stacked_cell(&day, 0.0, 100.0);
        assert_eq!(slot, Some(0));
        assert_eq!(level, 2, "25% of the band is two eighths");
    }

    #[test]
    fn saturated_days_do_not_overflow_the_chart_series() {
        let buckets = (0..3)
            .map(|idx| UsageDailyModelBucket {
                date_key: format!("2026-07-0{}", idx + 1),
                model: "corrupt".to_string(),
                total_tokens: u64::MAX,
                input_tokens: u64::MAX,
                output_tokens: u64::MAX,
                cache_read_tokens: u64::MAX,
                cache_creation_tokens: u64::MAX,
                ..UsageDailyModelBucket::default()
            })
            .collect::<Vec<_>>();
        let series = crate::cli::tui::data::build_usage_daily_chart_series(
            &[],
            &buckets,
            HOME_CHART_MAX_MODELS,
            "Other",
        );
        assert_eq!(series.total_tokens, u64::MAX);
        assert_eq!(series.max_total, u64::MAX);
        // Every cell still resolves to a valid block index.
        for day in &series.days {
            let (level, _) = stacked_cell(day, 0.0, u64::MAX as f64);
            assert!(level <= 8);
        }
    }

    #[test]
    fn date_labels_cover_the_first_middle_and_newest_day() {
        let days = (1..=30)
            .map(|day| UsageDailyChartDay {
                date_key: format!("2026-07-{day:02}"),
                label: format!("07/{day:02}"),
                segments: Vec::new(),
                total: 0,
            })
            .collect::<Vec<_>>();
        let series = UsageDailyChartSeries {
            days,
            ..UsageDailyChartSeries::default()
        };

        let row = date_label_row(&series, MIN_Y_AXIS_WIDTH, 60);

        assert_eq!(row.len(), MIN_Y_AXIS_WIDTH as usize + 60);
        assert!(row.starts_with("        07/01"), "{row:?}");
        assert!(row.contains("07/16"), "{row:?}");
        assert!(row.ends_with("07/30"), "{row:?}");
    }

    #[test]
    fn date_labels_drop_collisions_on_narrow_charts() {
        let days = (1..=3)
            .map(|day| UsageDailyChartDay {
                date_key: format!("2026-07-0{day}"),
                label: format!("07/0{day}"),
                segments: Vec::new(),
                total: 0,
            })
            .collect::<Vec<_>>();
        let series = UsageDailyChartSeries {
            days,
            ..UsageDailyChartSeries::default()
        };

        // Three one-column days cannot host three five-column labels.
        let row = date_label_row(&series, MIN_Y_AXIS_WIDTH, 3);
        assert_eq!(row.len(), MIN_Y_AXIS_WIDTH as usize + 3);
        assert_eq!(row.matches("07/").count(), 1, "{row:?}");
    }

    #[test]
    fn relative_since_buckets_by_unit() {
        let _lang = crate::cli::i18n::use_test_language(crate::cli::i18n::Language::English);
        let now = 1_800_000_000;
        assert_eq!(format_relative_since(now, now), "just now");
        assert_eq!(format_relative_since(now, now + 30), "just now");
        assert_eq!(format_relative_since(now, now - 90), "1m ago");
        assert_eq!(format_relative_since(now, now - 2 * 3600), "2h ago");
        assert_eq!(format_relative_since(now, now - 3 * 86_400), "3d ago");
    }
}
