//! Character-based chat selection survives clipping, wheel and edge scrolling.
use eframe::egui::{self, epaint};
use std::sync::Arc;

#[derive(Clone, Copy)]
struct Cursor {
    id: egui::Id,
    character: usize,
}
#[derive(Clone, Default)]
pub(super) struct Selection {
    anchor: Option<Cursor>,
    focus: Option<Cursor>,
    pub(super) dragging: bool,
}
struct Block {
    id: egui::Id,
    rect: egui::Rect,
    clip: egui::Rect,
    galley: Arc<egui::Galley>,
    layer: egui::LayerId,
    shape: egui::layers::ShapeIdx,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn edge_scroll_speed_is_time_based_and_increases_with_distance() {
        let rect = egui::Rect::from_min_size(egui::pos2(10.0, 30.0), egui::vec2(250.0, 200.0));
        assert_eq!(edge_delta(rect, rect.center(), 0.1), 0.0);
        let near = edge_delta(rect, egui::pos2(20.0, rect.bottom() + 5.0), 0.1);
        let far = edge_delta(rect, egui::pos2(20.0, rect.bottom() + 80.0), 0.1);
        assert!(far < near && near < 0.0);
        assert!(edge_delta(rect, egui::pos2(20.0, rect.top() - 80.0), 0.1) > 0.0);
        assert_eq!(
            far,
            2.0 * edge_delta(rect, egui::pos2(20.0, rect.bottom() + 80.0), 0.05)
        );
        assert_eq!(
            far,
            edge_delta(rect, egui::pos2(20.0, rect.bottom() + 80.0), 100.0)
        );
    }
    #[test]
    fn drag_wheel_and_edge_scrolling_keep_an_offscreen_chat_anchor_and_copy_unicode() {
        let ctx = egui::Context::default();
        let mut selection = Selection::default();
        let mut time = 0.0;
        let mut render = |events: Vec<egui::Event>, focused: bool| {
            time += 0.016;
            let mut offset = 0.0;
            let mut viewport = egui::Rect::NOTHING;
            let mut first = egui::Pos2::ZERO;
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(350.0, 260.0),
                    )),
                    events,
                    time: Some(time),
                    focused,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        let mut frame =
                            Frame::new(selection.clone(), ui.available_rect_before_wrap());
                        let scroll = egui::ScrollArea::vertical()
                            .id_salt("selection-fixture")
                            .animated(false)
                            .scroll_source(
                                egui::scroll_area::ScrollSource::SCROLL_BAR
                                    | egui::scroll_area::ScrollSource::MOUSE_WHEEL,
                            )
                            .show(ui, |ui| {
                                for index in 0..40 {
                                    ui.push_id(index, |ui| {
                                        frame.text(
                                            ui,
                                            &format!("message {index:02}\nalpha ñ\nomega"),
                                        );
                                        ui.add_space(8.0);
                                    });
                                }
                                first = frame.blocks[0].rect.left_top() + egui::vec2(1.0, 6.0);
                                frame.finish(ui);
                            });
                        offset = scroll.state.offset.y;
                        viewport = scroll.inner_rect;
                        selection = frame.selection;
                    });
                },
            );
            let copied = output
                .platform_output
                .commands
                .into_iter()
                .find_map(|command| match command {
                    egui::OutputCommand::CopyText(text) => Some(text),
                    _ => None,
                });
            (offset, viewport, first, copied, selection.dragging)
        };
        for _ in 0..3 {
            render(vec![], true);
        }
        let (_, viewport, first, _, _) = render(vec![], true);
        let button = |position, pressed| egui::Event::PointerButton {
            pos: position,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        render(
            vec![egui::Event::PointerMoved(first), button(first, true)],
            true,
        );
        let below = egui::pos2(viewport.center().x, viewport.bottom() + 80.0);
        let mut last = render(vec![egui::Event::PointerMoved(below)], true);
        for _ in 0..50 {
            last = render(vec![], true);
        }
        assert!(
            last.0 > 300.0,
            "dragging below the edge did not scroll: {}",
            last.0
        );
        let before_wheel = last.0;
        let middle = viewport.center();
        render(
            vec![
                egui::Event::PointerMoved(middle),
                egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: egui::vec2(0.0, 100.0),
                    modifiers: egui::Modifiers::NONE,
                },
            ],
            true,
        );
        let after_wheel = render(vec![], true);
        assert!(
            after_wheel.0 < before_wheel,
            "wheel must remain usable during selection: {} -> {}",
            before_wheel,
            after_wheel.0
        );
        let copied = render(vec![egui::Event::Copy], true).3.unwrap();
        assert!(
            copied.contains("message 00"),
            "offscreen anchor was lost: {copied}"
        );
        assert!(
            copied.contains("message 05"),
            "selection must span multiple messages/pages: {copied}"
        );
        assert!(
            copied.contains("alpha ñ\nomega"),
            "Unicode and line breaks must survive"
        );
        let above = egui::pos2(viewport.center().x, viewport.top() - 80.0);
        render(vec![egui::Event::PointerMoved(above)], true);
        let mut reversed = after_wheel.0;
        for _ in 0..20 {
            reversed = render(vec![], true).0;
        }
        assert!(
            reversed < after_wheel.0,
            "moving above the edge must reverse scrolling"
        );
        let released = render(vec![button(above, false)], true);
        assert!(!released.4);
        let settled = render(vec![], true).0;
        for _ in 0..4 {
            assert!(
                (render(vec![], true).0 - settled).abs() < 0.1,
                "edge scrolling continued after release"
            );
        }
        assert!(!render(vec![], false).4);
    }
}
pub(super) struct Frame {
    pub(super) selection: Selection,
    viewport: egui::Rect,
    blocks: Vec<Block>,
    pressed_text: bool,
}
pub(super) fn edge_delta(rect: egui::Rect, position: egui::Pos2, dt: f32) -> f32 {
    let band = 20.0_f32.min(rect.height() / 3.0).max(1.0);
    let distance = if position.y < rect.top() + band {
        rect.top() + band - position.y
    } else if position.y > rect.bottom() - band {
        rect.bottom() - band - position.y
    } else {
        0.0
    };
    (distance / band * 240.0).clamp(-2400.0, 2400.0) * dt.clamp(0.0, 0.1)
}
impl Frame {
    pub(super) fn new(selection: Selection, viewport: egui::Rect) -> Self {
        Self {
            selection,
            viewport,
            blocks: Vec::new(),
            pressed_text: false,
        }
    }
    pub(super) fn text(&mut self, ui: &mut egui::Ui, text: &str) {
        let (position, galley, response) = egui::Label::new(text)
            .wrap()
            .selectable(false)
            .sense(egui::Sense::click_and_drag())
            .layout_in_ui(ui);
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Label, ui.is_enabled(), text)
        });
        let rect = egui::Rect::from_min_size(position, galley.size());
        if response.hovered() || (self.selection.dragging && ui.input(|i| i.pointer.primary_down()))
        {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Text);
        }
        if response.contains_pointer()
            && ui.input(|i| i.pointer.button_pressed(egui::PointerButton::Primary))
        {
            if let Some(pointer) = ui.input(|i| i.pointer.interact_pos()) {
                let cursor = Cursor {
                    id: response.id,
                    character: galley.cursor_from_pos(pointer - position).index,
                };
                self.selection.anchor = Some(cursor);
                self.selection.focus = Some(cursor);
                self.selection.dragging = true;
                self.pressed_text = true;
                response.request_focus();
            }
        }
        let shape = ui.painter().add(epaint::TextShape::new(
            position,
            galley.clone(),
            ui.visuals().text_color(),
        ));
        self.blocks.push(Block {
            id: response.id,
            rect,
            clip: ui.clip_rect(),
            galley,
            layer: ui.layer_id(),
            shape,
        });
    }
    fn range(&self) -> Option<((usize, usize), (usize, usize))> {
        let a = self.selection.anchor?;
        let b = self.selection.focus?;
        let a = (
            self.blocks.iter().position(|block| block.id == a.id)?,
            a.character,
        );
        let b = (
            self.blocks.iter().position(|block| block.id == b.id)?,
            b.character,
        );
        Some(if a <= b { (a, b) } else { (b, a) })
    }
    fn selected_text(&self) -> String {
        let Some(((first, start), (last, end))) = self.range() else {
            return String::new();
        };
        let mut parts = Vec::new();
        for index in first..=last {
            let text = &self.blocks[index].galley.job.text;
            let from = if index == first { start } else { 0 };
            let to = if index == last {
                end
            } else {
                text.chars().count()
            };
            parts.push(
                text.chars()
                    .skip(from)
                    .take(to.saturating_sub(from))
                    .collect::<String>(),
            );
        }
        parts.join("\n\n")
    }
    pub(super) fn finish(&mut self, ui: &mut egui::Ui) {
        self.viewport = self.viewport.intersect(ui.clip_rect());
        let (down, released, pressed, pointer, focused, escape, copy, dt) = ui.input(|i| {
            (
                i.pointer.primary_down(),
                i.pointer.button_released(egui::PointerButton::Primary),
                i.pointer.button_pressed(egui::PointerButton::Primary),
                i.pointer.interact_pos(),
                i.focused,
                i.key_pressed(egui::Key::Escape),
                i.events
                    .iter()
                    .any(|event| matches!(event, egui::Event::Copy)),
                i.stable_dt,
            )
        });
        if !focused || escape || (pressed && !self.pressed_text) {
            self.selection = Selection::default();
        }
        if self.selection.dragging && (down || released) {
            if let Some(pointer) = pointer {
                let point = self.viewport.shrink(1.0).clamp(pointer);
                let nearest = self
                    .blocks
                    .iter()
                    .filter(|block| block.rect.intersects(self.viewport.intersect(block.clip)))
                    .min_by(|a, b| {
                        let distance = |block: &Block| {
                            (block.rect.top() - point.y)
                                .max(point.y - block.rect.bottom())
                                .max(0.0)
                        };
                        distance(a).total_cmp(&distance(b))
                    });
                if let Some(block) = nearest {
                    self.selection.focus = Some(Cursor {
                        id: block.id,
                        character: block.galley.cursor_from_pos(point - block.rect.min).index,
                    });
                }
                if down && focused {
                    // egui's ScrollArea intentionally ignores the wheel while
                    // any widget owns a drag. A text-selection drag needs the
                    // wheel, so consume it here exactly once (no smoothing tail
                    // or parent scroll) and keep the character anchor fixed.
                    let wheel = if self.viewport.contains(pointer) {
                        ui.input_mut(|input| {
                            let delta = input.raw_scroll_delta.y;
                            input.raw_scroll_delta.y = 0.0;
                            input.smooth_scroll_delta.y = 0.0;
                            delta
                        })
                    } else {
                        0.0
                    };
                    let delta = if wheel != 0.0 {
                        wheel
                    } else {
                        edge_delta(self.viewport, pointer, dt)
                    };
                    if delta != 0.0 {
                        ui.scroll_with_delta_animation(
                            egui::vec2(0.0, delta),
                            egui::style::ScrollAnimation::none(),
                        );
                        ui.ctx()
                            .request_repaint_after(std::time::Duration::from_millis(16));
                    }
                }
            }
        }
        if released || !down {
            self.selection.dragging = false;
        }
        let Some(((first, start), (last, end))) = self.range() else {
            self.selection = Selection::default();
            return;
        };
        for index in first..=last {
            let block = &self.blocks[index];
            let from = if index == first { start } else { 0 };
            let to = if index == last {
                end
            } else {
                block.galley.job.text.chars().count()
            };
            let range = egui::text_selection::CCursorRange::two(
                egui::text::CCursor::new(from),
                egui::text::CCursor::new(to),
            );
            ui.ctx().graphics_mut(|layers| {
                if let Some(list) = layers.get_mut(block.layer) {
                    list.mutate_shape(block.shape, |shape| {
                        if let epaint::Shape::Text(text) = &mut shape.shape {
                            egui::text_selection::visuals::paint_text_selection(
                                &mut text.galley,
                                ui.visuals(),
                                &range,
                                None,
                            );
                        }
                    });
                }
            });
        }
        if copy {
            let text = self.selected_text();
            if !text.is_empty() {
                ui.ctx().copy_text(text);
            }
        }
    }
}
