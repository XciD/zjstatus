use zellij_tile::prelude::*;

use chrono::Local;
use std::{collections::BTreeMap, sync::Arc};
use uuid::Uuid;

const LOADER_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

use zjstatus::{
    config::{self, ModuleConfig, UpdateEventMask, ZellijState},
    frames, pipe,
    widgets::{
        command::{CommandResult, CommandWidget},
        datetime::DateTimeWidget,
        mode::ModeWidget,
        notification::NotificationWidget,
        pipe::PipeWidget,
        session::SessionWidget,
        swap_layout::SwapLayoutWidget,
        tabs::TabsWidget,
        widget::Widget,
    },
};

#[derive(Default)]
struct State {
    pending_events: Vec<Event>,
    got_permissions: bool,
    state: ZellijState,
    userspace_configuration: BTreeMap<String, String>,
    module_config: config::ModuleConfig,
    widget_map: BTreeMap<String, Arc<dyn Widget>>,
    err: Option<anyhow::Error>,
    timer_active: bool,
    pending_pipe_overrides: Vec<(u32, String, Option<String>)>,
    sync_requested: bool,
    sync_complete: bool,
    last_render_output: String,
    render_dirty: bool,
    last_timer_ts: i64,
    last_good_cols: usize,
    pending_small_cols: Option<usize>,
    small_cols_seen: u8,
}

#[cfg(not(test))]
register_plugin!(State);

#[cfg(feature = "tracing")]
fn init_tracing() {
    use std::fs::File;
    use tracing_subscriber::filter::LevelFilter;
    use tracing_subscriber::Layer;
    use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

    let file = File::create("/host/.zjstatus.log");
    let file = match file {
        Ok(file) => file,
        Err(error) => panic!("Error: {:?}", error),
    };
    let debug_log = tracing_subscriber::fmt::layer()
        .with_writer(Arc::new(file))
        .with_filter(LevelFilter::DEBUG);

    tracing_subscriber::registry().with(debug_log).init();

    tracing::info!("tracing initialized");
}

impl ZellijPlugin for State {
    fn load(&mut self, configuration: BTreeMap<String, String>) {
        #[cfg(feature = "tracing")]
        init_tracing();

        // we need the ReadApplicationState permission to receive the ModeUpdate and TabUpdate
        // events
        // we need the RunCommands permission to run "cargo test" in a floating window
        request_permission(&[
            PermissionType::ReadApplicationState,
            PermissionType::ChangeApplicationState,
            PermissionType::RunCommands,
            PermissionType::MessageAndLaunchOtherPlugins,
        ]);

        subscribe(&[
            EventType::Mouse,
            EventType::ModeUpdate,
            EventType::PaneUpdate,
            EventType::PermissionRequestResult,
            EventType::TabUpdate,
            EventType::RunCommandResult,
            EventType::Timer,
        ]);

        self.module_config = match ModuleConfig::new(&configuration) {
            Ok(mc) => mc,
            Err(e) => {
                self.err = Some(e);
                return;
            }
        };
        self.widget_map = register_widgets(&configuration);
        self.userspace_configuration = configuration;
        self.pending_events = Vec::new();
        self.got_permissions = false;
        let uid = Uuid::new_v4();

        self.state = ZellijState {
            cols: 0,
            right_width: 0,
            tabs_available: None,
            suppress_tabs: false,
            command_results: BTreeMap::new(),
            pipe_results: BTreeMap::new(),
            mode: ModeInfo::default(),
            panes: PaneManifest::default(),
            plugin_uuid: uid.to_string(),
            tabs: Vec::new(),
            sessions: Vec::new(),
            start_time: Local::now(),
            cache_mask: 0,
            incoming_notification: None,
            tab_name_overrides: BTreeMap::new(),
            tab_name_fallbacks: BTreeMap::new(),
            spinner_idx: 0,
        };

        set_timeout(0.1); // start loader animation
    }

