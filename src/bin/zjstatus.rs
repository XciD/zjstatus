use zellij_tile::prelude::*;

use chrono::Local;
use std::{
    collections::BTreeMap,
    sync::Arc,
};
use uuid::Uuid;

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
    has_command_widgets: bool,
    command_poll_interval_secs: f64,
    pending_pipe_overrides: Vec<(u32, String, Option<String>)>,
    last_tab_count: usize,
}

#[cfg(not(test))]
register_plugin!(State);

#[cfg(feature = "tracing")]
fn init_tracing() {
    use std::fs::File;
    use tracing_subscriber::layer::Layer;
    use tracing_subscriber::filter::LevelFilter;
    use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

    let file = File::create("/host/.zjstatus.log");
    let file = match file {
        Ok(file) => file,
        Err(error) => panic!("Error: {:?}", error),
    };
    let debug_log = tracing_subscriber::fmt::layer()
        .with_writer(Arc::new(file))
        .with_filter(LevelFilter::INFO);

    tracing_subscriber::registry().with(debug_log).init();
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
        self.has_command_widgets = self
            .userspace_configuration
            .keys()
            .any(|k| k.starts_with("command_") && k.ends_with("_command"));
        self.command_poll_interval_secs =
            command_poll_interval_secs_from_config(&self.userspace_configuration);
        self.pending_events = Vec::new();
        self.got_permissions = false;
        let uid = Uuid::new_v4();

        self.state = ZellijState {
            cols: 0,
            command_results: BTreeMap::new(),
            pipe_results: BTreeMap::new(),
            is_current_tab_plugin: false,
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
    }

    fn pipe(&mut self, pipe_message: PipeMessage) -> bool {
        // Handle "title" pipe for tab naming
        if pipe_message.name == "title" {
            if let Some(target_session) = pipe_message.args.get("session") {
                if let Some(ref current_session) = self.state.mode.session_name {
                    if target_session != current_session {
                        return false;
                    }
                }
            }

            if let Some(payload) = pipe_message.payload {
                let payload = sanitize_pipe_title_payload(payload);
                let pane_id = pipe_message
                    .args
                    .get("pane_id")
                    .and_then(|s| s.parse::<u32>().ok());

                let tab_pos = pane_id.and_then(|id| self.find_tab_for_pane(id));

                if tab_pos.is_none() {
                    if let Some(id) = pane_id {
                        let fb = pipe_message.args.get("fallback").cloned();
                        self.pending_pipe_overrides.retain(|(pid, _, _)| *pid != id);
                        self.pending_pipe_overrides.push((id, payload, fb));
                        return false;
                    }
                }

                // Empty payload = clear override
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
                        return true;
                    }
                    return false;
                }

                if let Some(pos) = tab_pos {
                    let id = pane_id.unwrap();
                    let incoming_fallback = pipe_message.args.get("fallback").cloned();
                    let payload_unchanged = self
                        .state
                        .tab_name_overrides
                        .get(&pos)
                        .and_then(|m| m.get(&id))
                        == Some(&payload);
                    let fallback_unchanged = match incoming_fallback.as_ref() {
                        Some(fb) => {
                            self.state
                                .tab_name_fallbacks
                                .get(&pos)
                                .and_then(|m| m.get(&id))
                                == Some(fb)
                        }
                        None => true,
                    };
                    if payload_unchanged && fallback_unchanged {
                        self.ensure_timer();
                        return false;
                    }

                    self.state
                        .tab_name_overrides
                        .entry(pos)
                        .or_default()
                        .insert(id, payload);
                    if let Some(fb) = incoming_fallback {
                        self.state
                            .tab_name_fallbacks
                            .entry(pos)
                            .or_default()
                            .insert(id, fb);
                    }
                    self.ensure_timer();
                    self.state.cache_mask = UpdateEventMask::Tab as u8;
                    return true;
                }
            }
            return false;
        }

        // Handle "focus" pipe to switch to tab containing a pane
        if pipe_message.name == "focus" {
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

        should_render
    }

    #[tracing::instrument(skip_all, fields(event_type))]
    fn update(&mut self, event: Event) -> bool {
        if let Event::PermissionRequestResult(PermissionStatus::Granted) = event {
            self.got_permissions = true;

            while !self.pending_events.is_empty() {
                tracing::debug!("processing cached event");
                let ev = self.pending_events.pop();

                self.handle_event(ev.unwrap());
            }
        }

        if !self.got_permissions {
            tracing::debug!("caching event");
            self.pending_events.push(event);

            return false;
        }

        self.handle_event(event)
    }

    #[tracing::instrument(skip_all)]
    fn render(&mut self, _rows: usize, cols: usize) {
        if !self.got_permissions {
            return;
        }

        if let Some(err) = &self.err {
            println!("Error: {:?}", err);

            return;
        }

        self.state.cols = cols;

        tracing::debug!("{:?}", self.state.mode.session_name);

        let output = self
            .module_config
            .render_bar(self.state.clone(), self.widget_map.clone());

        print!("{}", output);
    }
}

