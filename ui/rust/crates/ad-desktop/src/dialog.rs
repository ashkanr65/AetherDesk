//! Default form actions are local to the focused dialog. Never consume Enter
//! from a terminal, another window, a menu, or an IME composition.
use eframe::egui;
pub fn primary(ui: &egui::Ui) -> bool {
    let ctx = ui.ctx();
    if !ui.is_enabled() || !ctx.input(|i| i.focused) || egui::Popup::is_any_open(ctx) {
        return false;
    }
    let layer = ctx
        .memory(|m| m.focused())
        .and_then(|id| ctx.read_response(id))
        .map(|r| r.layer_id);
    if layer != Some(ui.layer_id()) {
        return false;
    }
    if ctx.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Ime(_)))) {
        return false;
    }
    let pressed = ctx.input(|i| {
        i.events.iter().any(|e| {
            matches!(e,
        egui::Event::Key { key: egui::Key::Enter, pressed: true, repeat: false, modifiers, .. }
            if *modifiers == egui::Modifiers::NONE)
        })
    });
    pressed && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter))
}
pub fn focus_on_open(ui: &egui::Ui, response: &egui::Response) {
    if !response.enabled() {
        return;
    }
    let key = ui.layer_id().id.with("default-dialog-focus");
    let frame = ui.ctx().cumulative_frame_nr();
    let newly_open = ui.ctx().data_mut(|d| {
        let previous = d.get_temp::<u64>(key);
        d.insert_temp(key, frame);
        previous.is_none_or(|previous| previous.saturating_add(1) < frame)
    });
    if newly_open {
        response.request_focus();
    }
}
pub fn field_submit(ui: &egui::Ui, response: &egui::Response) -> bool {
    ui.is_enabled()
        && response.enabled()
        && response.lost_focus()
        && ui.input(|i| {
            i.focused
                && i.modifiers == egui::Modifiers::NONE
                && i.key_pressed(egui::Key::Enter)
                && !i.events.iter().any(|e| {
                    matches!(
                        e,
                        egui::Event::Ime(_) | egui::Event::PointerButton { pressed: true, .. }
                    )
                })
        })
        && !egui::Popup::is_any_open(ui.ctx())
        && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn enter_submits_only_the_focused_dialog_and_is_consumed() {
        let ctx = egui::Context::default();
        let mut text = String::new();
        let mut render = |events| {
            let mut submitted = false;
            let mut other = false;
            ctx.memory_mut(|m| m.request_focus(egui::Id::new("first-input")));
            let _ = ctx.run(
                egui::RawInput {
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::Window::new("first").show(ctx, |ui| {
                        submitted = primary(ui);
                        let response = ui.add(
                            egui::TextEdit::singleline(&mut text).id(egui::Id::new("first-input")),
                        );
                        focus_on_open(ui, &response);
                    });
                    egui::Window::new("second").show(ctx, |ui| {
                        other = primary(ui);
                        ui.label("unfocused form");
                    });
                },
            );
            (submitted, other)
        };
        render(vec![]);
        let key = |modifiers, pressed| egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers,
        };
        assert_eq!(
            render(vec![key(egui::Modifiers::CTRL, true)]),
            (false, false)
        );
        assert!(
            ctx.input(|i| i.key_pressed(egui::Key::Enter)),
            "modified Enter remains available to its field"
        );
        render(vec![key(egui::Modifiers::CTRL, false)]);
        assert_eq!(
            render(vec![
                egui::Event::Ime(egui::ImeEvent::Enabled),
                key(egui::Modifiers::NONE, true)
            ]),
            (false, false)
        );
        render(vec![
            key(egui::Modifiers::NONE, false),
            egui::Event::Ime(egui::ImeEvent::Disabled),
        ]);
        let (submitted, other) = render(vec![key(egui::Modifiers::NONE, true)]);
        assert!(submitted);
        assert!(!other);
        assert!(!ctx.input(|i| i.key_pressed(egui::Key::Enter)));
        // egui derives repeat from its held-key set, even when RawInput says
        // repeat=false. A held Enter must not submit the same form twice.
        assert_eq!(
            render(vec![key(egui::Modifiers::NONE, true)]),
            (false, false)
        );
        assert!(ctx.input(|i| i.events.iter().any(|e| matches!(
            e,
            egui::Event::Key {
                key: egui::Key::Enter,
                pressed: true,
                repeat: true,
                ..
            }
        ))));
        render(vec![key(egui::Modifiers::NONE, false)]);
        // Releasing and pressing Enter again is a separate allowed action.
        let (submitted, other) = render(vec![key(egui::Modifiers::NONE, true)]);
        drop(render);
        assert!(submitted);
        assert!(!other);
        assert!(!ctx.input(|i| i.key_pressed(egui::Key::Enter)));
    }
}
