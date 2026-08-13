use ratatui::symbols;

pub(super) const DOTS: symbols::bar::Set = symbols::bar::Set {
    empty: " ",
    one_eighth: "⡀",
    one_quarter: "⣀",
    three_eighths: "⣄",
    half: "⣤",
    five_eighths: "⣦",
    three_quarters: "⣶",
    seven_eighths: "⣷",
    full: "⣿",
};

pub(super) const REV_DOTS: symbols::bar::Set = symbols::bar::Set {
    empty: " ",
    one_eighth: "⠁",
    one_quarter: "⠉",
    three_eighths: "⠋",
    half: "⠛",
    five_eighths: "⠟",
    three_quarters: "⠿",
    seven_eighths: "⡿",
    full: "⣿",
};

pub(super) fn proxy_wave_lines(
    width: u16,
    height: u16,
    current_app_routed: bool,
    samples: &[u64],
    bar_set: &symbols::bar::Set,
    reversed: bool,
) -> Vec<String> {
    if width == 0 || height == 0 {
        return Vec::new();
    }

    let width = width as usize;
    let height = height as usize;
    let recent = recent_samples(width, current_app_routed, samples);
    let mut scaled = scale_samples(height as u16, &recent, current_app_routed);
    let mut rows = vec![String::with_capacity(width * 3); height];

    for j in (0..height).rev() {
        let row_index = if reversed { height - j - 1 } else { j };
        for value in &mut scaled {
            rows[row_index].push_str(bar_symbol(*value, bar_set));
            if *value > 8 {
                *value -= 8;
            } else {
                *value = 0;
            }
        }
    }

    rows
}

pub(super) fn recent_samples(width: usize, current_app_routed: bool, samples: &[u64]) -> Vec<u64> {
    if !current_app_routed {
        return vec![0; width];
    }

    let recent = if samples.len() > width {
        &samples[samples.len() - width..]
    } else {
        samples
    };

    let mut out = vec![0; width.saturating_sub(recent.len())];
    out.extend_from_slice(recent);
    out
}

pub(super) fn scale_samples(height: u16, samples: &[u64], show_idle_baseline: bool) -> Vec<u64> {
    let baseline = if show_idle_baseline { 1 } else { 0 };
    let max = samples.iter().copied().max().unwrap_or(0);
    if max == 0 {
        return vec![baseline; samples.len()];
    }

    samples
        .iter()
        .map(|value| {
            if *value == 0 {
                baseline
            } else {
                (value * u64::from(height) * 8 / max).max(baseline + 1)
            }
        })
        .collect()
}

fn bar_symbol<'a>(level: u64, bar_set: &'a symbols::bar::Set) -> &'a str {
    match level {
        0 => bar_set.empty,
        1 => bar_set.one_eighth,
        2 => bar_set.one_quarter,
        3 => bar_set.three_eighths,
        4 => bar_set.half,
        5 => bar_set.five_eighths,
        6 => bar_set.three_quarters,
        7 => bar_set.seven_eighths,
        _ => bar_set.full,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proxy_wave_lines_left_pads_recent_samples() {
        let rows = proxy_wave_lines(8, 1, true, &[0, 1, 4, 8], &DOTS, false);

        assert_eq!(rows, vec!["⡀⡀⡀⡀⡀⣀⣤⣿".to_string()]);
    }

    #[test]
    fn proxy_wave_lines_reverses_rows_for_lower_half() {
        let rows = proxy_wave_lines(4, 2, true, &[0, 1, 4, 8], &REV_DOTS, true);

        assert_eq!(rows.len(), 2);
        assert!(rows[0].contains('⠁') || rows[0].contains('⠉'));
        assert!(rows[0].contains('⣿'));
        assert!(rows[1].contains('⣿'));
    }

    #[test]
    fn proxy_wave_lines_keep_baseline_when_burst_appears() {
        let upper = proxy_wave_lines(8, 1, true, &[0, 1, 4, 8], &DOTS, false);
        let lower = proxy_wave_lines(8, 1, true, &[0, 1, 4, 8], &REV_DOTS, true);

        assert!(upper[0].contains('⡀'), "{:?}", upper);
        assert!(lower[0].contains('⠁'), "{:?}", lower);
        assert!(upper[0].contains('⣿'), "{:?}", upper);
    }
}
