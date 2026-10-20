//! Application navigation consumes only the original workspace shortcuts.
use eframe::egui;
/// Bound overflowing controls to the panel's chosen width. Every sidebar uses
/// the same outer SidePanel state; content must not enlarge it on selection.
pub fn sidebar_body<R>(
    ui: &mut egui::Ui,
    section: &str,
    contents: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    let width = ui.available_width();
    egui::ScrollArea::horizontal()
        .id_salt(("sidebar-body", section))
        .max_width(width)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.set_width(width);
            contents(ui)
        })
        .inner
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    NewConnection,
    ToggleSidebar,
    Relative(isize),
}

pub fn shortcut(key: egui::Key, m: egui::Modifiers) -> Option<Action> {
    // AltGr is text input; never reinterpret it as an application shortcut.
    if !m.ctrl || m.alt || m.mac_cmd {
        return None;
    }
    match (key, m.shift) {
        (egui::Key::K, true) => Some(Action::NewConnection),
        (egui::Key::B, true) => Some(Action::ToggleSidebar),
        (egui::Key::Tab, false) | (egui::Key::PageDown, false) => Some(Action::Relative(1)),
        (egui::Key::Tab, true) | (egui::Key::PageUp, false) => Some(Action::Relative(-1)),
        _ => None,
    }
}

pub fn take_shortcuts(ctx: &egui::Context) -> Vec<Action> {
    let bindings = crate::preferences::current().shortcuts;
    ctx.input_mut(|input| {
        let mut actions = Vec::new();
        input.events.retain(|event| {
            if let egui::Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } = event
            {
                if let Some(action) = shortcut_with(*key, *modifiers, &bindings) {
                    actions.push(action);
                    return false;
                }
            }
            true
        });
        actions
    })
}

pub fn shortcut_with(
    key: egui::Key,
    m: egui::Modifiers,
    bindings: &crate::preferences::Shortcuts,
) -> Option<Action> {
    if !m.ctrl || m.alt || m.mac_cmd {
        return None;
    }
    for (binding, action) in [
        (&bindings.new_connection, Action::NewConnection),
        (&bindings.sidebar, Action::ToggleSidebar),
        (&bindings.next_tab, Action::Relative(1)),
        (&bindings.previous_tab, Action::Relative(-1)),
    ] {
        if crate::preferences::key(&binding.key) == Some(key) && binding.shift == m.shift {
            return Some(action);
        }
    }
    // Keep the original paging aliases when using the default tab bindings.
    if *bindings == crate::preferences::Shortcuts::default() {
        shortcut(key, m)
    } else {
        None
    }
}

