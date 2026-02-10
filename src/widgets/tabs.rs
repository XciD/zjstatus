use std::{cmp, collections::BTreeMap};

use zellij_tile::{
    prelude::{InputMode, ModeInfo, PaneInfo, PaneManifest, TabInfo},
    shim::switch_tab_to,
};

use anstyle::Style;

use crate::{config::ZellijState, render::FormattedPart};

use super::widget::Widget;

const SPINNER_FRAMES: &[&str] = &["✦", "✶", "✽", "✶"];

/// Strip leading spinner prefix (e.g. "⠋ " or "● ") from pane titles.
/// Returns (stripped_text, had_prefix).
fn strip_spinner_prefix(s: &str) -> (String, bool) {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if !c.is_alphanumeric() => {
            (chars.as_str().trim_start().to_string(), true)
        }
        _ => (s.to_string(), false),
    }
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
    tab_name_max_len: usize,
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
            .map(|s| FormattedPart::from_format_string(s, config));

        let tab_name_max_len = config
            .get("tab_name_max_len")
            .and_then(|v| v.parse().ok())
            .unwrap_or(20);

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
            tab_name_max_len,
        }
    }
}

impl Widget for TabsWidget {
    fn process(&self, _name: &str, state: &ZellijState) -> String {
        let mut output = "".to_owned();
        let mut counter = 0;

        let (truncated_start, truncated_end, tabs) =
            get_tab_window(&state.tabs, self.tab_display_count);

        // ~6 chars overhead per tab (index, spaces, formatting)
        let per_tab_overhead = 6;
        let name_max_len = if !tabs.is_empty() && state.cols > 0 {
            let available = state.cols.saturating_sub(tabs.len() * per_tab_overhead);
            let dynamic = available / tabs.len();
            cmp::max(5, dynamic)
        } else {
            self.tab_name_max_len
        };

        if truncated_start > 0 {
            for f in &self.tab_truncate_start_format {
                let mut content = f.content.clone();

                if content.contains("{count}") {
                    content = content.replace("{count}", (truncated_start).to_string().as_str());
                }

                output = format!("{output}{}", f.format_string(&content));
            }
        }

        for tab in &tabs {
            let display_name = self.resolve_tab_name(tab, &state.panes, &state.tab_name_overrides, &state.tab_name_fallbacks, state.spinner_idx, name_max_len);
            let content = self.render_tab(tab, &state.panes, &state.mode, &display_name);
            counter += 1;

            output = format!("{}{}", output, content);

            if counter < tabs.len()
                && let Some(sep) = &self.separator
            {
                output = format!("{}{}", output, sep.format_string(&sep.content));
            }
        }

        if truncated_end > 0 {
            for f in &self.tab_truncate_end_format {
                let mut content = f.content.clone();

                if content.contains("{count}") {
                    content = content.replace("{count}", (truncated_end).to_string().as_str());
                }

                output = format!("{output}{}", f.format_string(&content));
            }
        }

        output
    }

