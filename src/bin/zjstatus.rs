use zellij_tile::prelude::*;

use chrono::Local;
use std::{collections::BTreeMap, sync::Arc};
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
    pending_pipe_overrides: Vec<(u32, String, Option<String>)>,
    synced_from_host: bool,
}

#[cfg(not(test))]
register_plugin!(State);

#[cfg(feature = "tracing")]
fn init_tracing() {
    use std::fs::File;
    use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

    let file = File::create("/host/.zjstatus.log");
    let file = match file {
        Ok(file) => file,
        Err(error) => panic!("Error: {:?}", error),
    };
    let debug_log = tracing_subscriber::fmt::layer().with_writer(Arc::new(file));

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
        ]);

        subscribe(&[
            EventType::Mouse,
            EventType::ModeUpdate,
            EventType::PaneUpdate,
            EventType::PermissionRequestResult,
            EventType::TabUpdate,
            EventType::SessionUpdate,
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
    }

    fn pipe(&mut self, pipe_message: PipeMessage) -> bool {
        // Handle "title" pipe for tab naming
        if pipe_message.name == "title" {
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

                if let Some(pos) = tab_pos {
                    let id = pane_id.unwrap();
                    if payload.is_empty() {
                        if let Some(inner) = self.state.tab_name_overrides.get_mut(&pos) {
                            inner.remove(&id);
                            if inner.is_empty() {
                                self.state.tab_name_overrides.remove(&pos);
                            }
                        }
                        if let Some(inner) = self.state.tab_name_fallbacks.get_mut(&pos) {
                            inner.remove(&id);
                            if inner.is_empty() {
                                self.state.tab_name_fallbacks.remove(&pos);
                            }
                        }
                    } else {
                        self.state.tab_name_overrides.entry(pos).or_default().insert(id, payload);
                        if let Some(fb) = pipe_message.args.get("fallback") {
                            self.state.tab_name_fallbacks.entry(pos).or_default().insert(id, fb.clone());
                        }
                        self.ensure_timer();
                    }
                    self.state.cache_mask = UpdateEventMask::Tab as u8;
                    return true;
                }
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

        self.ensure_timer();

        tracing::debug!("{:?}", self.state.mode.session_name);

        let output = self
            .module_config
            .render_bar(self.state.clone(), self.widget_map.clone());

        print!("{}", output);
    }
}

impl State {
    fn ensure_timer(&mut self) {
        if !self.timer_active
            && self
                .state
                .tab_name_overrides
                .values()
                .flat_map(|m| m.values())
                .any(|v| v.contains("{spin}"))
        {
            self.timer_active = true;
            set_timeout(0.3);
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

    fn maybe_sync_from_host(&mut self) {
        if !self.synced_from_host
            && !self.state.tabs.is_empty()
            && !self.state.panes.panes.is_empty()
        {
            self.synced_from_host = true;
            run_command(
                &[
                    "sh",
                    "-c",
                    "for f in /tmp/zjstatus-pane-*; do [ -f \"$f\" ] && id=\"${f##*-}\" && printf '%s:%s\\n' \"$id\" \"$(cat \"$f\")\"; done",
                ],
                BTreeMap::from([("name".to_string(), "sync_overrides".to_string())]),
            );
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

    fn handle_event(&mut self, event: Event) -> bool {
        let mut should_render = false;
        match event {
            Event::Mouse(mouse_info) => {
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
                self.state.cache_mask = UpdateEventMask::Tab as u8;

                self.resolve_pending_overrides();
                self.maybe_sync_from_host();
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

                if let Some(name) = context.get("name") {
                    if name == "sync_overrides" {
                        let stdout = String::from_utf8(stdout).unwrap_or_default();
                        for line in stdout.lines() {
                            if let Some((id_str, payload)) = line.split_once(':') {
                                if let Ok(pane_id) = id_str.parse::<u32>() {
                                    if let Some(pos) = self.find_tab_for_pane(pane_id) {
                                        let inner = self.state.tab_name_overrides.entry(pos).or_default();
                                        if !inner.contains_key(&pane_id)
                                            && !payload.is_empty()
                                        {
                                            inner.insert(pane_id, payload.to_string());
                                        }
                                    }
                                }
                            }
                        }
                        self.ensure_timer();
                        self.state.cache_mask = UpdateEventMask::Tab as u8;
                        should_render = true;
                    } else {
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
                    }
                }
            }
            Event::SessionUpdate(session_info, _) => {
                tracing::Span::current().record("event_type", "Event::SessionUpdate");

                let current_session = session_info.iter().find(|s| s.is_current_session);

                if let Some(current_session) = current_session {
                    frames::hide_frames_conditionally(
                        &frames::FrameConfig::new(
                            self.module_config.hide_frame_for_single_pane,
                            self.module_config.hide_frame_except_for_search,
                            self.module_config.hide_frame_except_for_fullscreen,
                            self.module_config.hide_frame_except_for_scroll,
                        ),
                        &current_session.tabs,
                        &current_session.panes,
                        &self.state.mode,
                        get_plugin_ids(),
                        false,
                    );
                }

                self.state.sessions = session_info;
                self.state.cache_mask = UpdateEventMask::Session as u8;

                should_render = true;
            }
            Event::TabUpdate(tab_info) => {
                tracing::Span::current().record("event_type", "Event::TabUpdate");
                tracing::debug!(tab_count = ?tab_info.len());

                self.state.cache_mask = UpdateEventMask::Tab as u8;
                self.state.tabs = tab_info;

                self.resolve_pending_overrides();
                self.maybe_sync_from_host();
                self.ensure_timer();

                should_render = true;
            }
            Event::Timer(_) => {
                tracing::Span::current().record("event_type", "Event::Timer");

                self.state.spinner_idx = self.state.spinner_idx.wrapping_add(1);
                self.state.cache_mask = UpdateEventMask::Tab as u8;

                // Keep the timer running while any override contains {spin}
                let has_spin = self
                    .state
                    .tab_name_overrides
                    .values()
                    .flat_map(|m| m.values())
                    .any(|v| v.contains("{spin}"));
                if has_spin {
                    set_timeout(0.3);
                } else {
                    self.timer_active = false;
                }
                should_render = true;
            }
            _ => (),
        };
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

    tracing::debug!("registered widgets: {:?}", widget_map.keys());

    widget_map
}