#[derive(Clone)]
struct TabDrag(uuid::Uuid);
pub struct Tab {
    pub id: uuid::Uuid,
    pub name: String,
    pub detail: String,
    pub state: &'static str,
}
#[derive(Default)]
pub struct Strip {
    revealed: Option<uuid::Uuid>,
    revealed_index: Option<usize>,
}
#[derive(Default)]
pub struct Change {
    pub select: Option<usize>,
    pub close: Option<usize>,
    pub reorder: Option<(usize, usize)>,
}
const TAB_WIDTH: f32 = 190.0;
const TAB_EXTENT: f32 = TAB_WIDTH + 8.0;
fn tab_regions(rect: egui::Rect) -> (egui::Rect, egui::Rect) {
    let close = egui::Rect::from_min_max(
        egui::pos2(rect.right() - 27.0, rect.top() + 3.0),
        egui::pos2(rect.right() - 3.0, rect.bottom() - 3.0),
    );
    let label = egui::Rect::from_min_max(
        rect.min + egui::vec2(10.0, 0.0),
        egui::pos2(close.left() - 2.0, rect.bottom()),
    );
    (label, close)
}
impl Strip {
    pub fn ui(&mut self, ui: &mut egui::Ui, tabs: &[Tab], selected: Option<usize>) -> Change {
        let mut change = Change::default();
        let active = selected.and_then(|i| tabs.get(i)).map(|t| t.id);
        let width = ui.available_width().max(1.0);
        let mut scroll = egui::ScrollArea::horizontal()
            .id_salt("session-tabs")
            .max_width(width)
            .auto_shrink([false, true]);
        if self.revealed != active || self.revealed_index != selected {
            if let Some(index) = selected {
                scroll = scroll.horizontal_scroll_offset(
                    (index as f32 * TAB_EXTENT + TAB_WIDTH / 2.0 - width / 2.0)
                        .clamp(0.0, (tabs.len() as f32 * TAB_EXTENT - width).max(0.0)),
                );
            }
        }
        scroll.show_viewport(ui, |ui, viewport| {
            let (rect, _) = ui.allocate_exact_size(
                egui::vec2(tabs.len() as f32 * TAB_EXTENT, 32.0),
                egui::Sense::hover(),
            );
            let first = (viewport.left() / TAB_EXTENT).floor().max(0.0) as usize;
            let last = ((viewport.right() / TAB_EXTENT).ceil() as usize + 1).min(tabs.len());
            for index in first.min(last)..last {
                let tab = &tabs[index];
                let item = egui::Rect::from_min_size(
                    rect.min + egui::vec2(index as f32 * TAB_EXTENT, 0.0),
                    egui::vec2(TAB_WIDTH, 30.0),
                );
                let (label, close) = tab_regions(item);
                let hovered = ui.rect_contains_pointer(item);
                let colors = crate::theme::colors(ui.visuals().dark_mode);
                let fill = if selected == Some(index) {
                    colors.tab_active
                } else {
                    colors.tab
                };
                ui.painter()
                    .rect_filled(item, 3.0, if hovered { colors.hover } else { fill });
                if hovered {
                    ui.painter().rect_stroke(
                        item,
                        3.0,
                        ui.visuals().widgets.hovered.bg_stroke,
                        egui::StrokeKind::Inside,
                    );
                }
                ui.scope_builder(egui::UiBuilder::new().max_rect(item), |ui| {
                    ui.push_id(tab.id, |ui| {
                        let response = ui.put(
                            label,
                            egui::Button::new(
                                egui::RichText::new(format!("{} · {}", tab.name, tab.state)).color(
                                    if hovered {
                                        colors.hover_text
                                    } else if selected == Some(index) {
                                        colors.tab_active_text
                                    } else {
                                        colors.tab_text
                                    },
                                ),
                            )
                            .truncate()
                            .frame(false)
                            .selected(selected == Some(index))
                            .sense(egui::Sense::click_and_drag()),
                        );
                        if response.clicked() {
                            change.select = Some(index);
                        }
                        if response.middle_clicked() {
                            change.close = Some(index);
                        }
                        if response.drag_started() {
                            response.dnd_set_drag_payload(TabDrag(tab.id));
                        }
                        if let Some(payload) = response.dnd_release_payload::<TabDrag>() {
                            if let Some(from) = tabs.iter().position(|t| t.id == payload.0) {
                                change.reorder = Some((from, index));
                            }
                        }
                        response.on_hover_text(format!(
                            "{}\n{}\nDrag to reorder; middle click to close",
                            tab.name, tab.detail
                        ));
                        let close_response = ui.put(close, egui::Button::new("").frame(false));
                        if close_response.hovered() {
                            ui.painter().rect_filled(
                                close.shrink(1.0),
                                3.0,
                                ui.visuals().widgets.hovered.bg_fill,
                            );
                            ui.painter().rect_stroke(
                                close.shrink(1.0),
                                3.0,
                                ui.visuals().widgets.hovered.bg_stroke,
                                egui::StrokeKind::Inside,
                            );
                        }
                        let c = close.center();
                        let color = if close_response.hovered() {
                            colors.hover_text
                        } else if selected == Some(index) {
                            colors.tab_active_text
                        } else {
                            colors.tab_text
                        };
                        for direction in [-1.0, 1.0] {
                            ui.painter().line_segment(
                                [
                                    c + egui::vec2(-3.5, -3.5 * direction),
                                    c + egui::vec2(3.5, 3.5 * direction),
                                ],
                                egui::Stroke::new(1.3, color),
                            );
                        }
                        close_response.widget_info(|| {
                            egui::WidgetInfo::labeled(
                                egui::WidgetType::Button,
                                ui.is_enabled(),
                                "Close tab",
                            )
                        });
                        if close_response
                            .on_hover_text("Close this tab (remote desktop is retained)")
                            .clicked()
                        {
                            change.close = Some(index);
                        }
                    });
                });
            }
        });
        self.revealed = active;
        self.revealed_index = selected;
        change
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn customized_navigation_shortcuts_preserve_altgr_and_release_unbound_keys() {
        let mut bindings = crate::preferences::Shortcuts::default();
        bindings.new_connection.key = "N".into();
        let m = egui::Modifiers {
            ctrl: true,
            shift: true,
            ..Default::default()
        };
        assert_eq!(
            shortcut_with(egui::Key::N, m, &bindings),
            Some(Action::NewConnection)
        );
        assert_eq!(shortcut_with(egui::Key::K, m, &bindings), None);
        assert_eq!(
            shortcut_with(egui::Key::N, egui::Modifiers { alt: true, ..m }, &bindings),
            None
        );
    }
    #[test]
    fn switching_sidebars_does_not_expand_the_shared_width_to_fit_controls() {
        let ctx = egui::Context::default();
        for section in [
            "Sessions",
            "AI Assistant",
            "Connections",
            "Tools",
            "Sessions",
        ] {
            for pass in 0..3 {
                let _ = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(1200.0, 800.0),
                        )),
                        ..Default::default()
                    },
                    |ctx| {
                        let response = egui::SidePanel::left("shared-sidebar-fixture")
                            .default_width(240.0)
                            .width_range(190.0..=900.0)
                            .show(ctx, |ui| {
                                sidebar_body(ui, section, |ui| {
                                    ui.add_sized([1500.0, 28.0], egui::Button::new(section));
                                });
                            });
                        if pass == 2 {
                            assert!(
                                (response.response.rect.width() - 240.0).abs() < 1.0,
                                "sidebar changed size on {section}"
                            );
                        }
                    },
                );
            }
        }
    }
    #[test]
    fn close_is_inside_fixed_size_tab_and_does_not_select_it() {
        let ctx = egui::Context::default();
        let mut strip = Strip::default();
        let tabs = vec![Tab {
            id: uuid::Uuid::new_v4(),
            name: "A very long connection name".into(),
            detail: "server".into(),
            state: "Disconnected",
        }];
        let mut last = Change::default();
        let mut render = |events| {
            ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(800.0, 300.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        last = strip.ui(ui, &tabs, None);
                    });
                },
            )
        };
        let output = render(vec![]);
        let rect = output
            .shapes
            .iter()
            .find_map(|s| match &s.shape {
                egui::epaint::Shape::Rect(r)
                    if r.rect.width() == TAB_WIDTH && r.rect.height() == 30.0 =>
                {
                    Some(r.rect)
                }
                _ => None,
            })
            .expect("fixed-size tab background");
        let (label, close) = tab_regions(rect);
        assert!(rect.contains_rect(close));
        assert!(!label.intersects(close));
        let point = close.center();
        let _ = render(vec![
            egui::Event::PointerMoved(point),
            egui::Event::PointerButton {
                pos: point,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
        ]);
        let _ = render(vec![egui::Event::PointerButton {
            pos: point,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        }]);
        drop(render);
        assert_eq!(last.close, Some(0));
        assert_eq!(last.select, None);
    }
    #[test]
    fn workspace_shortcuts_preserve_readline_altgr_and_editor_keys() {
        let ctrl = egui::Modifiers {
            ctrl: true,
            ..Default::default()
        };
        assert_eq!(shortcut(egui::Key::Tab, ctrl), Some(Action::Relative(1)));
        assert_eq!(
            shortcut(
                egui::Key::Tab,
                egui::Modifiers {
                    shift: true,
                    ..ctrl
                }
            ),
            Some(Action::Relative(-1))
        );
        assert_eq!(
            shortcut(egui::Key::PageUp, ctrl),
            Some(Action::Relative(-1))
        );
        for key in [
            egui::Key::A,
            egui::Key::E,
            egui::Key::W,
            egui::Key::K,
            egui::Key::C,
            egui::Key::V,
            egui::Key::S,
        ] {
            assert_eq!(shortcut(key, ctrl), None);
        }
        assert_eq!(
            shortcut(
                egui::Key::B,
                egui::Modifiers {
                    alt: true,
                    shift: true,
                    ..ctrl
                }
            ),
            None
        );
    }
    #[test]
    fn consumed_workspace_key_is_not_forwarded_to_the_terminal() {
        let ctx = egui::Context::default();
        let modifiers = egui::Modifiers {
            ctrl: true,
            ..Default::default()
        };
        let _ = ctx.run(
            egui::RawInput {
                events: vec![
                    egui::Event::Key {
                        key: egui::Key::Tab,
                        physical_key: Some(egui::Key::Tab),
                        pressed: true,
                        repeat: false,
                        modifiers,
                    },
                    egui::Event::Key {
                        key: egui::Key::E,
                        physical_key: Some(egui::Key::E),
                        pressed: true,
                        repeat: false,
                        modifiers,
                    },
                ],
                ..Default::default()
            },
            |ctx| {
                assert_eq!(take_shortcuts(ctx), vec![Action::Relative(1)]);
                assert_eq!(ctx.input(|i| i.events.len()), 1);
                assert!(take_shortcuts(ctx).is_empty());
            },
        );
    }
}
