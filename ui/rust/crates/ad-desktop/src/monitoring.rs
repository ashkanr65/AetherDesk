//! Connection-scoped, demand-driven monitoring. All commands use separate SSH
//! channels and owned process groups; results never enter the interactive PTY.
use crate::{
    connection::Connections,
    monitoring_data::{self as data, Data, Filters, Format, Kind, Monitor},
    ui_icons::{self, Icon},
};
use eframe::egui;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
pub struct Target {
    pub profile: String,
    pub label: String,
    pub host: String,
    pub account: String,
    pub session: Option<Arc<ad_ssh::SshSession>>,
}
impl Target {
    fn identity(&self) -> String {
        self.session
            .as_ref()
            .map(|s| format!("{:?}", s.connection_id()))
            .unwrap_or_else(|| format!("offline:{}", self.profile))
    }
    fn connected(&self) -> bool {
        self.session.as_ref().is_some_and(|s| !s.is_closed())
    }
}
fn all_metrics() -> [bool; 6] {
    [true; 6]
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Store {
    schema: u32,
    pinned: bool, // Accepted for older settings; the bar is now always visible.
    #[serde(default = "all_metrics")]
    visibility: [bool; 6],
    monitors: Vec<Monitor>,
}
impl Default for Store {
    fn default() -> Self {
        Self {
            schema: 1,
            pinned: true,
            visibility: all_metrics(),
            monitors: vec![],
        }
    }
}
impl Store {
    fn validate(&self) -> Result<(), String> {
        if self.schema != 1 || self.monitors.len() > 64 {
            return Err(
                "Unsupported or oversized monitoring settings; original file preserved.".into(),
            );
        }
        let mut ids = BTreeSet::new();
        for monitor in &self.monitors {
            monitor.validate()?;
            if !ids.insert(&monitor.id) {
                return Err("Duplicate monitor identifier; original file preserved.".into());
            }
        }
        Ok(())
    }
    fn load() -> Result<Self, String> {
        let value = crate::storage::read_json("monitoring.json")?;
        let store: Self = match value {
            Some(v) => serde_json::from_value(v)
                .map_err(|_| "Monitoring settings are damaged; original file preserved.")?,
            None => Self::default(),
        };
        store.validate()?;
        Ok(store)
    }
    fn save(&self) -> Result<(), String> {
        self.validate()?;
        crate::storage::write_json(
            "monitoring.json",
            &serde_json::to_value(self).map_err(|e| e.to_string())?,
        )
    }
}
struct ResultMessage {
    result: Result<Data, String>,
    duration: Duration,
    raw: Option<String>,
}
struct Running {
    cancel: CancellationToken,
    result: mpsc::Receiver<ResultMessage>,
}
impl Drop for Running {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}
#[derive(Default)]
struct Sample {
    value: Option<Data>,
    raw: Option<String>,
    error: Option<String>,
    received: Option<Instant>,
    finished: Option<Instant>,
    duration: Option<Duration>,
    running: Option<Running>,
}
impl Sample {
    fn due(&self, interval: Duration) -> bool {
        self.running.is_none() && self.finished.is_none_or(|t| t.elapsed() >= interval)
    }
    fn stale(&self, interval: u32) -> bool {
        self.error.is_some()
            || self
                .received
                .is_none_or(|t| t.elapsed() > Duration::from_secs(u64::from(interval) * 2 + 12))
    }
}
struct Editor {
    monitor: Monitor,
    identity: String,
    error: Option<String>,
}
#[derive(Default)]
pub struct Panel {
    loaded: bool,
    store: Store,
    storage_error: Option<String>,
    target: Option<Target>,
    identity: Option<String>,
    samples: BTreeMap<String, Sample>,
    editor: Option<Editor>,
    details: Option<Monitor>,
    add_open: bool,
    home_scan_requested: bool,
    previous_fast: Option<(data::System, Instant)>,
    cpu: Option<f32>,
    network: BTreeMap<String, (f64, f64)>,
}
impl Panel {
    fn switch_identity(&mut self, identity: Option<String>) {
        if self.identity != identity {
            self.stop();
            self.samples.clear();
            self.previous_fast = None;
            self.cpu = None;
            self.network.clear();
            self.editor = None;
            self.details = None;
            self.add_open = false;
            self.home_scan_requested = false;
            self.identity = identity;
        }
    }
    pub fn stop(&mut self) {
        for sample in self.samples.values_mut() {
            sample.running = None;
        }
    }
    pub fn tick(
        &mut self,
        ctx: &egui::Context,
        runtime: &Connections,
        target: Option<Target>,
        visible: bool,
    ) {
        if !self.loaded {
            self.loaded = true;
            match Store::load() {
                Ok(store) => self.store = store,
                Err(error) => self.storage_error = Some(error),
            }
        }
        let identity = target.as_ref().map(Target::identity);
        self.switch_identity(identity);
        self.target = target;
        let connected = self.target.as_ref().is_some_and(Target::connected);
        if !connected {
            self.stop();
            if !connected {
                self.samples.clear();
                self.previous_fast = None;
                self.cpu = None;
                self.network.clear();
            }
            return;
        }
        for (key, sample) in &mut self.samples {
            let reply = sample.running.as_mut().map(|r| r.result.try_recv());
            let reply = match reply {
                Some(Ok(reply)) => Some(reply),
                Some(Err(mpsc::error::TryRecvError::Disconnected)) => Some(ResultMessage {
                    result: Err("Monitoring worker stopped.".into()),
                    duration: Duration::ZERO,
                    raw: None,
                }),
                _ => None,
            };
            if let Some(reply) = reply {
                sample.running = None;
                sample.finished = Some(Instant::now());
                sample.duration = Some(reply.duration);
                sample.raw = reply.raw;
                match reply.result {
                    Ok(value) => {
                        if key == "system-fast" {
                            if let Data::System(current) = &value {
                                let now = Instant::now();
                                self.cpu = self
                                    .previous_fast
                                    .as_ref()
                                    .and_then(|(p, _)| p.cpu.zip(current.cpu))
                                    .and_then(|(p, c)| data::cpu_percent(p, c));
                                self.network.clear();
                                if let Some((previous, time)) = &self.previous_fast {
                                    let seconds = now.duration_since(*time).as_secs_f64();
                                    if seconds > 0.0 {
                                        for (name, (rx, tx)) in &current.network {
                                            if let Some((old_rx, old_tx)) =
                                                previous.network.get(name)
                                            {
                                                if let (Some(rx), Some(tx)) = (
                                                    rx.checked_sub(*old_rx),
                                                    tx.checked_sub(*old_tx),
                                                ) {
                                                    self.network.insert(
                                                        name.clone(),
                                                        (rx as f64 / seconds, tx as f64 / seconds),
                                                    );
                                                }
                                            }
                                        }
                                    }
                                }
                                self.previous_fast = Some((current.clone(), now));
                            }
                        }
                        sample.value = Some(value);
                        sample.received = Some(Instant::now());
                        sample.error = None;
                    }
                    Err(error) => {
                        sample.error = Some(error);
                        if key == "system-fast" {
                            self.previous_fast = None;
                        }
                    }
                }
            }
        }
        let mut requests = vec![
            (
                "system-fast".into(),
                data::system_command(false),
                Source::System(false),
                3,
            ),
            (
                "system-slow".into(),
                data::system_command(true),
                Source::System(true),
                30,
            ),
        ];
        {
            if let Some(target) = &self.target {
                // Identical sources share one report; filtering never launches a job.
                let mut sources: BTreeMap<String, (&Monitor, u32)> = BTreeMap::new();
                for monitor in self.store.monitors.iter().filter(|m| {
                    m.enabled
                        && m.profile == target.profile
                        && (visible
                            || m.show_in_bar
                            || self.details.as_ref().is_some_and(|d| d.id == m.id))
                }) {
                    sources
                        .entry(monitor.key())
                        .and_modify(|(_, interval)| *interval = (*interval).min(monitor.interval))
                        .or_insert((monitor, monitor.interval));
                }
                for (key, (monitor, interval)) in sources {
                    if let Ok(command) = monitor.command() {
                        requests.push((
                            key,
                            command,
                            Source::Monitor(monitor.kind.clone()),
                            interval,
                        ));
                    }
                }
            }
        }
        let live: BTreeSet<_> = requests
            .iter()
            .map(|(key, _, _, _)| key.clone())
            .chain(self.editor.as_ref().map(|_| "test".into()))
            .chain(std::iter::once("home-usage".into()))
            .collect();
        self.samples.retain(|key, _| live.contains(key));
        for (key, command, source, interval) in requests {
            if self
                .samples
                .values()
                .filter(|s| s.running.is_some())
                .count()
                >= 2
            {
                break;
            }
            if self
                .samples
                .entry(key.clone())
                .or_default()
                .due(Duration::from_secs(interval.into()))
            {
                self.start(key, command, source, runtime, ctx);
            }
        }
        if self.home_scan_requested
            && self
                .samples
                .values()
                .filter(|s| s.running.is_some())
                .count()
                < 2
        {
            self.home_scan_requested = false;
            if self
                .samples
                .get("home-usage")
                .is_none_or(|s| s.running.is_none())
            {
                self.start(
                    "home-usage".into(),
                    data::homes_command(),
                    Source::System(true),
                    runtime,
                    ctx,
                );
            }
        }
        ctx.request_repaint_after(Duration::from_secs(1));
    }
    fn start(
        &mut self,
        key: String,
        command: String,
        source: Source,
        runtime: &Connections,
        ctx: &egui::Context,
    ) {
        let Some(session) = self.target.as_ref().and_then(|t| t.session.clone()) else {
            return;
        };
        let cancel = CancellationToken::new();
        let worker_cancel = cancel.clone();
        let (send, result) = mpsc::channel(1);
        let repaint = ctx.clone();
        runtime.spawn(async move {
            let start=Instant::now();
            let output=tokio::select! {
                _=worker_cancel.cancelled()=>return,
                output=session.execute_scoped_command_output(&command,Duration::from_secs(12))=>output,
            };
            let mut raw=None;
            let result=output.map_err(|_|"Check failed or exceeded its 12-second timeout. The connection may be unavailable.".to_owned()).and_then(|output| {
                raw=Some(format!("{}\n{}",output.stdout_text().chars().take(30000).collect::<String>(),output.stderr_text().chars().take(2048).collect::<String>()));
                if output.stdout.len()+output.stderr.len()>data::MAX_OUTPUT {return Err("Monitor output exceeds 256 KiB; narrow the report.".into());}
                if output.exit_status!=0 {return Err(format!("Check exited with status {}. {}",output.exit_status,output.stderr_text().chars().take(1000).collect::<String>()));}
                let text=String::from_utf8(output.stdout).map_err(|_|"Monitor output must be UTF-8 text.")?;
                match source {
                    Source::System(slow)=>data::parse_system(&text,slow).map(Data::System),
                    Source::Monitor(Kind::FlexNet{..})=>data::parse_licences(&text).map(Data::Licences),
                    Source::Monitor(Kind::Custom{format,..})=>data::parse_custom(&text,&format),
                }
            });
            let _=send.send(ResultMessage {result,duration:start.elapsed(),raw}).await;
            repaint.request_repaint();
        });
        self.samples.entry(key).or_default().running = Some(Running { cancel, result });
    }
    fn persist(&mut self, store: Store) -> bool {
        if self.storage_error.is_some() {
            return false;
        }
        match store.save() {
            Ok(()) => {
                self.store = store;
                if let Some(id) = self.details.as_ref().map(|m| m.id.clone()) {
                    self.details = self.store.monitors.iter().find(|m| m.id == id).cloned();
                }
                true
            }
            Err(error) => {
                self.storage_error = Some(error);
                false
            }
        }
    }
    pub fn compact(&mut self, ui: &mut egui::Ui) -> bool {
        let connected = self.target.as_ref().is_some_and(Target::connected);
        let fast = self.system(false);
        let slow = self.system(true);
        let fast_stale = self.samples.get("system-fast").is_none_or(|s| s.stale(3));
        let slow_stale = self.samples.get("system-slow").is_none_or(|s| s.stale(30));
        let state = |stale: bool| {
            if !connected {
                "Disconnected"
            } else if stale {
                "Stale or awaiting a reading"
            } else {
                "Current reading"
            }
        };
        let mut items: Vec<(Icon, String, String, f32, Option<Monitor>)> = vec![];
        if self.store.visibility[0] {
            items.push((
                Icon::Cpu,
                self.cpu
                    .map(|c| format!("{c:.0}%"))
                    .unwrap_or_else(|| "—".into()),
                format!("CPU usage · {}", state(fast_stale)),
                80.0,
                None,
            ));
        }
        if self.store.visibility[1] {
            let ram = fast
                .and_then(|s| s.memory)
                .map(|(t, a, _, _)| {
                    format!(
                        "{:.1} / {:.1} GiB",
                        (t - a) as f64 / 1048576.0,
                        t as f64 / 1048576.0
                    )
                })
                .unwrap_or_else(|| "— / — GiB".into());
            items.push((
                Icon::Memory,
                ram,
                format!("RAM used / total · {}", state(fast_stale)),
                170.0,
                None,
            ));
        }
        if self.store.visibility[2] {
            items.push((
                Icon::Uptime,
                fast.and_then(|s| s.uptime)
                    .map(uptime)
                    .unwrap_or_else(|| "—".into()),
                format!("Server uptime · {}", state(fast_stale)),
                145.0,
                None,
            ));
        }
        if self.store.visibility[3] {
            let (rx, tx) = self
                .network
                .values()
                .fold((0.0, 0.0), |(a, b), (r, t)| (a + r, b + t));
            for (icon, value, label) in [
                (Icon::Upload, tx, "Server upload"),
                (Icon::Download, rx, "Server download"),
            ] {
                items.push((icon,if self.network.is_empty() {"—".into()} else {rate(value)},format!("{label} · {}\nSum of non-loopback interface counters; bridges or virtual interfaces may count traffic more than once.",state(fast_stale)),110.0,None));
            }
        }
        if self.store.visibility[4] {
            let users = slow
                .filter(|s| s.users_error.is_none())
                .map(|s| account_summary(&s.users))
                .unwrap_or_else(|| "—".into());
            items.push((Icon::Users,users.clone(),format!("Recorded login sessions: {users} · {}\nA suffix counts sessions, not active application use.",state(slow_stale)),160.0,None));
        }
        if self.store.visibility[5] {
            let disk = slow
                .and_then(|s| s.disks.first())
                .map(|(mount, _, used, free)| {
                    format!(
                        "{mount}: {:.0}%",
                        100.0 * *used as f64 / (*used as f64 + *free as f64).max(1.0)
                    )
                })
                .unwrap_or_else(|| "—".into());
            let home = slow.map(|s| s.home.as_str()).unwrap_or("");
            items.push((Icon::Storage,disk,format!("Filesystem containing {home} · {}\nOpen Monitoring for capacity and per-user home-folder usage.",state(slow_stale)),145.0,None));
        }
        if self.store.visibility[5] {
            if let Some(Data::System(homes)) = self
                .samples
                .get("home-usage")
                .and_then(|s| s.value.as_ref())
            {
                for home in &homes.homes {
                    let size = home
                        .kib
                        .map(|n| {
                            format!(
                                "{}{:.1} GiB",
                                if homes.homes_error.is_some() {
                                    "≥"
                                } else {
                                    ""
                                },
                                n as f64 / 1048576.0
                            )
                        })
                        .unwrap_or_else(|| "?".into());
                    items.push((Icon::Storage,format!("{}: {size}",home.owner),format!("{}\nManual home-folder scan; shared contents are not per-user quotas.\n{}",home.path,homes.homes_error.as_deref().unwrap_or("Cached until you run Scan home folders again.")),145.0,None));
                }
            }
        }
        if let Some(target) = &self.target {
            for monitor in self
                .store
                .monitors
                .iter()
                .filter(|m| m.profile == target.profile && m.enabled && m.show_in_bar)
            {
                let sample = self.samples.get(&monitor.key());
                let text = sample
                    .and_then(|s| s.value.as_ref())
                    .map(|v| compact_value(v, &monitor.filters))
                    .unwrap_or_else(|| "—".into());
                let stale = sample.is_none_or(|s| s.stale(monitor.interval));
                items.push((
                    if matches!(monitor.kind, Kind::FlexNet { .. }) {
                        Icon::SecureKeys
                    } else {
                        Icon::TerminalSettings
                    },
                    text,
                    format!("{} · {}\nClick for details", monitor.name, state(stale)),
                    180.0,
                    Some(monitor.clone()),
                ));
            }
        }
        let height = ui.available_height().max(28.0);
        let width = (ui.available_width() - height).max(1.0);
        let mut open_panel = false;
        ui.spacing_mut().item_spacing = egui::Vec2::ZERO;
        ui.horizontal(|ui| {
            ui.allocate_ui_with_layout(
                egui::vec2(width, height),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    egui::ScrollArea::horizontal()
                        .id_salt("monitoring-compact-scroll")
                        .max_width(width)
                        .max_height(height)
                        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                for (icon, text, tooltip, width, monitor) in items {
                                    if metric_segment(ui, icon, &text, &tooltip, width, height)
                                        .clicked()
                                    {
                                        if let Some(monitor) = monitor {
                                            self.details = Some(monitor);
                                        } else {
                                            open_panel = true;
                                        }
                                    }
                                }
                            });
                        });
                },
            );
            if metric_segment(ui, Icon::NewChat, "", "Add monitor", height, height).clicked() {
                self.add_open = true;
            }
        });
        open_panel
    }
    fn system(&self, slow: bool) -> Option<&data::System> {
        match self
            .samples
            .get(if slow { "system-slow" } else { "system-fast" })?
            .value
            .as_ref()?
        {
            Data::System(s) => Some(s),
            _ => None,
        }
    }
    fn edit(&mut self, monitor: Monitor) {
        self.samples.remove("test");
        self.editor = Some(Editor {
            monitor,
            identity: self.identity.clone().unwrap_or_default(),
            error: None,
        });
    }
    pub fn ui(&mut self, ui: &mut egui::Ui) {
        let width = ui.available_width();
        ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
            ui.set_width(width);
            ui.strong("Monitoring");
            let Some(target) = self.target.clone() else {
                ui.label("Open a connection to monitor its server.");
                return;
            };
            ui.add(
                egui::Label::new(format!(
                    "{}\n{}@{}",
                    target.label, target.account, target.host
                ))
                .wrap(),
            );
            if let Some(error) = &self.storage_error {
                ui.colored_label(ui.visuals().error_fg_color, error);
            }
            ui.small("Choose which readings appear in the status bar.");
            egui::ScrollArea::vertical()
                .id_salt("monitor-cards")
                .auto_shrink([false, false])
                .max_width(width)
                .show(ui, |ui| {
                    ui.set_width(width);
                    self.system_cards(ui, target.connected());
                    let monitors: Vec<_> = self
                        .store
                        .monitors
                        .iter()
                        .filter(|m| m.profile == target.profile)
                        .cloned()
                        .collect();
                    let mut remove = None;
                    for mut monitor in monitors {
                        let mut edit = false;
                        let mut refresh = false;
                        let mut enabled = monitor.enabled;
                        let mut show_in_bar = monitor.show_in_bar;
                        ui.push_id(&monitor.id, |ui| {
                            card(ui, &monitor.name, |ui| {
                                ui.checkbox(&mut enabled, "Enabled");
                                ui.checkbox(&mut show_in_bar, "Show in status bar");
                                ui.horizontal_wrapped(|ui| {
                                    edit |=
                                        ui_icons::button(ui, Icon::Settings, "Edit monitor", false)
                                            .clicked();
                                    edit |=
                                        ui_icons::button(ui, Icon::Filter, "Filter results", false)
                                            .clicked();
                                    if ui_icons::button(ui, Icon::Remove, "Remove monitor", false)
                                        .clicked()
                                    {
                                        remove = Some(monitor.id.clone());
                                    }
                                    refresh = ui
                                        .add_enabled(
                                            target.connected() && enabled,
                                            egui::Button::new("Refresh"),
                                        )
                                        .clicked();
                                });
                                let interval = monitor.interval;
                                if let Some(sample) = self.samples.get(&monitor.key()) {
                                    sample_status(
                                        ui,
                                        sample,
                                        interval,
                                        target.connected() && enabled,
                                    );
                                    if let Some(value) = &sample.value {
                                        render_data(ui, value, &monitor.filters, 6);
                                    }
                                } else {
                                    ui.label(if enabled {
                                        "Waiting for a reading…"
                                    } else {
                                        "Disabled"
                                    });
                                }
                                if ui.button("Show details").clicked() {
                                    self.details = Some(monitor.clone());
                                }
                            });
                        });
                        if monitor.enabled != enabled || monitor.show_in_bar != show_in_bar {
                            monitor.show_in_bar = show_in_bar;
                            monitor.enabled = enabled;
                            let mut store = self.store.clone();
                            if let Some(old) =
                                store.monitors.iter_mut().find(|m| m.id == monitor.id)
                            {
                                *old = monitor.clone();
                            }
                            self.persist(store);
                        }
                        if refresh {
                            if let Some(sample) = self.samples.get_mut(&monitor.key()) {
                                sample.finished = None;
                            }
                        }
                        if edit {
                            self.edit(monitor);
                        }
                    }
                    if let Some(id) = remove {
                        let mut store = self.store.clone();
                        store.monitors.retain(|m| m.id != id);
                        self.persist(store);
                    }
                    ui.add_space(6.0);
                    ui.add_enabled_ui(
                        self.storage_error.is_none() && self.store.monitors.len() < 64,
                        |ui| {
                            if ui.button("+ Add monitor").clicked() {
                                self.add_open = true;
                            }
                        },
                    );
                    ui.add_space(8.0);
                });
        });
    }
    fn system_cards(&mut self, ui: &mut egui::Ui, connected: bool) {
        let mut visibility = self.store.visibility;
        let fast = self.system(false).cloned();
        let slow = self.system(true).cloned();
        card(ui, "Server connection", |ui| {
            ui.label(if connected {
                "SSH connected"
            } else {
                "Disconnected"
            });
            if let Some(host) = fast
                .as_ref()
                .map(|s| s.host.as_str())
                .filter(|h| !h.is_empty())
            {
                ui.label(format!("Hostname: {host}"));
            }
            if let Some(sample) = self.samples.get("system-fast") {
                sample_status(ui, sample, 3, connected);
                if let Some(duration) = sample.duration {
                    ui.label(format!("Check response: {} ms", duration.as_millis()))
                        .on_hover_text(
                            "Command execution plus SSH transport time; not a pure network ping.",
                        );
                }
            }
            if ui
                .add_enabled(connected, egui::Button::new("Refresh system"))
                .clicked()
            {
                for key in ["system-fast", "system-slow"] {
                    if let Some(s) = self.samples.get_mut(key) {
                        s.finished = None;
                    }
                }
            }
        });
        visibility_card(ui, "CPU", &mut visibility[0], |ui| {
            if let Some(cpu) = self.cpu {
                ui.add(egui::ProgressBar::new(cpu / 100.0).text(format!("{cpu:.1}%")));
            } else {
                ui.label("Waiting for two samples…");
            }
        });
        visibility_card(ui, "Memory", &mut visibility[1], |ui| {
            if let Some((total, available, swap, free)) = fast.as_ref().and_then(|s| s.memory) {
                ui.label(format!(
                    "{:.2} / {:.2} GiB used",
                    (total - available) as f64 / 1048576.0,
                    total as f64 / 1048576.0
                ));
                ui.add(egui::ProgressBar::new(
                    1.0 - available as f32 / total as f32,
                ));
                ui.label(format!(
                    "Swap: {:.2} / {:.2} GiB",
                    (swap - free) as f64 / 1048576.0,
                    swap as f64 / 1048576.0
                ));
            } else {
                ui.label("Unavailable");
            }
        });
        visibility_card(ui, "Recorded logins", &mut visibility[4], |ui| {
            if let Some(slow) = &slow {
                if let Some(error) = &slow.users_error {
                    ui.label(error);
                } else {
                    ui.add(egui::Label::new(account_summary(&slow.users)).wrap());
                    ui.small(
                        "Login records do not show every GUI/background job or prove active use.",
                    );
                }
            } else {
                ui.label("Waiting for a reading…");
            }
            if let Some(s) = self.samples.get("system-slow") {
                sample_status(ui, s, 30, connected);
            }
        });
        visibility_card(ui, "Storage", &mut visibility[5], |ui| {
            if let Some(slow) = &slow {
                if let Some(error) = &slow.disks_error {
                    ui.label(error);
                }
                for (mount, _, used, available) in &slow.disks {
                    let fraction = *used as f64 / ((*used as f64 + *available as f64).max(1.0));
                    let label = format!("{mount} · {:.0}%", fraction * 100.0);
                    if fraction >= 0.9 {
                        ui.colored_label(ui.visuals().warn_fg_color, label);
                    } else {
                        ui.label(label);
                    }
                    ui.small(format!(
                        "{:.1} GiB available",
                        *available as f64 / 1048576.0
                    ));
                }
            } else {
                ui.label("Waiting for a reading…");
            }
            ui.separator();
            ui.strong("Home folders by owner");
            ui.small("Manual metadata scan · up to 8 seconds · your account's permissions only. Shared folders are not per-user quotas.");
            let running = self.home_scan_requested
                || self
                    .samples
                    .get("home-usage")
                    .is_some_and(|s| s.running.is_some());
            if ui
                .add_enabled(
                    connected && !running,
                    egui::Button::new("Scan home folders"),
                )
                .clicked()
            {
                self.home_scan_requested = true;
            }
            if let Some(sample) = self.samples.get("home-usage") {
                sample_status(ui, sample, 3600, connected);
                if let Some(Data::System(homes)) = &sample.value {
                    for home in &homes.homes {
                        let size = home
                            .kib
                            .map(|n| {
                                format!(
                                    "{}{:.2} GiB",
                                    if homes.homes_error.is_some() {
                                        "≥ "
                                    } else {
                                        ""
                                    },
                                    n as f64 / 1048576.0
                                )
                            })
                            .unwrap_or_else(|| "Unknown / unreadable".into());
                        ui.add(egui::Label::new(format!("{} · {size}", home_label(home))).wrap())
                            .on_hover_text(&home.path);
                    }
                    if let Some(error) = &homes.homes_error {
                        ui.add(egui::Label::new(error).wrap());
                    }
                    if homes.homes.is_empty() {
                        ui.label("No home folders reported under /home.");
                    }
                }
            }
            if let Some(sample) = self.samples.get("system-slow") {
                sample_status(ui, sample, 30, connected);
            }
        });
        visibility_card(ui, "Network", &mut visibility[3], |ui| {
            ui.small("Server interfaces · receive / send");
            if self.network.is_empty() {
                ui.label("Waiting for two samples…");
            }
            for (name, (rx, tx)) in &self.network {
                ui.label(name);
                ui.label(format!("↓ {} / ↑ {}", rate(*rx), rate(*tx)));
            }
        });
        visibility_card(ui, "Uptime", &mut visibility[2], |ui| {
            if let Some(seconds) = fast.as_ref().and_then(|s| s.uptime) {
                ui.label(format!(
                    "{} days · {:02}:{:02}:{:02}",
                    seconds / 86400,
                    seconds / 3600 % 24,
                    seconds / 60 % 60,
                    seconds % 60
                ));
            } else {
                ui.label("Unavailable");
            }
        });
        if visibility != self.store.visibility {
            let mut store = self.store.clone();
            store.visibility = visibility;
            self.persist(store);
        }
    }
    pub fn windows(&mut self, ctx: &egui::Context, runtime: &Connections) {
        if self.add_open {
            let mut open = true;
            let mut choice = None;
            egui::Window::new("Add monitor")
                .id(egui::Id::new("monitor-add"))
                .open(&mut open)
                .resizable(false)
                .collapsible(false)
                .fixed_size(dialog_size(ctx, egui::vec2(340.0, 150.0)))
                .show(ctx, |ui| {
                    ui.vertical(|ui| {
                        ui.label("Choose a check for this connection.");
                        let allowed = self.target.is_some()
                            && self.storage_error.is_none()
                            && self.store.monitors.len() < 64;
                        if ui
                            .add_enabled(
                                allowed,
                                egui::Button::new("Licence usage · FlexNet / Cadence"),
                            )
                            .clicked()
                        {
                            choice = Some(true);
                        }
                        if ui
                            .add_enabled(allowed, egui::Button::new("Custom command or script"))
                            .clicked()
                        {
                            choice = Some(false);
                        }
                        if self.target.is_none() {
                            ui.label("Open a connection first.");
                        }
                    });
                });
            self.add_open = open;
            if let Some(licence) = choice {
                if let Some(target) = &self.target {
                    self.edit(Monitor::new(target.profile.clone(), licence));
                }
                self.add_open = false;
            }
        }

        if let Some(mut editor) = self.editor.take() {
            let original_source = editor.monitor.key();
            let mut open = true;
            let mut save = false;
            let mut test = false;
            let mut cancel = false;
            egui::Window::new("Monitor settings").id(egui::Id::new("monitor-editor")).open(&mut open)
                .fixed_size(dialog_size(ctx,egui::vec2(600.0,620.0))).resizable(false).collapsible(false).show(ctx,|ui|{
                let compact=ui.available_width()<350.0;
                let footer=ui.spacing().interact_size.y*if compact {2.0}else{1.0}+20.0;
                let height=(ui.available_height()-footer).max(40.0);
                let mut multiline_focus=false;
                egui::ScrollArea::vertical().id_salt("monitor-editor-content").auto_shrink([false,false]).max_height(height).show(ui,|ui|{
                    ui.set_width(ui.available_width());
                    ui.strong("Connection");
                    if let Some(target)=&self.target {ui.label(format!("{} · {}@{}",target.label,target.account,target.host));}
                    ui.separator(); ui.strong("General");
                    field(ui,"Name",&mut editor.monitor.name);
                    ui.checkbox(&mut editor.monitor.enabled,"Enabled");
                    ui.checkbox(&mut editor.monitor.show_in_bar,"Show in status bar (keep collecting while panel is closed)");
                    ui.label("Refresh interval (seconds)");
                    let minimum=if matches!(editor.monitor.kind,Kind::FlexNet{..}){30}else{10};
                    ui.add(egui::DragValue::new(&mut editor.monitor.interval).range(minimum..=3600));
                    ui.separator(); ui.strong("Source");
                    match &mut editor.monitor.kind {
                        Kind::FlexNet {executable,server}=>{
                            field(ui,"lmutil or standalone lmstat executable",executable);
                            ui.label("Licence server or licence-file path");
                            ui.add(egui::TextEdit::singleline(server).hint_text("port@licence-server").desired_width(f32::INFINITY));
                            ui.small("Reports licence checkouts, which do not prove active application use.");
                        }
                        Kind::Custom {command,format}=>{
                            ui.label("Server command or shell script");
                            multiline_focus|=ui.add(egui::TextEdit::multiline(command).code_editor().desired_rows(5).desired_width(f32::INFINITY)).has_focus();
                            ui.small("Runs as your connected account. A custom script can change server files; use a read-only check.");
                            ui.label("Output format");
                            egui::ComboBox::from_id_salt("monitor-format").selected_text(match format {Format::Text=>"Text",Format::Value=>"Single value",Format::Table=>"JSON table"}).show_ui(ui,|ui|{
                                ui.selectable_value(format,Format::Text,"Text");ui.selectable_value(format,Format::Value,"Single value");ui.selectable_value(format,Format::Table,"JSON table");
                            });
                            if *format==Format::Table {ui.label("Return JSON: {\"columns\":[\"Tool\",\"User\"],\"rows\":[[\"Feature A\",\"user1\"]]}");}
                        }
                    }
                    ui.small("Checks stop after 12 seconds. Results never enter terminal history.");
                    ui.separator(); ui.strong("Filters and display");
                    if matches!(editor.monitor.kind,Kind::FlexNet{..}) {
                        ui.checkbox(&mut editor.monitor.filters.show_unused,"Include unused features");
                        field(ui,"Feature or tool",&mut editor.monitor.filters.feature);
                        field(ui,"Username",&mut editor.monitor.filters.user);
                        field(ui,"Machine using the licence",&mut editor.monitor.filters.machine);
                        ui.checkbox(&mut editor.monitor.filters.group_users,"Group by user");
                        ui.label("Friendly labels (one exact_feature=Label per line)");
                        multiline_focus|=ui.add(egui::TextEdit::multiline(&mut editor.monitor.filters.aliases).desired_rows(3).desired_width(f32::INFINITY)).has_focus();
                    } else {field(ui,"Show lines or rows containing",&mut editor.monitor.filters.search);}
                    if let Some(error)=&editor.error {ui.colored_label(ui.visuals().error_fg_color,error);}
                    if let Some(sample)=self.samples.get("test") {
                        ui.separator();ui.strong("Test result");sample_status(ui,sample,60,true);
                        if let Some(value)=&sample.value {render_data(ui,value,&editor.monitor.filters,8);}
                        raw_output(ui,sample);
                    }
                });
                ui.separator();
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center).with_main_wrap(true),|ui|{
                    cancel=ui.button("Cancel").clicked();
                    save=ui.add_enabled(self.storage_error.is_none(),egui::Button::new(if compact {"Save"} else {"Save monitor"})).clicked();
                    let can_test=self.target.as_ref().is_some_and(Target::connected) && self.identity.as_ref()==Some(&editor.identity)
                        && self.samples.values().filter(|s|s.running.is_some()).count()<2;
                    test=ui.add_enabled(can_test,egui::Button::new("Test")).clicked();
                });
                if !multiline_focus && crate::dialog::primary(ui){save=true;}
            });
            if editor.monitor.key() != original_source {
                self.samples.remove("test");
            }
            if test || save {
                match editor.monitor.validate() {
                    Err(error) => editor.error = Some(error),
                    Ok(()) => {
                        editor.error = None;
                        if test {
                            if let Ok(command) = editor.monitor.command() {
                                self.start(
                                    "test".into(),
                                    command,
                                    Source::Monitor(editor.monitor.kind.clone()),
                                    runtime,
                                    ctx,
                                );
                            }
                        }
                        if save {
                            let mut store = self.store.clone();
                            if let Some(old) = store
                                .monitors
                                .iter_mut()
                                .find(|m| m.id == editor.monitor.id)
                            {
                                *old = editor.monitor.clone();
                            } else if store.monitors.len() < 64 {
                                store.monitors.push(editor.monitor.clone());
                            }
                            if self.persist(store) {
                                open = false;
                            }
                        }
                    }
                }
            }
            if open && !cancel {
                self.editor = Some(editor);
            } else {
                self.samples.remove("test");
            }
        }
        if let Some(monitor) = self.details.clone() {
            let mut open = true;
            egui::Window::new(&monitor.name)
                .id(egui::Id::new("monitor-details"))
                .open(&mut open)
                .fixed_size(dialog_size(ctx, egui::vec2(760.0, 520.0)))
                .resizable(false)
                .collapsible(false)
                .show(ctx, |ui| {
                    if let Some(sample) = self.samples.get(&monitor.key()) {
                        sample_status(
                            ui,
                            sample,
                            monitor.interval,
                            self.target.as_ref().is_some_and(Target::connected) && monitor.enabled,
                        );
                        egui::ScrollArea::both()
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                if let Some(value) = &sample.value {
                                    render_data(ui, value, &monitor.filters, usize::MAX);
                                }
                                raw_output(ui, sample);
                            });
                    } else {
                        ui.label("No reading available.");
                    }
                });
            if !open {
                self.details = None;
            }
        }
    }
}
impl Drop for Panel {
    fn drop(&mut self) {
        self.stop();
    }
}
enum Source {
    System(bool),
    Monitor(Kind),
}
fn field(ui: &mut egui::Ui, label: &str, value: &mut String) {
    ui.label(label);
    ui.add(egui::TextEdit::singleline(value).desired_width(f32::INFINITY));
}
fn card(ui: &mut egui::Ui, title: &str, body: impl FnOnce(&mut egui::Ui)) {
    let width = ui.available_width();
    egui::Frame::group(ui.style())
        .inner_margin(8.0)
        .show(ui, |ui| {
            ui.set_width((width - 18.0).max(20.0));
            ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                ui.add(egui::Label::new(egui::RichText::new(title).strong()).wrap());
                body(ui);
            });
        });
    ui.add_space(6.0);
}
fn dialog_size(ctx: &egui::Context, desired: egui::Vec2) -> egui::Vec2 {
    desired.min((ctx.content_rect().size() - egui::vec2(40.0, 80.0)).max(egui::vec2(100.0, 120.0)))
}
fn rate(value: f64) -> String {
    if value >= 1048576.0 {
        format!("{:.2} MiB/s", value / 1048576.0)
    } else {
        format!("{:.1} KiB/s", value / 1024.0)
    }
}
fn home_label(home: &data::HomeUsage) -> String {
    let folder = home.path.rsplit('/').next().unwrap_or(&home.path);
    if folder == home.owner {
        folder.into()
    } else {
        format!("{folder} ({})", home.owner)
    }
}
fn uptime(seconds: u64) -> String {
    format!(
        "{}d {:02}:{:02}",
        seconds / 86400,
        seconds / 3600 % 24,
        seconds / 60 % 60
    )
}
fn account_summary(users: &[(String, String, String)]) -> String {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for (user, _, _) in users {
        *counts.entry(user).or_default() += 1;
    }
    if counts.is_empty() {
        return "—".into();
    }
    counts
        .into_iter()
        .map(|(user, n)| {
            if n == 1 {
                user.to_owned()
            } else {
                format!("{user}:{n}")
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}
fn compact_value(value: &Data, filters: &Filters) -> String {
    match value {
        Data::Licences(features) => {
            let visible = filters.features(features);
            if visible.is_empty() {
                return "No matching use".into();
            }
            visible
                .iter()
                .take(4)
                .map(|f| {
                    let users = f
                        .users
                        .iter()
                        .map(|u| {
                            if u.count == 1 {
                                u.user.clone()
                            } else {
                                format!("{}:{}", u.user, u.count)
                            }
                        })
                        .collect::<Vec<_>>()
                        .join(" ");
                    format!(
                        "{} {}/{} {}",
                        filters.label(&f.name),
                        f.used,
                        f.total,
                        users
                    )
                })
                .collect::<Vec<_>>()
                .join(" · ")
                .chars()
                .take(160)
                .collect()
        }
        Data::Value(s) | Data::Text(s) => s
            .lines()
            .find(|l| l.to_lowercase().contains(&filters.search.to_lowercase()))
            .unwrap_or("No matching output")
            .chars()
            .take(120)
            .collect(),
        Data::Table(table) => format!(
            "{} matching rows",
            table
                .rows
                .iter()
                .filter(|r| r
                    .iter()
                    .any(|c| c.to_lowercase().contains(&filters.search.to_lowercase())))
                .count()
        ),
        Data::System(_) => String::new(),
    }
}
fn visibility_card(
    ui: &mut egui::Ui,
    title: &str,
    visible: &mut bool,
    body: impl FnOnce(&mut egui::Ui),
) {
    card(ui, title, |ui| {
        ui.checkbox(visible, "Show in status bar");
        body(ui);
    });
}
fn metric_segment(
    ui: &mut egui::Ui,
    icon: Icon,
    text: &str,
    tooltip: &str,
    width: f32,
    height: f32,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::click());
    let visuals = ui.style().interact(&response);
    let color = if tooltip.contains("Stale") || tooltip.contains("Disconnected") {
        ui.visuals().warn_fg_color
    } else if response.hovered() {
        visuals.fg_stroke.color
    } else {
        ui.visuals().text_color()
    };
    if response.hovered() {
        ui.painter().rect_filled(rect, 0, visuals.bg_fill);
    }
    ui.painter().line_segment(
        [rect.right_top(), rect.right_bottom()],
        ui.visuals().widgets.noninteractive.bg_stroke,
    );
    ui_icons::draw(
        ui.painter(),
        egui::Rect::from_center_size(
            egui::pos2(
                if text.is_empty() {
                    rect.center().x
                } else {
                    rect.left() + 16.0
                },
                rect.center().y,
            ),
            egui::vec2(18.0, 18.0),
        ),
        icon,
        color,
    );
    let galley = egui::WidgetText::from(text).into_galley(
        ui,
        Some(egui::TextWrapMode::Truncate),
        (width - 40.0).max(0.0),
        egui::TextStyle::Body,
    );
    ui.painter().galley(
        egui::pos2(rect.left() + 32.0, rect.center().y - galley.size().y / 2.0),
        galley,
        color,
    );
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, response.enabled(), tooltip)
    });
    response.on_hover_text(tooltip)
}
fn sample_status(ui: &mut egui::Ui, sample: &Sample, interval: u32, active: bool) {
    // One reserved line: refresh spinners/timestamps never move the cards.
    let label = if !active {
        "Inactive"
    } else if sample.error.is_some() {
        "Check failed · hover for details"
    } else if sample.stale(interval) {
        "Awaiting a fresh reading"
    } else {
        ""
    };
    let mut tooltip = sample.error.clone().unwrap_or_default();
    if sample.running.is_some() {
        tooltip.push_str("\nRefreshing");
    }
    if let Some(at) = sample.received {
        tooltip.push_str(&format!(
            "\nLast reading {} seconds ago",
            at.elapsed().as_secs()
        ));
    }
    ui.add_sized(
        [
            ui.available_width(),
            ui.text_style_height(&egui::TextStyle::Small),
        ],
        egui::Label::new(egui::RichText::new(label).small()).truncate(),
    )
    .on_hover_text(tooltip);
}
fn raw_output(ui: &mut egui::Ui, sample: &Sample) {
    if let Some(raw) = &sample.raw {
        egui::CollapsingHeader::new("Raw output (bounded preview)").show(ui, |ui| {
            for line in raw.lines().take(128) {
                ui.add(
                    egui::Label::new(egui::RichText::new(line).monospace())
                        .wrap()
                        .selectable(true),
                );
            }
            ui.small("Preview limited to 128 lines / 32,768 characters.");
        });
    }
}
fn render_data(ui: &mut egui::Ui, value: &Data, filters: &Filters, limit: usize) {
    match value {
        Data::Licences(features) => {
            let visible = filters.features(features);
            if visible.is_empty() {
                ui.label("No licence features match these filters.");
            }
            if filters.group_users {
                let mut users: BTreeMap<String, Vec<String>> = BTreeMap::new();
                for feature in &visible {
                    for user in &feature.users {
                        users.entry(user.user.clone()).or_default().push(format!(
                            "{} · {} · {} · {} · {} licences",
                            filters.label(&feature.name),
                            user.machine,
                            user.display,
                            user.since,
                            user.count
                        ));
                    }
                }
                for (user, items) in users.iter().take(limit) {
                    ui.strong(user);
                    for item in items.iter().take(limit) {
                        ui.add(egui::Label::new(item).wrap());
                    }
                    ui.separator();
                }
                if users.is_empty() && !visible.is_empty() {
                    ui.label("User details not reported by the licence server.");
                }
                if users.len() > limit {
                    ui.small("Open details for more users.");
                }
            } else {
                for feature in visible.iter().take(limit) {
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(filters.label(&feature.name)).strong(),
                        )
                        .wrap(),
                    )
                    .on_hover_text(&feature.name);
                    ui.label(format!(
                        "{} / {} licences checked out",
                        feature.used, feature.total
                    ));
                    let users: BTreeSet<_> =
                        feature.users.iter().map(|u| u.user.as_str()).collect();
                    if !users.is_empty() {
                        ui.add(
                            egui::Label::new(format!(
                                "Users: {}",
                                users.into_iter().collect::<Vec<_>>().join(", ")
                            ))
                            .wrap(),
                        );
                    } else if feature.used > 0 {
                        ui.small("User details not reported by the licence server.");
                    }
                    if limit == usize::MAX {
                        for checkout in &feature.users {
                            ui.label(format!(
                                "{} · {} · {} · {} · {} licences",
                                checkout.user,
                                checkout.machine,
                                checkout.display,
                                checkout.since,
                                checkout.count
                            ));
                        }
                    }
                    ui.separator();
                }
                if visible.len() > limit {
                    ui.small(format!(
                        "{} more features · open details",
                        visible.len() - limit
                    ));
                }
            }
            ui.small("Counts are server totals; user/machine filters select the details shown.");
        }
        Data::Text(text) | Data::Value(text) => {
            let lines: Vec<_> = text
                .lines()
                .filter(|line| line.to_lowercase().contains(&filters.search.to_lowercase()))
                .collect();
            let take = if matches!(value, Data::Value(_)) {
                1
            } else {
                limit.min(512)
            };
            for line in lines.iter().take(take) {
                ui.add(egui::Label::new(*line).wrap().selectable(true));
            }
            if lines.len() > take {
                ui.small("Open details for more output.");
            }
            if lines.is_empty() {
                ui.label("No matching output.");
            }
        }
        Data::Table(table) => {
            let rows: Vec<_> = table
                .rows
                .iter()
                .filter(|row| {
                    row.iter()
                        .any(|c| c.to_lowercase().contains(&filters.search.to_lowercase()))
                })
                .collect();
            if limit == usize::MAX {
                egui::Grid::new("monitor-output-table")
                    .striped(true)
                    .show(ui, |ui| {
                        for title in &table.columns {
                            ui.strong(title);
                        }
                        ui.end_row();
                        for row in rows {
                            for cell in row {
                                ui.add(egui::Label::new(cell).wrap().selectable(true));
                            }
                            ui.end_row();
                        }
                    });
            } else {
                for row in rows.iter().take(limit) {
                    for (label, cell) in table.columns.iter().zip(row.iter()) {
                        ui.add(egui::Label::new(format!("{label}: {cell}")).wrap());
                    }
                    ui.separator();
                }
                if rows.len() > limit {
                    ui.small("Open details for the complete table.");
                }
            }
        }
        Data::System(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn monitoring_settings_preserve_scopes_and_corrupt_files() {
        let root = tempfile::tempdir().unwrap();
        crate::storage::with_test_root(root.path(), || {
            let mut store = Store::default();
            let mut monitor = Monitor::new("profile-one".into(), false);
            monitor.kind = Kind::Custom {
                command: "printf ok".into(),
                format: Format::Value,
            };
            monitor.show_in_bar = true;
            store.visibility[3] = false;
            store.monitors.push(monitor);
            store.save().unwrap();
            let read = Store::load().unwrap();
            assert!(read.monitors[0].show_in_bar);
            assert!(!read.visibility[3]);
            let legacy: Store =
                serde_json::from_str(r#"{"schema":1,"pinned":false,"monitors":[]}"#).unwrap();
            assert_eq!(legacy.visibility, [true; 6]);
            assert_eq!(read.monitors[0].profile, "profile-one");
            assert_ne!(read.monitors[0].profile, "profile-two");
            std::fs::write(root.path().join("monitoring.json"), "broken {").unwrap();
            assert!(Store::load().is_err());
            assert_eq!(
                std::fs::read_to_string(root.path().join("monitoring.json")).unwrap(),
                "broken {"
            );
        });
    }
    #[test]
    fn monitor_schedule_does_not_overlap_or_treat_old_results_as_live() {
        let mut sample = Sample::default();
        assert!(sample.due(Duration::from_secs(30)));
        sample.finished = Some(Instant::now());
        assert!(!sample.due(Duration::from_secs(30)));
        assert!(sample.stale(30));
        sample.received = Some(Instant::now());
        assert!(!sample.stale(30));
        let (send, result) = mpsc::channel(1);
        let cancel = CancellationToken::new();
        sample.running = Some(Running {
            cancel: cancel.clone(),
            result,
        });
        sample.finished = None;
        assert!(!sample.due(Duration::ZERO));
        sample.running = None;
        assert!(cancel.is_cancelled());
        drop(send);
        let mut first = Monitor::new("one".into(), true);
        first.kind = Kind::FlexNet {
            executable: "lmutil".into(),
            server: "5280@license-server.example".into(),
        };
        let mut second = first.clone();
        second.filters.user = "user1".into();
        assert_eq!(first.key(), second.key());
        let mut panel = Panel::default();
        panel.switch_identity(Some("connection-one".into()));
        let cancel = CancellationToken::new();
        let (_send, result) = mpsc::channel(1);
        panel.samples.insert(
            "source".into(),
            Sample {
                value: Some(Data::Text("server-one".into())),
                running: Some(Running {
                    cancel: cancel.clone(),
                    result,
                }),
                ..Default::default()
            },
        );
        panel.switch_identity(Some("connection-two".into()));
        assert!(cancel.is_cancelled());
        assert!(panel.samples.is_empty());
    }
    #[test]
    fn monitoring_cards_stack_within_a_narrow_panel() {
        let ctx = egui::Context::default();
        let mut rects = Vec::new();
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(240.0, 700.0),
                )),
                ..Default::default()
            },
            |ctx| {
                rects.clear();
                egui::CentralPanel::default().show(ctx, |ui| {
                    for title in ["CPU", "Memory", "Recorded logins"] {
                        let before = ui.cursor().top();
                        card(ui, title, |ui| {
                            ui.label("A long sample that must wrap within this monitor card.");
                        });
                        rects.push((before, ui.cursor().top(), ui.min_rect().right()));
                    }
                });
            },
        );
        assert_eq!(
            account_summary(&[
                ("user2".into(), "".into(), "".into()),
                ("user1".into(), "".into(), "".into()),
                ("user1".into(), "".into(), "".into()),
                ("user3".into(), "".into(), "".into())
            ]),
            "user1:2 user2 user3"
        );
        // Status refresh state must not change the reserved geometry.
        let mut heights = vec![];
        for running in [false, true] {
            let mut sample = Sample {
                received: Some(Instant::now()),
                ..Default::default()
            };
            let (_send, result) = mpsc::channel(1);
            if running {
                sample.running = Some(Running {
                    cancel: CancellationToken::new(),
                    result,
                });
            }
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let top = ui.cursor().top();
                    sample_status(ui, &sample, 3, true);
                    heights.push(ui.cursor().top() - top);
                });
            });
        }
        assert!(heights.iter().all(|h| (*h - heights[0]).abs() < 0.1));
        assert_eq!(rects.len(), 3);
        for pair in rects.windows(2) {
            assert!(pair[0].1 <= pair[1].0);
        }
        assert!(rects
            .iter()
            .all(|(top, bottom, right)| bottom > top && *right <= 240.0));
    }
}