    fn pipe(&mut self, pipe_message: PipeMessage) -> bool {
        // Handle sync request: respond only to the requester plugin
        if pipe_message.name == "zjstatus_sync" {
            if let Some(source_str) = pipe_message.args.get("source_plugin_id") {
                if let Ok(source_id) = source_str.parse::<u32>() {
                    let my_id = get_plugin_ids().plugin_id;
                    if source_id != my_id && !self.state.tab_name_overrides.is_empty() {
                        self.send_state_to(source_id);
                    }
                }
            }
            return false;
        }

        // Handle "title" pipe for tab naming
        if pipe_message.name == "title" {
            // If a session arg is provided, verify it matches this session
            if let Some(target_session) = pipe_message.args.get("session") {
                if let Some(ref current_session) = self.state.mode.session_name {
                    if target_session != current_session {
                        return false;
                    }
                }
            }

            if let Some(payload) = pipe_message.payload {
                let pane_id = pipe_message
                    .args
                    .get("pane_id")
                    .and_then(|s| s.parse::<u32>().ok());

                let tab_pos = pane_id.and_then(|id| self.find_tab_for_pane(id));

                if tab_pos.is_none() {
                    if let Some(id) = pane_id {
                        // PaneManifest not ready yet, store for later resolution
                        let fb = pipe_message.args.get("fallback").cloned();
                        self.pending_pipe_overrides.push((id, payload, fb));
                        return false;
                    }
                }

                // Empty payload = clear override (search all tabs since pane may be gone)
                if payload.is_empty() {
                    if let Some(id) = pane_id {
                        for inner in self.state.tab_name_overrides.values_mut() {
                            inner.remove(&id);
                        }
                        self.state.tab_name_overrides.retain(|_, m| !m.is_empty());
                        for inner in self.state.tab_name_fallbacks.values_mut() {
                            inner.remove(&id);
                        }
                        self.state.tab_name_fallbacks.retain(|_, m| !m.is_empty());
                        self.state.cache_mask = UpdateEventMask::Tab as u8;
                        self.render_dirty = true;
                        return true;
                    }
                    return false;
                }

                if let Some(pos) = tab_pos {
                    let id = pane_id.unwrap();
                    self.state.tab_name_overrides.entry(pos).or_default().insert(id, payload);
                    if let Some(fb) = pipe_message.args.get("fallback") {
                        self.state.tab_name_fallbacks.entry(pos).or_default().insert(id, fb.clone());
                    }
                    self.ensure_timer();
                    self.state.cache_mask = UpdateEventMask::Tab as u8;
                    self.render_dirty = true;
                    return true;
                }
            }
            return false;
        }

        // Handle "focus" pipe to switch to tab containing a pane
        if pipe_message.name == "focus" {
            // Verify session matches
            if let Some(target_session) = pipe_message.args.get("session") {
                if let Some(ref current_session) = self.state.mode.session_name {
                    if target_session != current_session {
                        return false;
                    }
                }
            }
            if let Some(pane_id) = pipe_message
                .args
                .get("pane_id")
                .and_then(|s| s.parse::<u32>().ok())
            {
                focus_terminal_pane(pane_id, false);
            }
            return false;
        }

        let mut should_render = false;

        match pipe_message.source {
            PipeSource::Cli(_) | PipeSource::Plugin(_) | PipeSource::Keybind => {
                if let Some(input) = pipe_message.payload {
                    should_render = pipe::parse_protocol(&mut self.state, &input);
                }
            }
        }

        if should_render {
            self.render_dirty = true;
        }
        should_render
    }

    fn update(&mut self, event: Event) -> bool {
        if let Event::PermissionRequestResult(PermissionStatus::Granted) = event {
            self.got_permissions = true;

            while !self.pending_events.is_empty() {
                let ev = self.pending_events.pop();

                self.handle_event(ev.unwrap());
            }
        }

        if !self.got_permissions {
            self.pending_events.push(event);

            return false;
        }

        self.handle_event(event)
    }

