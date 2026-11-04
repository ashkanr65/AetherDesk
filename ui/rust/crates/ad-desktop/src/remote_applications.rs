//! Independent views retain the application in its original desktop session.
use ad_stream_protocol::window_stream::{self as wire, Request, WindowInfo};
use eframe::egui;
use std::{collections::BTreeSet, time::Duration};
#[path = "remote_application_worker.rs"]
mod worker;
pub(crate) use worker::Target;
use worker::{State, Worker};
#[path = "application_geometry.rs"]
mod geometry;
#[path = "application_native.rs"]
mod native;
#[path = "remote_application_view.rs"]
mod view;
#[path = "application_workspace.rs"]
pub(crate) mod workspace;

pub(crate) struct Applications {
    managed: bool,
    retry_at: std::time::Instant,
    wheel: crate::remote_mouse::Wheel,
    cursor: crate::remote_cursor::Cursor,
    cursor_client: Option<std::sync::Arc<crate::remote::RemoteClient>>,
    target: Option<Target>,
    worker: Option<Worker>,
    id: egui::ViewportId,
    chooser: bool,
    view: bool,
    search: String,
    texture: Option<egui::TextureHandle>,
    sequence: u64,
    uploaded_image: Option<worker::Frame>,
    native: native::NativeWindow,
    popup_native: std::collections::BTreeMap<u32, native::NativeWindow>,
    stream_active: Option<bool>,
    input_window: Option<isize>,
    input_epoch: u64,
    sized_window: Option<u32>,
    content_zoom: f32,
    last_extent: Option<(u32, u32)>,
    resize_environment: Option<(egui::Rect, u32)>,
    resize_due: Option<(std::time::Instant, (u32, u32))>,
    resize_after: std::time::Instant,
    popup_textures:
        std::collections::BTreeMap<u32, (egui::TextureHandle, std::sync::Arc<egui::ColorImage>)>,
    focused: bool,
    keys: BTreeSet<u32>,
    buttons: BTreeSet<u8>,
    modifiers: egui::Modifiers,
    error: Option<String>,
    title: String,
}
impl Applications {
    pub(crate) fn new(target: Option<Target>, session: uuid::Uuid) -> Self {
        Self {
            managed: false,
            retry_at: std::time::Instant::now(),
            wheel: Default::default(),
            cursor: Default::default(),
            cursor_client: None,
            target,
            worker: None,
            id: egui::ViewportId::from_hash_of(("remote-application", session)),
            chooser: false,
            view: false,
            search: String::new(),
            texture: None,
            sequence: 0,
            uploaded_image: None,
            native: Default::default(),
            popup_native: Default::default(),
            stream_active: None,
            input_window: None,
            input_epoch: 0,
            sized_window: None,
            content_zoom: 1.0,
            last_extent: None,
            resize_environment: None,
            resize_due: None,
            resize_after: std::time::Instant::now(),
            popup_textures: std::collections::BTreeMap::new(),
            focused: false,
            keys: BTreeSet::new(),
            buttons: BTreeSet::new(),
            modifiers: egui::Modifiers::default(),
            error: None,
            title: format!("AetherDesk Remote Application {session}"),
        }
    }
    pub(crate) fn open_window(&mut self, ctx: &egui::Context, window: u32) {
        if self.managed && !self.view {
            if let Some(client) = &self.cursor_client {
                // Also initialize wl_pointer on retained application-only
                // sessions whose older compositor never sent the first enter.
                // No button or modifier is injected, and existing views do not
                // repeatedly reposition the host pointer.
                if let Err(error) = client.send(crate::remote::Request::Input {
                    event: ad_stream_protocol::InputMessage::PointerMove {
                        x: 0,
                        y: 0,
                        monitor: 0,
                    },
                }) {
                    self.error = Some(error);
                    return;
                }
            }
        }
        self.start(ctx);
        self.send(Request::Select(window));
        self.view = true;
    }
    pub(crate) fn is_open(&self) -> bool {
        self.view
    }
    pub(crate) fn focus_view(&self, ctx: &egui::Context) {
        ctx.send_viewport_cmd_to(self.id, egui::ViewportCommand::Focus);
    }
    fn start(&mut self, ctx: &egui::Context) {
        if self.worker.as_ref().is_some_and(|w| w.state.lock().alive) {
            return;
        }
        if self.worker.is_some() && std::time::Instant::now() < self.retry_at {
            return;
        }
        self.retry_at = std::time::Instant::now() + Duration::from_secs(3);
        let Some(target) = self.target.clone() else {
            self.error = Some("Remote application connection is unavailable".into());
            return;
        };
        self.worker = Some(Worker::start(target, ctx.clone()));
        self.sequence = 0;
        self.texture = None;
        self.error = None;
    }
    fn send(&mut self, request: Request) {
        if let Some(w) = &self.worker {
            if w.commands.try_send(request).is_err() {
                w.cancel.cancel();
                self.error =
                    Some("Application input queue is busy. Reopen the independent view.".into());
            }
        }
    }
    fn release(&mut self) {
        crate::remote_keypad::release(self.id);
        if self.focused || !self.keys.is_empty() || !self.buttons.is_empty() || self.modifiers.any()
        {
            self.send(Request::Release);
        }
        self.focused = false;
        self.input_window = None;
        self.keys.clear();
        self.buttons.clear();
        self.modifiers = egui::Modifiers::default();
    }
    pub(crate) fn captures_keyboard(&self) -> bool {
        let foreground = crate::remote_geometry::foreground();
        self.focused
            && (self.native.handle().is_some_and(|w| foreground == Some(w))
                || self
                    .popup_native
                    .values()
                    .any(|window| window.handle().is_some_and(|w| foreground == Some(w))))
    }
    pub(crate) fn button(&mut self, ui: &mut egui::Ui) {
        if crate::ui_icons::button(
            ui,
            crate::ui_icons::Icon::Applications,
            "Remote applications",
            self.chooser,
        )
        .clicked()
        {
            self.chooser = !self.chooser;
            if self.chooser {
                self.start(ui.ctx());
                self.send(Request::List);
            }
        }
    }
    pub(crate) fn ui(&mut self, ctx: &egui::Context) {
        let state = self
            .worker
            .as_ref()
            .map(|w| w.state.lock().clone())
            .unwrap_or_default();
        if state.input_epoch != self.input_epoch {
            self.input_epoch = state.input_epoch;
            self.release();
        }
        if self.view && state.selected.is_none() && state.error.is_some() {
            self.release();
            self.view = false;
            self.chooser = !self.managed;
        }
        if self.chooser {
            let mut open = true;
            let mut pick = None;
            let chooser_id = egui::ViewportId::from_hash_of(("application-chooser", self.id));
            let builder = crate::app_layout::builder(
                ctx,
                chooser_id,
                "Remote applications",
                egui::vec2(480.0, 430.0),
            );
            ctx.show_viewport_immediate(chooser_id,builder,|ctx,_|{
                crate::app_layout::update(ctx,"remote-application-chooser");
                if ctx.input(|i|i.viewport().close_requested()){open=false;return;}
                egui::CentralPanel::default().show(ctx,|ui|{
                    ui.add(egui::Label::new("Open an existing application as an independent window. It stays running in this remote desktop.").wrap());
                    ui.horizontal(|ui|{
                        ui.add(egui::TextEdit::singleline(&mut self.search).hint_text("Search applications").desired_width((ui.available_width()-90.0).max(60.0)));
                        if ui.button("Refresh").clicked(){self.start(ctx);self.send(Request::List);}
                    });
                    if !state.alive && state.error.is_none(){ui.spinner();}
                    if let Some(error)=self.error.as_ref().or(state.error.as_ref()){ui.add(egui::Label::new(egui::RichText::new(error).color(ui.visuals().error_fg_color)).wrap());}
                    egui::ScrollArea::vertical().show(ui,|ui|{
                        for window in &state.windows{
                            if !window.title.to_lowercase().contains(&self.search.to_lowercase()){continue;}
                            let label=if window.title.is_empty(){format!("Application {}",window.id)}else{window.title.clone()};
                            if ui.add(egui::Button::new(label).truncate().min_size(egui::vec2(ui.available_width(),30.0))).clicked(){pick=Some(window.id);}
                        }
                        if state.alive && state.windows.is_empty(){ui.label("No application windows found. Open an application inside the remote desktop, then refresh.");}
                    });
                });
            });
            self.chooser = open;
            if !open && pick.is_none() && !self.view {
                self.worker.take();
            }
            if let Some(id) = pick {
                self.release();
                self.wheel = Default::default();
                self.start(ctx);
                self.send(Request::Select(id));
                self.view = true;
                self.texture = None;
                self.sequence = 0;
                self.sized_window = None;
                self.last_extent = None;
                self.resize_due = None;
                self.popup_textures.clear();
                self.chooser = false;
            }
        }
        if self.view {
            self.viewport(ctx, &state);
        }
    }
    fn sync_modifiers(&mut self, next: egui::Modifiers) {
        let next = crate::remote_keyboard::without_altgr_shortcut(next, self.keys.contains(&100));
        for (before, after, evdev) in [
            (self.modifiers.ctrl, next.ctrl, 29),
            (self.modifiers.shift, next.shift, 42),
            (self.modifiers.alt, next.alt, 56),
        ] {
            if before != after {
                self.send(Request::Key {
                    evdev,
                    pressed: after,
                });
            }
        }
        self.modifiers = next;
    }
}
impl Drop for Applications {
    fn drop(&mut self) {
        self.release();
        self.worker.take();
    }
}

