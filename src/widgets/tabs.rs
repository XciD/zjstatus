use std::{cmp, collections::BTreeMap};

use zellij_tile::{
    prelude::{InputMode, ModeInfo, PaneInfo, PaneManifest, TabInfo},
    shim::switch_tab_to,
};

use crate::{config::ZellijState, render::FormattedPart};

use super::widget::Widget;

const SPINNER_FRAMES: &[&str] = &["✦", "✶", "✽", "✶"];

#[derive(Clone, Copy)]
struct TabRenderLayout {
    window_start: usize,
    window_end: usize,
    name_width_limit: Option<usize>,
}

pub struct TabsWidget {
    active_tab_format: Vec<FormattedPart>,
    active_tab_fullscreen_format: Vec<FormattedPart>,
    active_tab_sync_format: Vec<FormattedPart>,
    normal_tab_format: Vec<FormattedPart>,
    normal_tab_fullscreen_format: Vec<FormattedPart>,
    normal_tab_sync_format: Vec<FormattedPart>,
    rename_tab_format: Vec<FormattedPart>,
    separator: Option<FormattedPart>,
    fullscreen_indicator: Option<String>,
    floating_indicator: Option<String>,
    sync_indicator: Option<String>,
    tab_display_count: Option<usize>,
    tab_truncate_start_format: Vec<FormattedPart>,
    tab_truncate_end_format: Vec<FormattedPart>,
}

impl TabsWidget {
    pub fn new(config: &BTreeMap<String, String>) -> Self {
        let mut normal_tab_format: Vec<FormattedPart> = Vec::new();
        if let Some(form) = config.get("tab_normal") {
            normal_tab_format = FormattedPart::multiple_from_format_string(form, config);
        }

        let normal_tab_fullscreen_format = match config.get("tab_normal_fullscreen") {
            Some(form) => FormattedPart::multiple_from_format_string(form, config),
            None => normal_tab_format.clone(),
        };

        let normal_tab_sync_format = match config.get("tab_normal_sync") {
            Some(form) => FormattedPart::multiple_from_format_string(form, config),
            None => normal_tab_format.clone(),
        };

        let mut active_tab_format = normal_tab_format.clone();
        if let Some(form) = config.get("tab_active") {
            active_tab_format = FormattedPart::multiple_from_format_string(form, config);
        }

        let active_tab_fullscreen_format = match config.get("tab_active_fullscreen") {
            Some(form) => FormattedPart::multiple_from_format_string(form, config),
            None => active_tab_format.clone(),
        };

        let active_tab_sync_format = match config.get("tab_active_sync") {
            Some(form) => FormattedPart::multiple_from_format_string(form, config),
            None => active_tab_format.clone(),
        };

        let rename_tab_format = match config.get("tab_rename") {
            Some(form) => FormattedPart::multiple_from_format_string(form, config),
            None => active_tab_format.clone(),
        };

        let tab_display_count = match config.get("tab_display_count") {
            Some(count) => count.parse::<usize>().ok(),
            None => None,
        };

        let tab_truncate_start_format = config
            .get("tab_truncate_start_format")
            .map(|form| FormattedPart::multiple_from_format_string(form, config))
            .unwrap_or_default();

        let tab_truncate_end_format = config
            .get("tab_truncate_end_format")
            .map(|form| FormattedPart::multiple_from_format_string(form, config))
            .unwrap_or_default();

        let separator = config
            .get("tab_separator")
            .map(|s| FormattedPart::from_format_string(s, config))
            .or_else(|| Some(FormattedPart::from_format_string(" | ", config)));

        Self {
            normal_tab_format,
            normal_tab_fullscreen_format,
            normal_tab_sync_format,
            active_tab_format,
            active_tab_fullscreen_format,
            active_tab_sync_format,
            rename_tab_format,
            separator,
            floating_indicator: config.get("tab_floating_indicator").cloned(),
            sync_indicator: config.get("tab_sync_indicator").cloned(),
            fullscreen_indicator: config.get("tab_fullscreen_indicator").cloned(),
            tab_display_count,
            tab_truncate_start_format,
            tab_truncate_end_format,
        }
    }
}

