//! The host caption is inside the Windows view. Their Close buttons have
//! deliberately different lifetimes; never turn local WM_CLOSE into host Close.
use super::*;

fn point(rect: egui::Rect, size: (u32, u32), pos: egui::Pos2) -> Option<(u32, u32)> {
    if !rect.is_positive()
        || !rect.contains(pos)
        || !pos.x.is_finite()
        || !pos.y.is_finite()
        || size.0 == 0
        || size.1 == 0
    {
        return None;
    }
    Some((
        (((pos.x - rect.left()) / rect.width()) * size.0 as f32)
            .floor()
            .max(0.0) as u32,
        (((pos.y - rect.top()) / rect.height()) * size.1 as f32)
            .floor()
            .max(0.0) as u32,
    ))
    .filter(|&(x, y)| x < size.0 && y < size.1)
}
impl Applications {
    pub(super) fn host_titlebar(
        &mut self,
        ctx: &egui::Context,
        state: &State,
        window: &WindowInfo,
    ) {
        if !state.host_titlebar {
            return;
        }
        let bar = state
            .titlebar
            .as_ref()
            .filter(|bar| bar.info.width as usize <= ctx.input(|i| i.max_texture_side));
        let height = bar
            .map(|bar| {
                view::display_scale(ctx, self.content_zoom)
                    .logical(egui::vec2(0.0, bar.info.height as f32))
                    .y
            })
            .unwrap_or(28.0);
        egui::TopBottomPanel::top("host-titlebar")
            .frame(egui::Frame::NONE).exact_height(height).show(ctx, |ui| {
                if let Some(bar) = bar {
                    if self.titlebar_texture.as_ref().is_none_or(|(_, image)|
                        !std::sync::Arc::ptr_eq(image, &bar.image))
                    {
                        self.titlebar_texture = Some((ctx.load_texture(
                            format!("host-titlebar-{:?}", self.id),
                            egui::ImageData::Color(bar.image.clone()), egui::TextureOptions::LINEAR,
                        ), bar.image.clone()));
                    }
                    let (rect, response) = ui.allocate_exact_size(
                        egui::vec2(ui.available_width().max(1.0), height), egui::Sense::click());
                    ui.painter().image(self.titlebar_texture.as_ref().unwrap().0.id(), rect,
                        egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0,1.0)), egui::Color32::WHITE);
                    if state.alive && response.clicked_by(egui::PointerButton::Primary) {
                        if let Some((x,y)) = response.interact_pointer_pos().and_then(|p|
                            point(rect, (bar.info.width, bar.info.height), p))
                        {
                            self.release();
                            self.send(Request::TitlebarClick { window: window.id, x, y });
                        }
                    }
                    response.on_hover_text("Host title bar: its controls affect the host application. Use the Windows title bar to move this view or close it while keeping the application running.");
                } else {
                    // Unmanaged/undecorated clients have no host strip to read.
                    // Keep an explicit graceful close, with no force-kill path.
                    ui.horizontal(|ui| {
                        ui.add_sized(egui::vec2((ui.available_width()-28.0-ui.spacing().item_spacing.x).max(0.0),28.0),
                            egui::Label::new(&window.title).truncate());
                            ui.add_enabled_ui(state.alive, |ui| {
                                if crate::ui_icons::button(ui, crate::ui_icons::Icon::CloseApplication,
                                    "Close application on host (allow Save/Cancel)", false).clicked()
                                {
                                    self.release();
                                    self.send(Request::CloseApplication { window: window.id });
                                }
                            });
                    });
                }
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn host_caption_uses_one_row_and_clicks_the_host_without_closing_the_view() {
        let ctx = egui::Context::default();
        let window = WindowInfo {
            id: 7,
            title: "Editor".into(),
            width: 800,
            height: 400,
            transient_for: None,
        };
        let state = State {
            alive: true,
            host_titlebar: true,
            selected: Some(window.clone()),
            titlebar: Some(worker::PopupState {
                info: wire::Popup {
                    window: 77,
                    x: 4,
                    y: 0,
                    width: 800,
                    height: 24,
                    tiles: Vec::new(),
                },
                image: std::sync::Arc::new(egui::ColorImage::new(
                    [800, 24],
                    vec![egui::Color32::BLUE; 800 * 24],
                )),
            }),
            ..Default::default()
        };
        let (send, mut receive) = tokio::sync::mpsc::channel(16);
        let mut app = Applications::new(None, uuid::Uuid::new_v4());
        app.view = true;
        app.worker = Some(worker::Worker {
            commands: send,
            state: std::sync::Arc::new(parking_lot::Mutex::new(state.clone())),
            cancel: tokio_util::sync::CancellationToken::new(),
        });
        let input = |events| egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1000.0, 600.0),
            )),
            events,
            ..Default::default()
        };
        for events in [
            vec![],
            vec![],
            vec![
                egui::Event::PointerMoved(egui::pos2(980.0, 12.0)),
                egui::Event::PointerButton {
                    pos: egui::pos2(980.0, 12.0),
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: Default::default(),
                },
            ],
            vec![egui::Event::PointerButton {
                pos: egui::pos2(980.0, 12.0),
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: Default::default(),
            }],
        ] {
            let _ = ctx.run(input(events), |ctx| {
                app.host_titlebar(ctx, &state, &window);
                assert!(
                    (ctx.available_rect().height() - 576.0).abs() < 0.1,
                    "caption consumed the application surface"
                );
            });
        }
        assert!(matches!(
            receive.try_recv().unwrap(),
            Request::TitlebarClick {
                window: 7,
                x: 784,
                y: 12
            }
        ));
        assert!(receive.try_recv().is_err());
        assert!(app.view && app.worker.is_some());
    }
    #[test]
    fn host_caption_clicks_keep_coordinates_across_dpi_and_resize_and_reject_outside() {
        for scale in [0.5, 1.0, 1.5, 2.0] {
            let rect =
                egui::Rect::from_min_size(egui::pos2(12.0, 70.0), egui::vec2(800.0, 24.0) * scale);
            assert_eq!(
                point(rect, (800, 24), rect.min + egui::vec2(790.0, 12.0) * scale),
                Some((790, 12))
            );
            assert_eq!(
                point(rect, (800, 24), rect.min - egui::vec2(1.0, 0.0)),
                None
            );
            assert_eq!(point(rect, (800, 24), rect.max), None);
        }
        assert_eq!(
            point(egui::Rect::NOTHING, (800, 24), egui::Pos2::ZERO),
            None
        );
    }
}
