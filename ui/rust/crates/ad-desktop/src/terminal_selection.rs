//! Selection coordinates are relative to the live screen, including negative
//! history rows. Moving the viewport never moves the selected text.
use eframe::egui;
pub(super) type Cell = (i64, u16);
pub(super) fn cell(visible: (u16, u16), offset: usize) -> Cell {
    (i64::from(visible.0) - offset as i64, visible.1)
}
pub(super) fn edge_lines(
    credit: &mut f32,
    rect: egui::Rect,
    pointer: egui::Pos2,
    line_height: f32,
    dt: f32,
) -> i32 {
    let band = line_height.max(1.0).min(rect.height() / 3.0);
    let speed = if pointer.y < rect.top() + band {
        ((rect.top() + band - pointer.y) / band).clamp(0.0, 20.0) * 12.0
    } else if pointer.y > rect.bottom() - band {
        -((pointer.y - rect.bottom() + band) / band).clamp(0.0, 20.0) * 12.0
    } else {
        0.0
    };
    if speed == 0.0 || credit.signum() != speed.signum() {
        *credit = 0.0;
    }
    *credit += speed * dt.clamp(0.0, 0.1);
    let lines = credit.trunc() as i32;
    *credit -= lines as f32;
    lines
}
pub(super) fn copy(screen: &mut vt100::Screen, range: (Cell, Cell), history: usize) -> String {
    let (mut a, mut b) = if range.0 <= range.1 {
        range
    } else {
        (range.1, range.0)
    };
    let (rows, cols) = screen.size();
    if b.0 < -(history as i64) || a.0 >= i64::from(rows) {
        return String::new();
    }
    if a.0 < -(history as i64) {
        a = (-(history as i64), 0);
    }
    if b.0 >= i64::from(rows) {
        b = (i64::from(rows) - 1, cols - 1);
    }
    let offset = screen.scrollback();
    let mut text = String::new();
    let mut row = a.0;
    let mut col = a.1;
    while row <= b.0 {
        screen.set_scrollback(if row < 0 { (-row) as usize } else { 0 });
        let first = (row + screen.scrollback() as i64) as u16;
        let last = b.0.min(row + i64::from(rows - first) - 1);
        let end = first + (last - row) as u16;
        if last == b.0 {
            text.push_str(&screen.contents_between(first, col, end, b.1.saturating_add(1)));
        } else {
            // One-past-row/column-zero preserves newline vs soft-wrap semantics
            // across viewport-sized chunks without cloning the whole history.
            text.push_str(&screen.contents_between(first, col, end + 1, 0));
        }
        row = last + 1;
        col = 0;
    }
    screen.set_scrollback(offset);
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn history_selection_copies_multiple_pages_in_both_directions_and_restores_view() {
        let mut parser = vt100::Parser::new(3, 12, 100);
        parser.process(b"zero\r\none\r\ntwo\r\nthree\r\nfour\r\nfive");
        let screen = parser.screen_mut();
        screen.set_scrollback(2);
        for range in [((-3, 1), (2, 2)), ((2, 2), (-3, 1))] {
            assert_eq!(copy(screen, range, 3), "ero\none\ntwo\nthree\nfour\nfiv");
            assert_eq!(screen.scrollback(), 2);
        }
    }
    #[test]
    fn history_selection_preserves_soft_wrap_unicode_and_blank_rows() {
        let mut parser = vt100::Parser::new(2, 6, 100);
        parser.process("abcdefghi\r\n\r\n界é\r\nlast".as_bytes());
        let screen = parser.screen_mut();
        screen.set_scrollback(usize::MAX);
        let history = screen.scrollback();
        let text = copy(screen, ((-(history as i64), 0), (1, 3)), history);
        assert_eq!(text, "abcdefghi\n\n界é\nlast");
        assert_eq!(screen.scrollback(), history);
    }
    #[test]
    fn edge_selection_scrolling_is_time_based_bounded_and_reverses_without_momentum() {
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 200.0));
        let run = |fps: u32, y: f32| {
            let mut credit = 0.0;
            (0..fps)
                .map(|_| {
                    edge_lines(
                        &mut credit,
                        rect,
                        egui::pos2(10.0, y),
                        20.0,
                        1.0 / fps as f32,
                    )
                })
                .sum::<i32>()
        };
        assert!((run(30, -20.0) - run(120, -20.0)).abs() <= 1);
        assert!(
            run(60, -100.0) > run(60, -5.0) * 3,
            "distance must increase selection scrolling speed"
        );
        let mut credit = 0.9;
        assert!(edge_lines(&mut credit, rect, egui::pos2(10.0, 300.0), 20.0, 0.1) < 0);
        assert_eq!(
            edge_lines(&mut credit, rect, egui::pos2(10.0, 100.0), 20.0, 0.1),
            0
        );
        assert_eq!(credit, 0.0);
    }
}