fn point(pos: egui::Pos2, rect: egui::Rect, window: &WindowInfo) -> (i32, i32) {
    (
        ((pos.x - rect.left()) / rect.width().max(1.0) * window.width as f32) as i32,
        ((pos.y - rect.top()) / rect.height().max(1.0) * window.height as f32) as i32,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn retained_application_view_initializes_pointer_without_clicks_or_modifiers() {
        let ctx = egui::Context::default();
        let client = crate::remote::RemoteClient::test_stub(ctx.clone());
        let (commands, mut requests) = tokio::sync::mpsc::channel(16);
        let mut app = Applications::new(None, uuid::Uuid::new_v4());
        app.managed = true;
        app.cursor_client = Some(client.clone());
        app.worker = Some(Worker {
            commands,
            state: std::sync::Arc::new(parking_lot::Mutex::new(State {
                alive: true,
                ..Default::default()
            })),
            cancel: tokio_util::sync::CancellationToken::new(),
        });
        app.open_window(&ctx, 42);
        assert!(matches!(requests.try_recv(), Ok(Request::Select(42))));
        assert!(matches!(
            client.test_next_input().await,
            ad_stream_protocol::ArppMessage::Input(ad_stream_protocol::InputMessage::PointerMove {
                x: 0,
                y: 0,
                monitor: 0
            })
        ));
        // Re-selecting an already-open view must not jump the host cursor.
        app.open_window(&ctx, 42);
        assert!(
            tokio::time::timeout(Duration::from_millis(20), client.test_next_input())
                .await
                .is_err()
        );
        assert!(app.keys.is_empty() && app.buttons.is_empty() && !app.modifiers.any());
    }
    #[test]
    fn silent_focus_recovery_resets_owned_modifiers_and_buttons_before_new_input() {
        let (commands, mut requests) = tokio::sync::mpsc::channel(16);
        let mut app = Applications::new(None, uuid::Uuid::new_v4());
        app.worker = Some(Worker {
            commands,
            state: std::sync::Arc::new(parking_lot::Mutex::new(State {
                input_epoch: 1,
                ..Default::default()
            })),
            cancel: tokio_util::sync::CancellationToken::new(),
        });
        app.focused = true;
        app.modifiers.ctrl = true;
        app.modifiers.shift = true;
        app.keys.extend([29, 42]);
        app.buttons.insert(1);
        app.ui(&egui::Context::default());
        assert!(app.keys.is_empty() && app.buttons.is_empty());
        assert!(!app.modifiers.any());
        assert!(!app.focused);
        assert!(app.error.is_none());
        assert!(matches!(requests.try_recv(), Ok(Request::Release)));
        app.sync_modifiers(egui::Modifiers {
            ctrl: true,
            shift: true,
            ..Default::default()
        });
        assert!(matches!(
            requests.try_recv(),
            Ok(Request::Key {
                evdev: 29,
                pressed: true
            })
        ));
        assert!(matches!(
            requests.try_recv(),
            Ok(Request::Key {
                evdev: 42,
                pressed: true
            })
        ));
    }
    #[test]
    fn application_view_stays_registered_and_close_only_stops_viewing() {
        use std::{
            cell::{Cell, RefCell},
            rc::Rc,
            sync::Arc,
        };
        let ctx = egui::Context::default();
        ctx.set_embed_viewports(false);
        ctx.options_mut(|o| o.max_passes = 1.try_into().unwrap());
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                egui::Context::set_immediate_viewport_renderer(|ctx, mut viewport| {
                    (viewport.viewport_ui_cb)(ctx);
                });
            }
        }
        let _reset = Reset;
        let seen = Rc::new(RefCell::new(Vec::new()));
        let recorded = seen.clone();
        let closing = Rc::new(Cell::new(false));
        let close = closing.clone();
        egui::Context::set_immediate_viewport_renderer(move |ctx, mut viewport| {
            recorded.borrow_mut().push(viewport.ids.this);
            let mut input = egui::RawInput {
                viewport_id: viewport.ids.this,
                focused: false,
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(800.0, 600.0),
                )),
                ..Default::default()
            };
            input.viewports.insert(
                viewport.ids.this,
                egui::ViewportInfo {
                    parent: Some(viewport.ids.parent),
                    focused: Some(false),
                    inner_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(800.0, 600.0),
                    )),
                    outer_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(812.0, 632.0),
                    )),
                    monitor_size: Some(egui::vec2(1920.0, 1080.0)),
                    native_pixels_per_point: Some(1.0),
                    events: if close.get() {
                        vec![egui::ViewportEvent::Close]
                    } else {
                        vec![]
                    },
                    ..Default::default()
                },
            );
            let _ = ctx.run(input, |ctx| (viewport.viewport_ui_cb)(ctx));
        });
        let (commands, mut requests) = tokio::sync::mpsc::channel(16);
        let mut applications = Applications::new(None, uuid::Uuid::new_v4());
        applications.view = true;
        applications.worker = Some(Worker {
            commands,
            state: Arc::new(parking_lot::Mutex::new(State {
                alive: true,
                selected: Some(WindowInfo {
                    id: 42,
                    title: "Synthetic application".into(),
                    width: 300,
                    height: 160,
                    transient_for: None,
                }),
                ..Default::default()
            })),
            cancel: tokio_util::sync::CancellationToken::new(),
        });
        for selected_tab in ["remote desktop", "SSH + X11", "remote desktop"] {
            seen.borrow_mut().clear();
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.label(selected_tab);
                });
                applications.ui(ctx);
            });
            assert_eq!(*seen.borrow(), vec![applications.id]);
            assert!(applications.view);
        }
        // A menu extending beyond the client is a separate native viewport,
        // not an enlarged application image. It disappears with the next state.
        let popup = wire::Popup {
            window: 77,
            x: 290,
            y: 20,
            width: 80,
            height: 120,
            tiles: Vec::new(),
        };
        applications.worker.as_ref().unwrap().state.lock().popups = vec![worker::PopupState {
            info: popup,
            image: Arc::new(egui::ColorImage::new(
                [80, 120],
                vec![egui::Color32::GREEN; 80 * 120],
            )),
        }];
        seen.borrow_mut().clear();
        let _ = ctx.run(Default::default(), |ctx| applications.ui(ctx));
        assert_eq!(
            *seen.borrow(),
            vec![
                applications.id,
                egui::ViewportId::from_hash_of(("application-menu", applications.id, 77_u32))
            ]
        );
        applications
            .worker
            .as_ref()
            .unwrap()
            .state
            .lock()
            .popups
            .clear();
        seen.borrow_mut().clear();
        let _ = ctx.run(Default::default(), |ctx| applications.ui(ctx));
        assert_eq!(*seen.borrow(), vec![applications.id]);
        assert!(applications.popup_textures.is_empty());
        closing.set(true);
        let _ = ctx.run(egui::RawInput::default(), |ctx| applications.ui(ctx));
        assert!(!applications.view);
        assert!(applications.worker.is_none());
        assert!(matches!(requests.try_recv().unwrap(), Request::Stop));
    }
}