    fn process_click(&self, _name: &str, state: &ZellijState, pos: usize) {
        let mut offset = 0;
        let mut counter = 0;

        let (truncated_start, truncated_end, tabs) =
            get_tab_window(&state.tabs, self.tab_display_count);

        let active_pos = &state
            .tabs
            .iter()
            .find(|t| t.active)
            .expect("no active tab")
            .position
            + 1;

        if truncated_start > 0 {
            for f in &self.tab_truncate_start_format {
                let mut content = f.content.clone();

                if content.contains("{count}") {
                    content = content.replace("{count}", (truncated_end).to_string().as_str());
                }

                offset += console::measure_text_width(&f.format_string(&content));

                if pos <= offset {
                    switch_tab_to(active_pos.saturating_sub(1) as u32);
                }
            }
        }

        let per_tab_overhead = 6;
        let name_max_len = if !tabs.is_empty() && state.cols > 0 {
            let available = state.cols.saturating_sub(tabs.len() * per_tab_overhead);
            let dynamic = available / tabs.len();
            cmp::max(5, dynamic)
        } else {
            self.tab_name_max_len
        };

        for tab in &tabs {
            counter += 1;

            let display_name = self.resolve_tab_name(tab, &state.panes, &state.tab_name_overrides, &state.tab_name_fallbacks, state.spinner_idx, name_max_len);
            let mut rendered_content = self.render_tab(tab, &state.panes, &state.mode, &display_name);

            if counter < tabs.len()
                && let Some(sep) = &self.separator
            {
                rendered_content =
                    format!("{}{}", rendered_content, sep.format_string(&sep.content));
            }

            let content_len = console::measure_text_width(&rendered_content);

            if pos > offset && pos < offset + content_len {
                switch_tab_to(tab.position as u32 + 1);

                break;
            }

            offset += content_len;
        }

        if truncated_end > 0 {
            for f in &self.tab_truncate_end_format {
                let mut content = f.content.clone();

                if content.contains("{count}") {
                    content = content.replace("{count}", (truncated_end).to_string().as_str());
                }

                offset += console::measure_text_width(&f.format_string(&content));

                if pos <= offset {
                    switch_tab_to(cmp::min(active_pos + 1, state.tabs.len()) as u32);
                }
            }
        }
    }
}

impl TabsWidget {
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