impl Widget for TabsWidget {
    fn process(&self, _name: &str, state: &ZellijState) -> String {
        let mut output = "".to_owned();
        let mut counter = 0;

        let layout = self.tab_render_layout(state);
        let truncated_start = layout.window_start;
        let truncated_end = state.tabs.len().saturating_sub(layout.window_end);
        let tabs = &state.tabs[layout.window_start..layout.window_end];

        if truncated_start > 0 {
            output.push_str(&self.render_truncate_start(truncated_start));
        }

        for tab in tabs {
            let display_name = self.resolve_tab_name(
                tab,
                &state.panes,
                &state.tab_name_overrides,
                &state.tab_name_fallbacks,
                state.spinner_idx,
            );
            let content = self.render_tab(
                tab,
                &state.panes,
                &state.mode,
                &display_name,
                layout.name_width_limit,
            );
            counter += 1;

            output = format!("{}{}", output, content);

            if counter < tabs.len()
                && let Some(sep) = &self.separator
            {
                output = format!("{}{}", output, sep.format_string(&sep.content));
            }
        }

        if truncated_end > 0 {
            output.push_str(&self.render_truncate_end(truncated_end));
        }

        output
    }

    fn process_click(&self, _name: &str, state: &ZellijState, pos: usize) {
        let mut offset = 0;
        let mut counter = 0;

        let layout = self.tab_render_layout(state);
        let truncated_start = layout.window_start;
        let truncated_end = state.tabs.len().saturating_sub(layout.window_end);
        let tabs = &state.tabs[layout.window_start..layout.window_end];

        let active_pos = match state.tabs.iter().find(|t| t.active) {
            Some(tab) => tab.position + 1,
            None => return,
        };

        if truncated_start > 0 {
            let truncated = self.render_truncate_start(truncated_start);
            offset += console::measure_text_width(&truncated);

            if pos <= offset {
                switch_tab_to(active_pos.saturating_sub(1) as u32);
            }
        }

        for tab in tabs {
            counter += 1;

            let display_name = self.resolve_tab_name(
                tab,
                &state.panes,
                &state.tab_name_overrides,
                &state.tab_name_fallbacks,
                state.spinner_idx,
            );
            let mut rendered_content = self.render_tab(
                tab,
                &state.panes,
                &state.mode,
                &display_name,
                layout.name_width_limit,
            );

            if counter < tabs.len()
                && let Some(sep) = &self.separator
            {
                rendered_content =
                    format!("{}{}", rendered_content, sep.format_string(&sep.content));
            }

            let content_len = console::measure_text_width(&rendered_content);

            if pos >= offset && pos < offset + content_len {
                switch_tab_to(tab.position as u32 + 1);

                break;
            }

            offset += content_len;
        }

        if truncated_end > 0 {
            let truncated = self.render_truncate_end(truncated_end);
            offset += console::measure_text_width(&truncated);

            if pos <= offset {
                switch_tab_to(cmp::min(active_pos + 1, state.tabs.len()) as u32);
            }
        }
    }
}