impl State {
    fn handle_event(&mut self, event: Event) -> bool {
        let mut should_render = false;
        match event {
            Event::Mouse(mouse_info) => {
                if !self.state.is_current_tab_plugin {
                    return false;
                }
                tracing::Span::current().record("event_type", "Event::Mouse");
                tracing::debug!(mouse = ?mouse_info);

                self.module_config.handle_mouse_action(
                    self.state.clone(),
                    mouse_info,
                    self.widget_map.clone(),
                );
            }
            Event::ModeUpdate(mode_info) => {
                tracing::Span::current().record("event_type", "Event::ModeUpdate");
                tracing::debug!(mode = ?mode_info.mode);
                tracing::debug!(mode = ?mode_info.session_name);

                self.state.mode = mode_info;
                self.state.cache_mask = UpdateEventMask::Mode as u8;

                should_render = true;
            }
            Event::PaneUpdate(pane_info) => {
                tracing::Span::current().record("event_type", "Event::PaneUpdate");
                tracing::debug!(pane_count = ?pane_info.panes.len());
                if self.state.panes == pane_info {
                    return false;
                }
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
                self.refresh_is_current_tab_plugin();
                self.state.cache_mask = UpdateEventMask::Tab as u8;

                self.resolve_pending_overrides();
                self.ensure_timer();

                should_render = true;
            }
            Event::PermissionRequestResult(result) => {
                tracing::Span::current().record("event_type", "Event::PermissionRequestResult");
                tracing::debug!(result = ?result);
                set_selectable(false);
            }
            Event::RunCommandResult(exit_code, stdout, stderr, context) => {
                tracing::Span::current().record("event_type", "Event::RunCommandResult");
                tracing::debug!(
                    exit_code = ?exit_code,
                    stdout = ?String::from_utf8(stdout.clone()),
                    stderr = ?String::from_utf8(stderr.clone()),
                    context = ?context
                );

                self.state.cache_mask = UpdateEventMask::Command as u8;

                if let Some(name) = context.get("name") {
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
                    should_render = self.state.is_current_tab_plugin;
                }
            }
            Event::SessionUpdate(_, _) => {
                // Temporarily disabled for CPU profiling.
                return false;
            }
            Event::TabUpdate(tab_info) => {
                tracing::Span::current().record("event_type", "Event::TabUpdate");
                tracing::debug!(tab_count = ?tab_info.len());

                if self.state.tabs == tab_info {
                    return false;
                }

                // Detect new tab and broadcast overrides to new instance
                let new_tab_count = tab_info.len();
                if new_tab_count > self.last_tab_count
                    && self.last_tab_count > 0
                    && self.state.is_current_tab_plugin
                {
                    self.broadcast_overrides();
                }
                self.last_tab_count = new_tab_count;

                self.state.cache_mask = UpdateEventMask::Tab as u8;
                self.state.tabs = tab_info;
                self.refresh_is_current_tab_plugin();

                // Rebuild overrides with correct tab positions
                self.rebuild_override_positions();

                self.resolve_pending_overrides();
                self.ensure_timer();

                should_render = true;
            }
            Event::Timer(_) => {
                let has_spin = self.has_spin();
                let should_poll_commands = self.should_poll_commands();

                if has_spin {
                    self.state.spinner_idx = self.state.spinner_idx.wrapping_add(1);
                    self.state.cache_mask = UpdateEventMask::Tab as u8;
                    set_timeout(0.3);
                    should_render = true;
                } else if should_poll_commands {
                    self.state.cache_mask = UpdateEventMask::Command as u8;
                    set_timeout(self.command_poll_interval_secs);
                    should_render = true;
                } else {
                    self.timer_active = false;
                }
            }
            _ => (),
        };
        should_render
    }