    fn render(&mut self, _rows: usize, cols: usize) {
        if !self.got_permissions {
            return;
        }

        if let Some(err) = &self.err {
            println!("Error: {:?}", err);

            return;
        }

        if !self.sync_complete {
            let frame = LOADER_FRAMES[self.state.spinner_idx % LOADER_FRAMES.len()];
            print!("{}", frame);
            return;
        }

        if !self.is_in_active_tab() {
            return;
        }

        let prev_cols = self.state.cols;
        if self.last_good_cols == 0 {
            self.last_good_cols = prev_cols.max(cols);
        }

        if cols < self.last_good_cols
            && !self.last_render_output.is_empty()
            && self.last_good_cols.saturating_sub(cols) > 10
        {
            if self.pending_small_cols != Some(cols) {
                self.pending_small_cols = Some(cols);
                self.small_cols_seen = 1;
            } else {
                self.small_cols_seen = self.small_cols_seen.saturating_add(1);
            }

            if self.small_cols_seen < 2 {
                tracing::debug!(
                    cols,
                    last_good_cols = self.last_good_cols,
                    "render: transient small cols"
                );
                set_timeout(0.1);
                print!("{}", self.last_render_output);
                return;
            }
        }

        self.pending_small_cols = None;
        self.small_cols_seen = 0;
        self.last_good_cols = cols;

        let cols_changed = prev_cols != cols;
        self.state.cols = cols;

        if !self.render_dirty && !cols_changed && !self.last_render_output.is_empty() {
            tracing::debug!("render: cached");
            print!("{}", self.last_render_output);
            return;
        }

        tracing::debug!(dirty = self.render_dirty, cols_changed, "render: recompute");

        self.render_dirty = false;
        self.ensure_timer();

        let output = self
            .module_config
            .render_bar(&mut self.state, &self.widget_map);

        self.last_render_output = output.clone();
        self.state.cache_mask = 0;
        print!("{}", output);
    }
}

impl State {
    fn ensure_timer(&mut self) {
        if self.timer_active {
            return;
        }
        self.timer_active = true;
        let has_spin = self
            .state
            .tab_name_overrides
            .values()
            .flat_map(|m| m.values())
            .any(|v| v.contains("{spin}"));
        if has_spin {
            set_timeout(0.4);
        } else {
            set_timeout(1.0);
        }
    }

    fn resolve_pending_overrides(&mut self) {
        let pending = std::mem::take(&mut self.pending_pipe_overrides);
        for (pane_id, payload, fallback) in pending {
            if let Some(pos) = self.find_tab_for_pane(pane_id) {
                if payload.is_empty() {
                    if let Some(inner) = self.state.tab_name_overrides.get_mut(&pos) {
                        inner.remove(&pane_id);
                        if inner.is_empty() {
                            self.state.tab_name_overrides.remove(&pos);
                        }
                    }
                } else {
                    self.state.tab_name_overrides.entry(pos).or_default().insert(pane_id, payload);
                    if let Some(fb) = fallback {
                        self.state.tab_name_fallbacks.entry(pos).or_default().insert(pane_id, fb);
                    }
                }
            } else {
                self.pending_pipe_overrides.push((pane_id, payload, fallback));
            }
        }
    }

    /// Request sync from other plugins (called once when newly initialized).
    /// Broadcasts to all plugins; responders use our plugin_id to reply targeted.
    fn request_sync(&mut self) {
        if self.sync_requested
            || self.state.tabs.is_empty()
            || self.state.panes.panes.is_empty()
        {
            return;
        }
        self.sync_requested = true;
        let my_id = get_plugin_ids().plugin_id;
        let mut args = BTreeMap::new();
        args.insert("source_plugin_id".to_string(), my_id.to_string());
        pipe_message_to_plugin(
            MessageToPlugin::new("zjstatus_sync")
                .with_payload("sync")
                .with_args(args),
        );
        set_timeout(0.3); // wait for sync responses before rendering bar
    }

    /// Send current overrides to a specific plugin instance
    fn send_state_to(&self, target_plugin_id: u32) {
        let session = match &self.state.mode.session_name {
            Some(s) => s,
            None => return,
        };
        for (_tab_pos, inner) in &self.state.tab_name_overrides {
            for (pane_id, payload) in inner {
                let fallback = self.state.tab_name_fallbacks
                    .values()
                    .flat_map(|m| m.get(pane_id))
                    .next()
                    .cloned()
                    .unwrap_or_default();
                let mut args = BTreeMap::new();
                args.insert("pane_id".to_string(), pane_id.to_string());
                args.insert("fallback".to_string(), fallback);
                args.insert("session".to_string(), session.clone());
                // Replace {spin} with static symbol so synced instances don't run timers
                let synced_payload = payload.replace("{spin}", "●");
                pipe_message_to_plugin(
                    MessageToPlugin::new("title")
                        .with_destination_plugin_id(target_plugin_id)
                        .with_payload(synced_payload)
                        .with_args(args),
                );
            }
        }
    }