impl TabsWidget {
    fn tab_render_layout(&self, state: &ZellijState) -> TabRenderLayout {
        if state.tabs.is_empty() {
            return TabRenderLayout {
                window_start: 0,
                window_end: 0,
                name_width_limit: None,
            };
        }

        let (base_start, base_end) = get_tab_window_indices(&state.tabs, self.tab_display_count);
        if state.cols == 0 {
            return TabRenderLayout {
                window_start: base_start,
                window_end: base_end,
                name_width_limit: None,
            };
        }

        let separator = self.separator_string();

        if let Some(name_width_limit) =
            self.resolve_name_width_limit(state, base_start, base_end, &separator)
        {
            return TabRenderLayout {
                window_start: base_start,
                window_end: base_end,
                name_width_limit,
            };
        }

        let active_index = state.tabs.iter().position(|t| t.active).unwrap_or(0);
        let mut start = base_start;
        let mut end = base_end;

        loop {
            if let Some(name_width_limit) =
                self.resolve_name_width_limit(state, start, end, &separator)
            {
                return TabRenderLayout {
                    window_start: start,
                    window_end: end,
                    name_width_limit,
                };
            }

            if end - start <= 1 {
                return TabRenderLayout {
                    window_start: start,
                    window_end: end,
                    name_width_limit: Some(1),
                };
            }

            let left_distance = active_index.saturating_sub(start);
            let right_distance = end.saturating_sub(active_index + 1);

            if left_distance >= right_distance && start < active_index {
                start += 1;
                continue;
            }

            if right_distance > 0 {
                end = end.saturating_sub(1);
                continue;
            }

            break;
        }

        TabRenderLayout {
            window_start: start,
            window_end: end,
            name_width_limit: Some(1),
        }
    }

    fn window_render_width(
        &self,
        state: &ZellijState,
        start: usize,
        end: usize,
        separator: &str,
        name_width_limit: Option<usize>,
    ) -> usize {
        let mut width = 0;

        if start > 0 {
            width += console::measure_text_width(&self.render_truncate_start(start));
        }

        for i in start..end {
            let tab = &state.tabs[i];
            let display_name = self.resolve_tab_name(
                tab,
                &state.panes,
                &state.tab_name_overrides,
                &state.tab_name_fallbacks,
                state.spinner_idx,
            );
            width += console::measure_text_width(&self.render_tab(
                tab,
                &state.panes,
                &state.mode,
                &display_name,
                name_width_limit,
            ));
            if i + 1 < end && !separator.is_empty() {
                width += console::measure_text_width(separator);
            }
        }

        let truncated_end = state.tabs.len().saturating_sub(end);
        if truncated_end > 0 {
            width += console::measure_text_width(&self.render_truncate_end(truncated_end));
        }

        width
    }

    fn resolve_name_width_limit(
        &self,
        state: &ZellijState,
        start: usize,
        end: usize,
        separator: &str,
    ) -> Option<Option<usize>> {
        if self.window_render_width(state, start, end, separator, None) <= state.cols {
            return Some(None);
        }

        if self.window_render_width(state, start, end, separator, Some(1)) > state.cols {
            return None;
        }

        let mut low = 1usize;
        let mut high = self.max_tab_name_width(state, start, end);
        let mut best = 1usize;

        while low <= high {
            let mid = low + (high - low) / 2;

            if self.window_render_width(state, start, end, separator, Some(mid)) <= state.cols {
                best = mid;
                low = mid + 1;
            } else {
                high = mid.saturating_sub(1);
            }
        }

        Some(Some(best))
    }

    fn max_tab_name_width(&self, state: &ZellijState, start: usize, end: usize) -> usize {
        let mut max_width = 1usize;

        for i in start..end {
            let tab = &state.tabs[i];
            let display_name = self.resolve_tab_name(
                tab,
                &state.panes,
                &state.tab_name_overrides,
                &state.tab_name_fallbacks,
                state.spinner_idx,
            );
            let tab_name = self.raw_tab_name(tab, &state.mode, &display_name);
            max_width = cmp::max(max_width, console::measure_text_width(tab_name));
        }

        max_width
    }

    fn separator_string(&self) -> String {
        self.separator
            .as_ref()
            .map(|sep| sep.format_string(&sep.content))
            .unwrap_or_default()
    }

    fn render_truncate_start(&self, truncated_start: usize) -> String {
        let mut output = String::new();
        for f in &self.tab_truncate_start_format {
            let mut content = f.content.clone();

            if content.contains("{count}") {
                content = content.replace("{count}", truncated_start.to_string().as_str());
            }

            output.push_str(&f.format_string(&content));
        }

        output
    }

