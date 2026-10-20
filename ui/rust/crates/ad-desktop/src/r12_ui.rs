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
}
impl Smoke {
    fn finish(&self, passed: bool) {
        let result = json!({"passed":passed,"renderedDarkAndLight":self.phase>=2,
            "nativeEditorOpenedAndClosed":self.phase>=4,"mainWindowContinuedRendering":passed,
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
        if self.phase == 3 && self.ticks > 20 {
            if let Some((id, true)) = self.app.editors.diagnostic_window() {
                // The same native viewport close request used by its title bar.
                ctx.send_viewport_cmd_to(id, egui::ViewportCommand::Close);
                self.phase = 4;
                self.ticks = 0;
            }
        }
        if self.phase == 4 && self.ticks > 40 && self.app.editors.diagnostic_window().is_none() {
            self.finish(self.error.is_none());
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        } else if self.error.is_some() || self.started.elapsed() > Duration::from_secs(25) {
            if self.error.is_none() {
                self.error = Some("Renderer or editor popup lifecycle timed out".into());
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
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 840.0])
            .with_title("AetherDesk — isolated R12 UI check")
            .with_icon(super::theme::native_icon()),
        ..Default::default()
    };
    eframe::run_native(
        "AetherDesk R12 check",
        options,
        Box::new(move |creation| {
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
            }))
        }),
    )
}
