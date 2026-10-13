#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]
use ad_desktop_model::{
    profiles::{decode_sessions, SavedSession},
    state::{Sidebar, WorkspaceState},
};
use eframe::egui;
use std::path::PathBuf;

struct RustDesktop {
    workspace: WorkspaceState<SavedSession>,
    profiles: Vec<SavedSession>,
    filter: String,
    notice: String,
    dark: bool,
}
impl Default for RustDesktop {
    fn default() -> Self {
        Self {
            workspace: WorkspaceState::default(),
            profiles: vec![],
            filter: String::new(),
            notice: "Rust migration preview — connection, editor and AI parity is not complete."
                .to_owned(),
            dark: true,
        }
    }
}
fn palette(ctx: &egui::Context, dark: bool) {
    let ink = egui::Color32::from_rgb(44, 43, 63);
    let cream = egui::Color32::from_rgb(252, 238, 235);
    let peach = egui::Color32::from_rgb(247, 214, 208);
    let rose = egui::Color32::from_rgb(199, 162, 179);
    let lavender = egui::Color32::from_rgb(124, 122, 154);
    let mut visuals = if dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };
    visuals.panel_fill = if dark { ink } else { cream };
    visuals.window_fill = visuals.panel_fill;
    visuals.override_text_color = Some(if dark { cream } else { ink });
    visuals.selection.bg_fill = rose;
    visuals.selection.stroke = egui::Stroke::new(1.0, ink);
    visuals.widgets.active.bg_fill = rose;
    visuals.widgets.active.fg_stroke = egui::Stroke::new(1.0, ink);
    visuals.widgets.hovered.bg_fill = if dark { lavender } else { peach };
    visuals.widgets.hovered.fg_stroke = egui::Stroke::new(1.0, ink);
    ctx.set_visuals(visuals);
}
impl RustDesktop {
    fn import_profiles(&mut self) {
        // Explicit read-only import. Never mutate the production application's profile.
        let result = std::env::var_os("APPDATA")
            .ok_or("APPDATA is unavailable".to_owned())
            .and_then(|root| {
                std::fs::read(PathBuf::from(root).join("AetherDesk/sessions.json"))
                    .map_err(|error| format!("Could not read saved sessions: {error}"))
            })
            .and_then(|bytes| decode_sessions(&bytes));
        match result {
            Ok(profiles) => {
                self.profiles = profiles;
                self.notice =
                    "Saved sessions imported read-only. Connection UI is still being ported."
                        .to_owned();
            }
            Err(error) => self.notice = error,
        }
    }
}
impl eframe::App for RustDesktop {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::TopBottomPanel::top("toolbar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui.button("Menu").clicked() {
                    self.workspace.sidebar_visible = !self.workspace.sidebar_visible;
                }
                ui.heading("AetherDesk · Rust preview");
                if ui
                    .button(if self.dark { "Light mode" } else { "Dark mode" })
                    .clicked()
                {
                    self.dark = !self.dark;
                    palette(ctx, self.dark);
                }
                if ui.button("Import saved sessions (read only)").clicked() {
                    self.import_profiles();
                }
            });
        });
        if self.workspace.sidebar_visible {
            let width=egui::SidePanel::left("workspace").resizable(true).default_width(314.0)
                .width_range(280.0..=900.0).show(ctx,|ui| {
                    ui.horizontal_wrapped(|ui| {
                        for section in Sidebar::ALL {
                            if ui.selectable_label(self.workspace.sidebar==section,section.label()).clicked() {
                                self.workspace.select_sidebar(section);
                            }
                        }
                    });
                    ui.separator();
                    match self.workspace.sidebar {
                        Sidebar::Sessions|Sidebar::Bookmarks=> {
                            ui.text_edit_singleline(&mut self.filter);
                            let query=self.filter.to_lowercase();
                            for profile in self.profiles.iter().filter(|p|!p.is_deleted()) {
                                if self.workspace.sidebar==Sidebar::Bookmarks && !profile.favorite {continue;}
                                if !format!("{} {} {}",profile.name,profile.host,profile.username).to_lowercase().contains(&query) {continue;}
                                if ui.button(format!("{} · {} · {}",profile.name,profile.host,profile.connection_type.label())).clicked() {
                                    self.workspace.open_new(profile.clone());
                                }
                            }
                        }
                        _=>{ui.label("This sidebar's functional port is pending. The reference implementation remains in ui/flutter.");}
                    }
                }).response.rect.width();
            self.workspace.sidebar_width = width;
        }
        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.label(&self.notice);
        });
        egui::CentralPanel::default().show(ctx,|ui| {
            ui.horizontal_wrapped(|ui| {
                let mut close=None;
                for (index,tab) in self.workspace.tabs.iter().enumerate() {
                    if ui.selectable_label(self.workspace.selected==Some(index),&tab.name).clicked() {self.workspace.selected=Some(index);}
                    if ui.small_button("×").clicked() {close=Some(index);}
                }
                if let Some(index)=close {self.workspace.close_tab(index);}
            });
            ui.separator();
            if let Some(tab)=self.workspace.selected.and_then(|index|self.workspace.tabs.get(index)) {
                ui.heading(&tab.name);
                ui.label(format!("{}@{}:{} · {}",tab.username,tab.host,tab.port,tab.connection_type.label()));
                ui.label("Profile preview only. No connection has been opened.");
            } else {
                ui.heading("Rust desktop migration");
                ui.label("The production feature parity gate remains closed. This preview is not a replacement release.");
            }
        });
    }
}
fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 840.0])
            .with_min_inner_size([800.0, 500.0]),
        ..Default::default()
    };
    eframe::run_native(
        "AetherDesk Rust Preview",
        options,
        Box::new(|creation| {
            palette(&creation.egui_ctx, true);
            Ok(Box::new(RustDesktop::default()))
        }),
    )
}