    fn render_truncate_end(&self, truncated_end: usize) -> String {
        let mut output = String::new();
        for f in &self.tab_truncate_end_format {
            let mut content = f.content.clone();

            if content.contains("{count}") {
                content = content.replace("{count}", truncated_end.to_string().as_str());
            }

            output.push_str(&f.format_string(&content));
        }

        output
    }

    fn select_format(&self, info: &TabInfo, mode: &ModeInfo) -> &Vec<FormattedPart> {
        if info.active && mode.mode == InputMode::RenameTab {
            return &self.rename_tab_format;
        }

        if info.active && info.is_fullscreen_active {
            return &self.active_tab_fullscreen_format;
        }

        if info.active && info.is_sync_panes_active {
            return &self.active_tab_sync_format;
        }

        if info.active {
            return &self.active_tab_format;
        }

        if info.is_fullscreen_active {
            return &self.normal_tab_fullscreen_format;
        }

        if info.is_sync_panes_active {
            return &self.normal_tab_sync_format;
        }

        &self.normal_tab_format
    }

    fn raw_tab_name<'a>(
        &self,
        tab: &'a TabInfo,
        mode: &ModeInfo,
        display_name: &'a str,
    ) -> &'a str {
        match mode.mode {
            InputMode::RenameTab => match tab.name.is_empty() {
                true => "Enter name...",
                false => tab.name.as_str(),
            },
            _ => display_name,
        }
    }

    fn truncate_tab_name(&self, tab_name: &str, name_width_limit: Option<usize>) -> String {
        match name_width_limit {
            Some(width) => console::truncate_str(tab_name, width, "").into_owned(),
            None => tab_name.to_owned(),
        }
    }

    fn render_tab(
        &self,
        tab: &TabInfo,
        panes: &PaneManifest,
        mode: &ModeInfo,
        display_name: &str,
        name_width_limit: Option<usize>,
    ) -> String {
        let formatters = self.select_format(tab, mode);
        let mut output = "".to_owned();
        let tab_name =
            self.truncate_tab_name(self.raw_tab_name(tab, mode, display_name), name_width_limit);

        for f in formatters.iter() {
            let mut content = f.content.clone();

            if content.contains("{name}") {
                let rendered_name = render_inline_format(&tab_name, f.fg);
                content = content.replace("{name}", &rendered_name);
            }

            if content.contains("{index}") {
                content = content.replace("{index}", (tab.position + 1).to_string().as_str());
            }

            if content.contains("{floating_total_count}") {
                let panes_for_tab: Vec<PaneInfo> =
                    panes.panes.get(&tab.position).cloned().unwrap_or_default();

                content = content.replace(
                    "{floating_total_count}",
                    &format!("{}", panes_for_tab.iter().filter(|p| p.is_floating).count()),
                );
            }

            content = self.replace_indicators(content, tab, panes);

            output = format!("{}{}", output, f.format_string(&content));
        }

        output.to_owned()
    }

    fn replace_indicators(&self, content: String, tab: &TabInfo, panes: &PaneManifest) -> String {
        let mut content = content;
        if content.contains("{fullscreen_indicator}") && self.fullscreen_indicator.is_some() {
            content = content.replace(
                "{fullscreen_indicator}",
                if tab.is_fullscreen_active {
                    self.fullscreen_indicator.as_ref().unwrap()
                } else {
                    ""
                },
            );
        }

        if content.contains("{sync_indicator}") && self.sync_indicator.is_some() {
            content = content.replace(
                "{sync_indicator}",
                if tab.is_sync_panes_active {
                    self.sync_indicator.as_ref().unwrap()
                } else {
                    ""
                },
            );
        }

        if content.contains("{floating_indicator}") && self.floating_indicator.is_some() {
            let panes_for_tab: Vec<PaneInfo> =
                panes.panes.get(&tab.position).cloned().unwrap_or_default();

            let is_floating = panes_for_tab.iter().any(|p| p.is_floating);

            content = content.replace(
                "{floating_indicator}",
                if is_floating {
                    self.floating_indicator.as_ref().unwrap()
                } else {
                    ""
                },
            );
        }

        content
    }

    fn resolve_tab_name(
        &self,
        tab: &TabInfo,
        panes: &PaneManifest,
        overrides: &BTreeMap<usize, BTreeMap<u32, String>>,
        fallbacks: &BTreeMap<usize, BTreeMap<u32, String>>,
        spinner_idx: usize,
    ) -> String {
        if let Some(tab_ovr) = overrides.get(&tab.position) {
            if !tab_ovr.is_empty() {
                let frame = SPINNER_FRAMES[spinner_idx % SPINNER_FRAMES.len()];
                let tab_fb = fallbacks.get(&tab.position);

                let symbols: Vec<String> = tab_ovr
                    .values()
                    .map(|raw| {
                        let sym = raw.strip_prefix("🤖 ").unwrap_or(raw).to_string();
                        if sym.contains("{spin}") {
                            sym.replace("{spin}", frame)
                        } else {
                            sym
                        }
                    })
                    .collect();
                let chained = format!("🤖 {}", symbols.join(" "));

                let best_id = tab_ovr
                    .iter()
                    .find(|(_, s)| s.contains("{spin}"))
                    .or_else(|| tab_ovr.iter().next())
                    .map(|(id, _)| id)
                    .unwrap();
                let project = tab_fb.and_then(|m| m.get(best_id)).map(|s| s.as_str());
                let pane_title = panes
                    .panes
                    .get(&tab.position)
                    .and_then(|ps| ps.iter().find(|p| p.id == *best_id && !p.is_plugin))
                    .and_then(|p| {
                        let (title, _had_prefix) = strip_spinner_prefix(p.title.trim());
                        if title.is_empty() { None } else { Some(title) }
                    });

                let text = match (project, pane_title.as_deref()) {
                    (Some(p), Some(t)) => Some(format!("{} {}", p, t)),
                    (Some(p), None) => Some(p.to_string()),
                    (None, Some(t)) => Some(t.to_string()),
                    (None, None) => None,
                };

                let extra = tab_ovr.len() - 1;
                let suffix = if extra > 0 {
                    format!(" (+{})", extra)
                } else {
                    String::new()
                };
                let sep = if extra > 0 { " | " } else { " " };

                return if let Some(t) = text {
                    format!("{}{}{}{}", chained, sep, t, suffix)
                } else {
                    format!("{}{}", chained, suffix)
                };
            }
        }

        if is_default_tab_name(&tab.name) {
            let pane_title = panes
                .panes
                .get(&tab.position)
                .and_then(|ps| {
                    ps.iter()
                        .find(|p| p.is_focused && !p.is_plugin)
                        .or_else(|| ps.iter().find(|p| !p.is_plugin))
                })
                .and_then(|p| {
                    let (title, _had_prefix) = strip_spinner_prefix(p.title.trim());
                    if title.is_empty() { None } else { Some(title) }
                });

            if let Some(title) = pane_title {
                return title;
            }
        }

        tab.name.clone()
    }
}