    fn tabs_changed(&self, new_tabs: &[TabInfo]) -> bool {
        if self.state.tabs.len() != new_tabs.len() {
            return true;
        }
        for (old, new) in self.state.tabs.iter().zip(new_tabs.iter()) {
            if old.name != new.name || old.active != new.active || old.position != new.position {
                return true;
            }
        }
        false
    }

    fn find_tab_for_pane(&self, pane_id: u32) -> Option<usize> {
        for tab in &self.state.tabs {
            if let Some(panes) = self.state.panes.panes.get(&tab.position) {
                if panes.iter().any(|p| p.id == pane_id && !p.is_plugin) {
                    return Some(tab.position);
                }
            }
        }
        None
    }

    fn is_in_active_tab(&self) -> bool {
        if self.state.tabs.is_empty() || self.state.panes.panes.is_empty() {
            return true; // early startup: allow initial render
        }

        let my_id = get_plugin_ids().plugin_id;
        for tab in &self.state.tabs {
            if let Some(panes) = self.state.panes.panes.get(&tab.position) {
                if panes.iter().any(|p| p.is_plugin && p.id == my_id) {
                    return tab.active;
                }
            }
        }

        false // default to inactive once panes/tabs are known
    }

    fn handle_event(&mut self, event: Event) -> bool {
        let mut should_render = false;
        let mut evt_name = "Other";
        match event {
            Event::Mouse(mouse_info) => {
                evt_name = "Mouse";
                self.module_config.handle_mouse_action(
                    &self.state,
                    mouse_info,
                    &self.widget_map,
                );
            }
            Event::ModeUpdate(mode_info) => {
                evt_name = "ModeUpdate";
                self.state.mode = mode_info;
                self.state.cache_mask = UpdateEventMask::Mode as u8;

                should_render = self.is_in_active_tab();
            }
            Event::PaneUpdate(pane_info) => {
                evt_name = "PaneUpdate";
                frames::hide_frames_conditionally(
                    &frames::FrameConfig::new(
                        self.module_config.hide_frame_for_single_pane,
                        self.module_config.hide_frame_except_for_search,
                        self.module_config.hide_frame_except_for_fullscreen,
                        self.module_config.hide_frame_except_for_scroll,
                    ),
                    &self.state.tabs,
                    &pane_info,
                    &self.state.mode,
                    get_plugin_ids(),
                    false,
                );

                self.state.panes = pane_info;

                let had_pending = !self.pending_pipe_overrides.is_empty();
                self.resolve_pending_overrides();
                self.request_sync();

                // Only re-render if pending overrides were resolved
                if had_pending {
                    self.state.cache_mask = UpdateEventMask::Tab as u8;
                    let active = self.is_in_active_tab();
                    if active {
                        self.ensure_timer();
                    }
                    should_render = active;
                }
            }
            Event::PermissionRequestResult(result) => {
                let _ = result;
                set_selectable(false);
            }
            Event::RunCommandResult(exit_code, stdout, stderr, context) => {
                evt_name = "RunCommandResult";
                if let Some(name) = context.get("name") {
                    self.state.cache_mask = UpdateEventMask::Command as u8;

                    let stdout = match String::from_utf8(stdout) {
                        Ok(s) => s,
                        Err(_) => "".to_owned(),
                    };

                    let stderr = match String::from_utf8(stderr) {
                        Ok(s) => s,
                        Err(_) => "".to_owned(),
                    };

                    self.state.command_results.insert(
                        name.to_owned(),
                        CommandResult {
                            exit_code,
                            stdout,
                            stderr,
                            context,
                        },
                    );
                    should_render = self.is_in_active_tab();
                }
            }
            Event::TabUpdate(tab_info) => {
                evt_name = "TabUpdate";
                let changed = self.tabs_changed(&tab_info);
                self.state.tabs = tab_info;

                // Always try sync/overrides resolution (needed at startup)
                self.resolve_pending_overrides();
                self.request_sync();

                if changed {
                    self.state.cache_mask = UpdateEventMask::Tab as u8;

                    // Rebuild overrides with correct tab positions (positions shift when tabs are closed)
                    let mut new_overrides: BTreeMap<usize, BTreeMap<u32, String>> = BTreeMap::new();
                    for (_, inner) in std::mem::take(&mut self.state.tab_name_overrides) {
                        for (pane_id, value) in inner {
                            if let Some(new_pos) = self.find_tab_for_pane(pane_id) {
                                new_overrides.entry(new_pos).or_default().insert(pane_id, value);
                            }
                        }
                    }
                    self.state.tab_name_overrides = new_overrides;

                    let mut new_fallbacks: BTreeMap<usize, BTreeMap<u32, String>> = BTreeMap::new();
                    for (_, inner) in std::mem::take(&mut self.state.tab_name_fallbacks) {
                        for (pane_id, value) in inner {
                            if let Some(new_pos) = self.find_tab_for_pane(pane_id) {
                                new_fallbacks.entry(new_pos).or_default().insert(pane_id, value);
                            }
                        }
                    }
                    self.state.tab_name_fallbacks = new_fallbacks;

                    let active = self.is_in_active_tab();
                    if active {
                        self.ensure_timer();
                    }
                    should_render = active;
                }
            }
            Event::Timer(_) => {
                evt_name = "Timer";
                self.state.spinner_idx = self.state.spinner_idx.wrapping_add(1);

                // During sync, only run the sync timer — don't start the active tab timer
                let mut syncing = false;
                if !self.sync_complete {
                    if self.sync_requested && self.state.spinner_idx >= 3 {
                        self.sync_complete = true;
                    } else if self.state.spinner_idx > 10 {
                        self.sync_complete = true;
                    } else {
                        set_timeout(0.1);
                        should_render = true; // animate loading spinner
                        syncing = true;
                    }
                }

                if !syncing {
                    let now = Local::now().timestamp_millis();
                    // Drain stacked timers: skip if fired too recently
                    if now - self.last_timer_ts < 300 {
                        // Stacked timer — consume without rescheduling
                        self.timer_active = false;
                    } else if self.is_in_active_tab() {
                        self.last_timer_ts = now;
                        self.state.cache_mask = UpdateEventMask::Tab as u8;

                        // Schedule command re-execution without invalidating widget caches
                        if let Some(cmd_widget) = self.widget_map.get("command") {
                            cmd_widget.tick(&self.state);
                        }
                        let has_spin = self
                            .state
                            .tab_name_overrides
                            .values()
                            .flat_map(|m| m.values())
                            .any(|v| v.contains("{spin}"));
                        if has_spin {
                            set_timeout(0.4);
                            should_render = true;
                        } else {
                            set_timeout(1.0);
                            // No render needed — RunCommandResult will trigger render
                        }
                    } else {
                        self.timer_active = false;
                    }
                }
            }
            _ => (),
        };
        if should_render {
            self.render_dirty = true;
            tracing::debug!(
                evt_name,
                plugin_id = get_plugin_ids().plugin_id,
                cache_mask = self.state.cache_mask,
                "handle_event: dirty"
            );
        }
        should_render
    }
}

fn register_widgets(configuration: &BTreeMap<String, String>) -> BTreeMap<String, Arc<dyn Widget>> {
    let mut widget_map = BTreeMap::<String, Arc<dyn Widget>>::new();

    widget_map.insert(
        "command".to_owned(),
        Arc::new(CommandWidget::new(configuration)),
    );
    widget_map.insert(
        "datetime".to_owned(),
        Arc::new(DateTimeWidget::new(configuration)),
    );
    widget_map.insert("pipe".to_owned(), Arc::new(PipeWidget::new(configuration)));
    widget_map.insert(
        "swap_layout".to_owned(),
        Arc::new(SwapLayoutWidget::new(configuration)),
    );
    widget_map.insert("mode".to_owned(), Arc::new(ModeWidget::new(configuration)));
    widget_map.insert(
        "session".to_owned(),
        Arc::new(SessionWidget::new(configuration)),
    );
    widget_map.insert("tabs".to_owned(), Arc::new(TabsWidget::new(configuration)));
    widget_map.insert(
        "notifications".to_owned(),
        Arc::new(NotificationWidget::new(configuration)),
    );

    widget_map
}
