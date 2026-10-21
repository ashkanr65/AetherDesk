//! Explicit self-check of this application's own renderer and popup lifecycle.
//! No host/provider connections or arbitrary desktop capture are performed.
use eframe::egui;
use serde_json::json;
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

struct Smoke {
    app: super::RustDesktop,
    directory: PathBuf,
    started: Instant,
    phase: u8,
    ticks: u32,
    error: Option<String>,
    files: Option<crate::session::SessionTab>,
    popup_step: u8,
    popup_at: Instant,
    editor_checked: bool,
    files_checked: bool,
}
impl Smoke {
    fn finish(&self, passed: bool) {
        let result = json!({"passed":passed,"renderedDarkAndLight":self.phase>=2,
            "nativeEditorOpenedAndClosed":self.editor_checked,"nativeFilesOpenedAndClosed":self.files_checked,
            "nativePopupResizeMinimizeRestore":self.editor_checked && self.files_checked,
            "mainWindowContinuedRendering":passed,
            "isolatedSettings":true,"elapsedMs":self.started.elapsed().as_millis(),"error":self.error});
        let _ = std::fs::write(
            self.directory.join("ui-smoke.json"),
            serde_json::to_vec_pretty(&result).unwrap(),
        );
    }
}
impl eframe::App for Smoke {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        eframe::App::update(&mut self.app, ctx, frame);
        self.ticks += 1;
        if let (Some(files), Ok(runtime)) = (&mut self.files, &self.app.connections) {
            files.files_window(ctx, runtime);
        }
        let screenshots = ctx.input(|input| {
            input
                .events
                .iter()
                .filter_map(|event| match event {
                    egui::Event::Screenshot {
                        viewport_id, image, ..
                    } if *viewport_id == egui::ViewportId::ROOT => Some(image.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>()
        });
        for image in screenshots {
            let name = if self.phase == 0 {
                "dark.png"
            } else {
                "light.png"
            };
            let bytes = image
                .pixels
                .iter()
                .flat_map(|pixel| pixel.to_array())
                .collect::<Vec<_>>();
            let saved = image::save_buffer(
                self.directory.join(name),
                &bytes,
                image.size[0] as u32,
                image.size[1] as u32,
                image::ColorType::Rgba8,
            );
            if let Err(error) = saved {
                self.error = Some(error.to_string());
            }
            self.phase += 1;
            self.ticks = 0;
            if self.phase == 1 {
                self.app.dark = false;
                self.app.preferences.settings.mode = crate::preferences::Mode::Light;
                super::palette(ctx, false);
            }
        }
        if self.phase <= 1 && self.ticks == 20 {
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
        }
        if self.phase == 2 && self.ticks == 20 {
            let path = self.directory.join("popup-proof.txt");
            if let Err(error) = std::fs::write(&path, "R12 editor popup proof\n") {
                self.error = Some(error.to_string());
            }
            if let Ok(runtime) = &self.app.connections {
                self.app.editors.open(
                    super::editor::OpenFile {
                        location: super::editor::Location::Local(path),
                        force_text: true,
                        action: Default::default(),
                    },
                    runtime,
                    ctx,
                );
                self.phase = 3;
                self.ticks = 0;
            } else {
                self.error = Some("Application runtime unavailable".into());
            }
        }
        let popup = if self.phase == 3 {
            self.app
                .editors
                .diagnostic_window()
                .filter(|(_, ready)| *ready)
                .map(|(id, _)| id)
        } else if self.phase == 5 {
            self.files
                .as_ref()
                .filter(|f| f.diagnostic_files_open())
                .map(|f| f.files_viewport_id())
        } else {
            None
        };
        if let Some(id) = popup {
            if crate::app_layout::diagnostic_size(ctx, id).is_some()
                && self.popup_at.elapsed() > Duration::from_millis(600)
            {
                match self.popup_step {
                    0 => ctx.send_viewport_cmd_to(
                        id,
                        egui::ViewportCommand::InnerSize(egui::vec2(640.0, 440.0)),
                    ),
                    1 => {
                        let size = crate::app_layout::diagnostic_size(ctx, id);
                        if !size.is_some_and(|s| {
                            s.is_finite() && (s - egui::vec2(640.0, 440.0)).length() < 8.0
                        }) {
                            self.error = Some(format!(
                                "Native popup did not resize to the narrow test area: {size:?}"
                            ));
                        }
                        ctx.send_viewport_cmd_to(id, egui::ViewportCommand::Minimized(true));
                    }
                    2 => ctx.send_viewport_cmd_to(id, egui::ViewportCommand::Minimized(false)),
                    3 => ctx.send_viewport_cmd_to(
                        id,
                        egui::ViewportCommand::InnerSize(egui::vec2(1000.0, 650.0)),
                    ),
                    _ => {
                        ctx.send_viewport_cmd_to(id, egui::ViewportCommand::Close);
                        self.phase += 1;
                        self.ticks = 0;
                    }
                }
                self.popup_step += 1;
                self.popup_at = Instant::now();
            }
        }
        if self.phase == 4 && self.ticks > 20 && self.app.editors.diagnostic_window().is_none() {
            self.editor_checked = true;
            self.files = Some(crate::session::SessionTab::diagnostic_files(
                &self.directory,
            ));
            self.phase = 5;
            self.popup_step = 0;
            self.popup_at = Instant::now();
        }
        if self.phase == 6
            && self.ticks > 40
            && self
                .files
                .as_ref()
                .is_some_and(|f| !f.diagnostic_files_open())
        {
            self.files_checked = true;
            match std::fs::read(self.directory.join("popup-proof.txt")) {
                Ok(bytes) if bytes == b"R12 editor popup proof\n" => {}
                _ => self.error = Some("Popup lifecycle changed the saved editor fixture".into()),
            }
            self.finish(self.error.is_none());
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        } else if self.error.is_some() || self.started.elapsed() > Duration::from_secs(35) {
            if self.error.is_none() {
                self.error = Some("Renderer or popup lifecycle timed out".into());
            }
            self.finish(false);
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        ctx.request_repaint_after(Duration::from_millis(30));
    }
}
pub fn run() -> eframe::Result {
    // Require an explicitly isolated profile, rather than using the user's data.
    let directory =
        PathBuf::from(std::env::var_os("AETHERDESK_R12_UI_ROOT").expect("R12 UI root required"));
    let directory = std::fs::canonicalize(directory).expect("Existing R12 UI root required");
    let appdata =
        std::fs::canonicalize(std::env::var_os("APPDATA").expect("APPDATA required")).unwrap();
    assert_eq!(
        appdata,
        directory.join("profile"),
        "R12 refuses the user's normal profile"
    );
    let mut options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 840.0])
            .with_title("AetherDesk — isolated R12 UI check")
            .with_icon(super::theme::native_icon()),
        ..Default::default()
    };
    if let Ok(root) = crate::storage::root() {
        crate::logging::init(&root.join("logs/application.log"));
        crate::crash_diagnostics::init(&root.join("logs"));
    }
    super::graphics::configure(
        &mut options,
        crate::preferences::Settings::default().renderer,
    );
    eframe::run_native(
        "AetherDesk R12 check",
        options,
        Box::new(move |creation| {
            super::graphics::log_adapter(creation);
            creation
                .egui_ctx
                .set_fonts(super::theme::font_definitions());
            let mut app = super::RustDesktop::default();
            app.dark = true;
            super::palette(&creation.egui_ctx, true);
            Ok(Box::new(Smoke {
                app,
                directory,
                started: Instant::now(),
                phase: 0,
                ticks: 0,
                error: None,
                files: None,
                popup_step: 0,
                popup_at: Instant::now(),
                editor_checked: false,
                files_checked: false,
            }))
        }),
    )
}