/// Strip leading spinner prefix (e.g. "⠋ " or "● ") from pane titles.
/// Returns (stripped_text, had_prefix).
fn strip_spinner_prefix(s: &str) -> (String, bool) {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if !c.is_alphanumeric() => (chars.as_str().trim_start().to_string(), true),
        _ => (s.to_string(), false),
    }
}

fn is_default_tab_name(name: &str) -> bool {
    match name.strip_prefix("Tab #") {
        Some(num) => !num.is_empty() && num.chars().all(|c| c.is_ascii_digit()),
        None => false,
    }
}

fn render_inline_format(s: &str, parent_fg: Option<anstyle::Color>) -> String {
    if !s.contains("#[") {
        return s.to_string();
    }
    let restore_fg = color_to_ansi_fg(parent_fg);
    let mut result = String::new();
    let mut first = true;
    for segment in s.split("#[") {
        if first {
            result.push_str(segment);
            first = false;
            continue;
        }
        if let Some(bracket_end) = segment.find(']') {
            let attrs = &segment[..bracket_end];
            let text = &segment[bracket_end + 1..];
            if let Some(color_str) = attrs.strip_prefix("fg=") {
                if color_str.is_empty() {
                    result.push_str(&restore_fg);
                } else if let Some(hex) = color_str.strip_prefix('#') {
                    if let Ok(rgb) = hex_to_rgb(hex) {
                        result.push_str(&format!("\x1b[38;2;{};{};{}m", rgb.0, rgb.1, rgb.2));
                    }
                }
            }
            result.push_str(text);
        } else {
            result.push_str("#[");
            result.push_str(segment);
        }
    }
    result
}