    fn render_tab(
        &self,
        tab: &TabInfo,
        panes: &PaneManifest,
        mode: &ModeInfo,
        display_name: &str,
    ) -> String {
        let formatters = self.select_format(tab, mode);
        let mut output = "".to_owned();

        for f in formatters.iter() {
            let mut content = f.content.clone();

            let tab_name = match mode.mode {
                InputMode::RenameTab => match tab.name.is_empty() {
                    true => "Enter name...",
                    false => tab.name.as_str(),
                },
                _name => display_name,
            };

            if content.contains("{name}") {
                content = content.replace("{name}", tab_name);
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

            // If content contains inline format codes (e.g. from tab name),
            // re-parse and render each sub-part, inheriting parent style
            if content.contains("#[") {
                let empty_config = BTreeMap::new();
                let sub_parts =
                    FormattedPart::multiple_from_format_string(&content, &empty_config);
                for sub in &sub_parts {
                    let style = Style::new()
                        .fg_color(sub.fg.or(f.fg))
                        .bg_color(sub.bg.or(f.bg))
                        .underline_color(sub.us.or(f.us))
                        .effects(if sub.effects == anstyle::Effects::new() {
                            f.effects
                        } else {
                            sub.effects
                        });
                    output = format!(
                        "{}{}{}{}",
                        output,
                        style.render_reset(),
                        style.render(),
                        sub.content
                    );
                }
                output = format!("{}{}", output, Style::new().render_reset());
            } else {
                output = format!("{}{}", output, f.format_string(&content));
            }
        }

        output.to_owned()
    }

    fn resolve_tab_name(
        &self,
        tab: &TabInfo,
        panes: &PaneManifest,
        overrides: &BTreeMap<usize, BTreeMap<u32, String>>,
        fallbacks: &BTreeMap<usize, BTreeMap<u32, String>>,
        spinner_idx: usize,
        name_max_len: usize,
    ) -> String {
        let tab_panes = panes.panes.get(&tab.position);

        // Check for pipe overrides (multiple Claude instances possible)
        if let Some(tab_ovr) = overrides.get(&tab.position) {
            if !tab_ovr.is_empty() {
                let frame = SPINNER_FRAMES[spinner_idx % SPINNER_FRAMES.len()];
                let tab_fb = fallbacks.get(&tab.position);

                // Keep pane order (BTreeMap iterates by ascending pane_id = visual order)
                // Chain status symbols: strip "🤖 " prefix from each, combine under one 🤖
                let symbols: Vec<String> = tab_ovr
                    .values()
                    .map(|raw| {
                        let sym = raw.strip_prefix("🤖 ").unwrap_or(raw).to_string();
                        if sym.contains("{spin}") { sym.replace("{spin}", frame) } else { sym }
                    })
                    .collect();
                let chained = format!("🤖 {}", symbols.join(" "));

                // Text from the spinning Claude (or first if none spinning)
                let best_id = tab_ovr
                    .iter()
                    .find(|(_, s)| s.contains("{spin}"))
                    .or_else(|| tab_ovr.iter().next())
                    .map(|(id, _)| id)
                    .unwrap();
                let project = tab_fb
                    .and_then(|m| m.get(best_id))
                    .map(|s| s.as_str());
                let pane_title = tab_panes
                    .and_then(|ps| ps.iter().find(|p| p.id == *best_id && !p.is_plugin))
                    .and_then(|p| {
                        let (title, _) = strip_spinner_prefix(&p.title);
                        if title.is_empty() { None } else { Some(title) }
                    });

                let text = match (project, pane_title.as_deref()) {
                    (Some(p), Some(t)) => Some(format!("{} {}", p, t)),
                    (Some(p), None) => Some(p.to_string()),
                    (None, Some(t)) => Some(t.to_string()),
                    (None, None) => None,
                };

                let extra = tab_ovr.len() - 1;
                let suffix = if extra > 0 { format!(" (+{})", extra) } else { String::new() };
                let sep = if extra > 0 { " | " } else { " " };

                return if let Some(t) = text {
                    format!("{}{}{}{}", chained, sep, self.truncate_name_dynamic(&t, name_max_len), suffix)
                } else {
                    format!("{}{}", chained, suffix)
                };
            }
        }

        // No override: use focused pane title (🤖 prefix if Claude detected)
        let non_plugin: Vec<_> = tab_panes
            .map(|ps| ps.iter().filter(|p| !p.is_plugin).collect())
            .unwrap_or_default();

        let focused = non_plugin
            .iter()
            .find(|p| p.is_focused)
            .or(non_plugin.first());

        if let Some(pane) = focused {
            let (title, is_claude) = strip_spinner_prefix(&pane.title);
            if !title.is_empty() {
                let truncated = self.truncate_name_dynamic(&title, name_max_len);
                return if is_claude {
                    format!("🤖 {}", truncated)
                } else {
                    truncated
                };
            }
        }

        // Fallback to zellij tab name
        tab.name.clone()
    }

    fn truncate_name_dynamic(&self, name: &str, max_len: usize) -> String {
        // Use basename if it looks like a path
        let name = if name.contains('/') {
            name.rsplit('/').next().unwrap_or(name)
        } else {
            name
        };

        if max_len > 0 && name.chars().count() > max_len {
            let truncated: String = name.chars().take(max_len.saturating_sub(1)).collect();
            format!("{}…", truncated)
        } else {
            name.to_string()
        }
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
}

pub fn get_tab_window(
    tabs: &Vec<TabInfo>,
    max_count: Option<usize>,
) -> (usize, usize, Vec<TabInfo>) {
    let max_count = match max_count {
        Some(count) => count,
        None => return (0, 0, tabs.to_vec()),
    };

    if tabs.len() <= max_count {
        return (0, 0, tabs.to_vec());
    }

    let active_index = tabs.iter().position(|t| t.active).expect("no active tab");

    // active tab is in the last #max_count tabs, so return the last #max_count
    if active_index > tabs.len().saturating_sub(max_count) {
        return (
            tabs.len().saturating_sub(max_count),
            0,
            tabs.iter()
                .cloned()
                .rev()
                .take(max_count)
                .rev()
                .collect::<Vec<TabInfo>>(),
        );
    }

    // tabs must be truncated
    let first_index = active_index.saturating_sub(1);
    let last_index = cmp::min(first_index + max_count, tabs.len());

    (
        first_index,
        tabs.len().saturating_sub(last_index),
        tabs.as_slice()[first_index..last_index].to_vec(),
    )
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