    fn refresh_is_current_tab_plugin(&mut self) {
        self.state.is_current_tab_plugin = self.is_current_tab_plugin();
    }

    fn is_current_tab_plugin(&self) -> bool {
        let active_tab_pos = match self.state.tabs.iter().find(|t| t.active) {
            Some(tab) => tab.position,
            None => return false,
        };
        let plugin_id = get_plugin_ids().plugin_id;
        let panes = match self.state.panes.panes.get(&active_tab_pos) {
            Some(panes) => panes,
            None => return false,
        };

        panes.iter().any(|p| p.is_plugin && p.id == plugin_id)
    }

    fn has_spin(&self) -> bool {
        self.state
            .tab_name_overrides
            .values()
            .flat_map(|m| m.values())
            .any(|v| v.contains("{spin}"))
    }

    fn should_poll_commands(&self) -> bool {
        self.has_command_widgets && self.state.is_current_tab_plugin
    }

    fn ensure_timer(&mut self) {
        if self.timer_active {
            return;
        }
        if self.has_spin() {
            self.timer_active = true;
            set_timeout(0.3);
        } else if self.should_poll_commands() {
            self.timer_active = true;
            set_timeout(self.command_poll_interval_secs);
        }
    }

    fn rebuild_override_positions(&mut self) {
        let mut new_overrides: BTreeMap<usize, BTreeMap<u32, String>> = BTreeMap::new();
        for (_, inner) in std::mem::take(&mut self.state.tab_name_overrides) {
            for (pane_id, value) in inner {
                if let Some(new_pos) = self.find_tab_for_pane(pane_id) {
                    new_overrides
                        .entry(new_pos)
                        .or_default()
                        .insert(pane_id, value);
                }
            }
        }
        self.state.tab_name_overrides = new_overrides;

        let mut new_fallbacks: BTreeMap<usize, BTreeMap<u32, String>> = BTreeMap::new();
        for (_, inner) in std::mem::take(&mut self.state.tab_name_fallbacks) {
            for (pane_id, value) in inner {
                if let Some(new_pos) = self.find_tab_for_pane(pane_id) {
                    new_fallbacks
                        .entry(new_pos)
                        .or_default()
                        .insert(pane_id, value);
                }
            }
        }
        self.state.tab_name_fallbacks = new_fallbacks;
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
                    self.state
                        .tab_name_overrides
                        .entry(pos)
                        .or_default()
                        .insert(pane_id, payload);
                    if let Some(fb) = fallback {
                        self.state
                            .tab_name_fallbacks
                            .entry(pos)
                            .or_default()
                            .insert(pane_id, fb);
                    }
                }
            } else {
                self.pending_pipe_overrides
                    .push((pane_id, payload, fallback));
            }
        }
    }

    fn broadcast_overrides(&self) {
        let session = match &self.state.mode.session_name {
            Some(s) => s.clone(),
            None => return,
        };
        if self.state.tab_name_overrides.is_empty() {
            return;
        }
        for (_tab_pos, inner) in &self.state.tab_name_overrides {
            for (pane_id, payload) in inner {
                let fallback = self
                    .state
                    .tab_name_fallbacks
                    .values()
                    .flat_map(|m| m.get(pane_id))
                    .next()
                    .cloned()
                    .unwrap_or_default();
                let mut args = BTreeMap::new();
                args.insert("pane_id".to_string(), pane_id.to_string());
                args.insert("session".to_string(), session.clone());
                if !fallback.is_empty() {
                    args.insert("fallback".to_string(), fallback);
                }
                pipe_message_to_plugin(
                    MessageToPlugin::new("title")
                        .with_payload(payload)
                        .with_args(args),
                );
            }
        }
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
}

fn command_poll_interval_secs_from_config(config: &BTreeMap<String, String>) -> f64 {
    let min_interval = config
        .iter()
        .filter(|(key, _)| key.starts_with("command_") && key.ends_with("_interval"))
        .filter_map(|(_, value)| value.parse::<i64>().ok())
        .filter(|value| *value > 0)
        .min()
        .unwrap_or(1);

    min_interval.max(1) as f64
}

fn sanitize_pipe_title_payload(payload: String) -> String {
    payload
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

    tracing::debug!("registered widgets: {:?}", widget_map.keys());

    widget_map
}