fn color_to_ansi_fg(color: Option<anstyle::Color>) -> String {
    match color {
        Some(anstyle::Color::Rgb(anstyle::RgbColor(r, g, b))) => {
            format!("\x1b[38;2;{};{};{}m", r, g, b)
        }
        Some(anstyle::Color::Ansi(c)) => {
            let code = match c {
                anstyle::AnsiColor::Black => 30,
                anstyle::AnsiColor::Red => 31,
                anstyle::AnsiColor::Green => 32,
                anstyle::AnsiColor::Yellow => 33,
                anstyle::AnsiColor::Blue => 34,
                anstyle::AnsiColor::Magenta => 35,
                anstyle::AnsiColor::Cyan => 36,
                anstyle::AnsiColor::White => 37,
                anstyle::AnsiColor::BrightBlack => 90,
                anstyle::AnsiColor::BrightRed => 91,
                anstyle::AnsiColor::BrightGreen => 92,
                anstyle::AnsiColor::BrightYellow => 93,
                anstyle::AnsiColor::BrightBlue => 94,
                anstyle::AnsiColor::BrightMagenta => 95,
                anstyle::AnsiColor::BrightCyan => 96,
                anstyle::AnsiColor::BrightWhite => 97,
            };
            format!("\x1b[{}m", code)
        }
        Some(anstyle::Color::Ansi256(anstyle::Ansi256Color(n))) => {
            format!("\x1b[38;5;{}m", n)
        }
        None => "\x1b[39m".to_string(),
    }
}

fn hex_to_rgb(hex: &str) -> Result<(u8, u8, u8), ()> {
    if hex.len() != 6 {
        return Err(());
    }
    let r = u8::from_str_radix(&hex[0..2], 16).map_err(|_| ())?;
    let g = u8::from_str_radix(&hex[2..4], 16).map_err(|_| ())?;
    let b = u8::from_str_radix(&hex[4..6], 16).map_err(|_| ())?;
    Ok((r, g, b))
}

pub fn get_tab_window(
    tabs: &Vec<TabInfo>,
    max_count: Option<usize>,
) -> (usize, usize, Vec<TabInfo>) {
    let (first_index, last_index) = get_tab_window_indices(tabs, max_count);

    (
        first_index,
        tabs.len().saturating_sub(last_index),
        tabs.as_slice()[first_index..last_index].to_vec(),
    )
}

fn get_tab_window_indices(tabs: &Vec<TabInfo>, max_count: Option<usize>) -> (usize, usize) {
    let max_count = match max_count {
        Some(count) => count,
        None => return (0, tabs.len()),
    };

    if tabs.len() <= max_count {
        return (0, tabs.len());
    }

    let active_index = tabs.iter().position(|t| t.active).unwrap_or(0);
    let left_slots = (max_count.saturating_sub(1)) / 2;
    let mut first_index = active_index.saturating_sub(left_slots);
    let mut last_index = first_index + max_count;

    if last_index > tabs.len() {
        last_index = tabs.len();
        first_index = last_index.saturating_sub(max_count);
    }

    (first_index, last_index)
}

#[cfg(test)]
mod test {
    use zellij_tile::prelude::TabInfo;

    use super::get_tab_window;
    use rstest::rstest;

    #[rstest]
    #[case(
        vec![
            TabInfo {
                active: false,
                name: "1".to_owned(),
                ..TabInfo::default()
            },
            TabInfo {
                active: false,
                name: "2".to_owned(),
                ..TabInfo::default()
            },
            TabInfo {
                active: true,
                name: "3".to_owned(),
                ..TabInfo::default()
            },
            TabInfo {
                active: false,
                name: "4".to_owned(),
                ..TabInfo::default()
            },
            TabInfo {
                active: false,
                name: "5".to_owned(),
                ..TabInfo::default()
            },
        ],
        Some(3),
        (1, 1, vec![
                TabInfo {
                    active: false,
                    name: "2".to_owned(),
                    ..TabInfo::default()
                },
                TabInfo {
                    active: true,
                    name: "3".to_owned(),
                    ..TabInfo::default()
                },
                TabInfo {
                    active: false,
                    name: "4".to_owned(),
                    ..TabInfo::default()
                },
            ]
        )
    )]
    #[case(
        vec![
            TabInfo {
                active: true,
                name: "1".to_owned(),
                ..TabInfo::default()
            },
            TabInfo {
                active: false,
                name: "2".to_owned(),
                ..TabInfo::default()
            },
            TabInfo {
                active: false,
                name: "3".to_owned(),
                ..TabInfo::default()
            },
            TabInfo {
                active: false,
                name: "4".to_owned(),
                ..TabInfo::default()
            },
            TabInfo {
                active: false,
                name: "5".to_owned(),
                ..TabInfo::default()
            },
        ],
        Some(3),
        (0, 2, vec![
                TabInfo {
                    active: true,
                    name: "1".to_owned(),
                    ..TabInfo::default()
                },
                TabInfo {
                    active: false,
                    name: "2".to_owned(),
                    ..TabInfo::default()
                },
                TabInfo {
                    active: false,
                    name: "3".to_owned(),
                    ..TabInfo::default()
                },
            ]
        )
    )]
    #[case(
        vec![
            TabInfo {
                active: false,
                name: "1".to_owned(),
                ..TabInfo::default()
            },
            TabInfo {
                active: true,
                name: "2".to_owned(),
                ..TabInfo::default()
            },
            TabInfo {
                active: false,
                name: "3".to_owned(),
                ..TabInfo::default()
            },
            TabInfo {
                active: false,
                name: "4".to_owned(),
                ..TabInfo::default()
            },
            TabInfo {
                active: false,
                name: "5".to_owned(),
                ..TabInfo::default()
            },
        ],
        Some(3),
        (0, 2, vec![
                TabInfo {
                    active: false,
                    name: "1".to_owned(),
                    ..TabInfo::default()
                },
                TabInfo {
                    active: true,
                    name: "2".to_owned(),
                    ..TabInfo::default()
                },
                TabInfo {
                    active: false,
                    name: "3".to_owned(),
                    ..TabInfo::default()
                },
            ]
        )
    )]
    #[case(
        vec![
            TabInfo {
                active: false,
                name: "1".to_owned(),
                ..TabInfo::default()
            },
            TabInfo {
                active: false,
                name: "2".to_owned(),
                ..TabInfo::default()
            },
            TabInfo {
                active: false,
                name: "3".to_owned(),
                ..TabInfo::default()
            },
            TabInfo {
                active: false,
                name: "4".to_owned(),
                ..TabInfo::default()
            },
            TabInfo {
                active: true,
                name: "5".to_owned(),
                ..TabInfo::default()
            },
        ],
        Some(3),
        (2, 0, vec![
                TabInfo {
                    active: false,
                    name: "3".to_owned(),
                    ..TabInfo::default()
                },
                TabInfo {
                    active: false,
                    name: "4".to_owned(),
                    ..TabInfo::default()
                },
                TabInfo {
                    active: true,
                    name: "5".to_owned(),
                    ..TabInfo::default()
                },
            ]
        )
    )]
    #[case(
        vec![
            TabInfo {
                active: false,
                name: "1".to_owned(),
                ..TabInfo::default()
            },
            TabInfo {
                active: false,
                name: "2".to_owned(),
                ..TabInfo::default()
            },
            TabInfo {
                active: false,
                name: "3".to_owned(),
                ..TabInfo::default()
            },
            TabInfo {
                active: true,
                name: "4".to_owned(),
                ..TabInfo::default()
            },
            TabInfo {
                active: false,
                name: "5".to_owned(),
                ..TabInfo::default()
            },
        ],
        Some(3),
        (2, 0, vec![
                TabInfo {
                    active: false,
                    name: "3".to_owned(),
                    ..TabInfo::default()
                },
                TabInfo {
                    active: true,
                    name: "4".to_owned(),
                    ..TabInfo::default()
                },
                TabInfo {
                    active: false,
                    name: "5".to_owned(),
                    ..TabInfo::default()
                },
            ]
        )
    )]
    #[case(
        vec![
            TabInfo {
                active: false,
                name: "1".to_owned(),
                ..TabInfo::default()
            },
            TabInfo {
                active: false,
                name: "2".to_owned(),
                ..TabInfo::default()
            },
            TabInfo {
                active: true,
                name: "3".to_owned(),
                ..TabInfo::default()
            },
            TabInfo {
                active: false,
                name: "4".to_owned(),
                ..TabInfo::default()
            },
            TabInfo {
                active: false,
                name: "5".to_owned(),
                ..TabInfo::default()
            },
        ],
        None,
        (0, 0, vec![
            TabInfo {
                active: false,
                name: "1".to_owned(),
                ..TabInfo::default()
            },
            TabInfo {
                active: false,
                name: "2".to_owned(),
                ..TabInfo::default()
            },
            TabInfo {
                active: true,
                name: "3".to_owned(),
                ..TabInfo::default()
            },
            TabInfo {
                active: false,
                name: "4".to_owned(),
                ..TabInfo::default()
            },
            TabInfo {
                active: false,
                name: "5".to_owned(),
                ..TabInfo::default()
            },
            ]
        )
    )]
    #[case(
        vec![
            TabInfo {
                active: false,
                name: "1".to_owned(),
                ..TabInfo::default()
            },
            TabInfo {
                active: true,
                name: "2".to_owned(),
                ..TabInfo::default()
            },
        ],
        Some(3),
        (0, 0, vec![
            TabInfo {
                active: false,
                name: "1".to_owned(),
                ..TabInfo::default()
            },
            TabInfo {
                active: true,
                name: "2".to_owned(),
                ..TabInfo::default()
            },
            ]
        )
    )]
    #[case(
        vec![
            TabInfo {
                active: false,
                name: "1".to_owned(),
                ..TabInfo::default()
            },
            TabInfo {
                active: true,
                name: "2".to_owned(),
                ..TabInfo::default()
            },
            TabInfo {
                active: false,
                name: "3".to_owned(),
                ..TabInfo::default()
            },
        ],
        Some(3),
        (0, 0, vec![
            TabInfo {
                active: false,
                name: "1".to_owned(),
                ..TabInfo::default()
            },
            TabInfo {
                active: true,
                name: "2".to_owned(),
                ..TabInfo::default()
            },
            TabInfo {
                active: false,
                name: "3".to_owned(),
                ..TabInfo::default()
            },
            ]
        )
    )]
    pub fn test_get_tab_window(
        #[case] tabs: Vec<TabInfo>,
        #[case] max_count: Option<usize>,
        #[case] expected: (usize, usize, Vec<TabInfo>),
    ) {
        let res = get_tab_window(&tabs, max_count);

        assert_eq!(res, expected);
    }
}
