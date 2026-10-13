//! Native terminal screen: byte-stream parsing is independent of UI repaint.
use eframe::egui::{self, Color32, Event, Key, Modifiers};

#[derive(Default)]
pub struct Replies(Vec<u8>);
impl vt100::Callbacks for Replies {
    fn unhandled_csi(&mut self, screen: &mut vt100::Screen, i1: Option<u8>, i2: Option<u8>, params: &[&[u16]], c: char) {
        if self.0.len() > 4096 || i1.is_some() || i2.is_some() { return; }
        match (c, params) {
            ('n', [param]) if *param == [5] => self.0.extend_from_slice(b"\x1b[0n"),
            ('n', [param]) if *param == [6] => {
                let (row, col) = screen.cursor_position();
                self.0.extend_from_slice(format!("\x1b[{};{}R", row + 1, col + 1).as_bytes());
            }
            ('c', _) => self.0.extend_from_slice(b"\x1b[?1;2c"),
            _ => {}
        }
    }
    // OSC 52 clipboard access remains disabled; host output is not permission
    // to read/write the Windows clipboard or run a local command.
}

pub struct Terminal {
    parser: vt100::Parser<Replies>,
    selection: Option<((u16,u16), (u16,u16))>,
    font_size: f32,
}
pub struct TerminalAction { pub input: Vec<u8>, pub size: Option<(u16,u16)> }
impl Default for Terminal {
    fn default() -> Self {
        Self { parser: vt100::Parser::new_with_callbacks(24,80,10_000,Replies::default()), selection: None, font_size: 15.0 }
    }
}
impl Terminal {
    pub fn process(&mut self, bytes: &[u8]) -> Vec<u8> {
        self.parser.process(bytes);
        std::mem::take(&mut self.parser.callbacks_mut().0)
    }
    pub fn selected_text(&self) -> Option<String> {
        self.selection.map(|(a,b)| {
            let (a,b) = if a <= b { (a,b) } else { (b,a) };
            self.parser.screen().contents_between(a.0,a.1,b.0,b.1.saturating_add(1))
        })
    }
    pub fn ui(&mut self, ui: &mut egui::Ui, enabled: bool) -> TerminalAction {
        let font = egui::FontId::monospace(self.font_size);
        let cell_width = ui.fonts_mut(|fonts| fonts.glyph_width(&font,'M')).max(1.0);
        let cell_height = (self.font_size * 1.4).ceil();
        let available = ui.available_size();
        let cols = (available.x / cell_width).floor().clamp(2.0,500.0) as u16;
        let rows = (available.y / cell_height).floor().clamp(2.0,250.0) as u16;
        let mut action = TerminalAction { input: Vec::new(), size: None };
        if self.parser.screen().size() != (rows,cols) {
            self.parser.screen_mut().set_size(rows,cols);
            self.selection = None;
            action.size = Some((cols,rows));
        }
        let (rect, response) = ui.allocate_exact_size(egui::vec2(available.x, available.y.max(2.0)),egui::Sense::click_and_drag());
        if response.clicked() || response.drag_started() { response.request_focus(); }
        let cell_at = |pos: egui::Pos2| {
            (((pos.y - rect.top()) / cell_height).floor().clamp(0.0,f32::from(rows-1)) as u16,
             ((pos.x - rect.left()) / cell_width).floor().clamp(0.0,f32::from(cols-1)) as u16)
        };
        if response.drag_started() {
            if let Some(pos) = response.interact_pointer_pos() { let cell=cell_at(pos); self.selection=Some((cell,cell)); }
        }
        if response.dragged() {
            if let (Some((_,end)),Some(pos)) = (self.selection.as_mut(),response.interact_pointer_pos()) { *end=cell_at(pos); }
        }
        if response.clicked() { self.selection=None; }
        if response.hovered() {
            let scroll = ui.input(|i| i.raw_scroll_delta.y);
            if scroll.abs() >= 1.0 {
                let old = self.parser.screen().scrollback();
                let offset = if scroll > 0.0 { old.saturating_add(3) } else { old.saturating_sub(3) };
                self.parser.screen_mut().set_scrollback(offset);
                self.selection=None;
            }
        }
        if response.has_focus() && enabled {
            ui.memory_mut(|m| m.set_focus_lock_filter(response.id, egui::EventFilter {
                tab:true, horizontal_arrows:true, vertical_arrows:true, escape:true,
            }));
            let modifiers=ui.input(|i|i.modifiers);
            let events=ui.input(|i| i.events.clone());
            let has_copy=events.iter().any(|e|matches!(e,Event::Copy));
            let has_cut=events.iter().any(|e|matches!(e,Event::Cut));
            let has_paste=events.iter().any(|e|matches!(e,Event::Paste(_)));
            for event in events {
                match event {
                    Event::Text(text) if !modifiers.ctrl && !modifiers.command => {
                        // Alt-modified letters are encoded by Key below. AltGr
                        // printable text is handled by the platform Text event.
                        if !modifiers.alt { action.input.extend(text.as_bytes()); }
                    }
                    Event::Ime(egui::ImeEvent::Commit(text)) => action.input.extend(text.as_bytes()),
                    Event::Paste(text) => action.input.extend(paste_bytes(&text,self.parser.screen().bracketed_paste())),
                    Event::Copy => {
                        if let Some(text)=self.selected_text().filter(|s|!s.is_empty()) { ui.ctx().copy_text(text); }
                        else if !modifiers.shift { action.input.push(3); }
                    }
                    Event::Cut => action.input.push(24),
                    Event::Key {key,pressed:true,modifiers,..} => {
                        if (key==Key::C && has_copy)||(key==Key::X && has_cut)||(key==Key::V && has_paste) { continue; }
                        if modifiers.ctrl && modifiers.shift && key==Key::A {
                            self.selection=Some(((0,0),(rows-1,cols-1)));
                        } else if modifiers.ctrl && modifiers.shift && key==Key::C {
                            if let Some(text)=self.selected_text() {ui.ctx().copy_text(text);}
                        } else if let Some(bytes)=key_bytes(key,modifiers,self.parser.screen().application_cursor()) {
                            action.input.extend(bytes);
                        }
                    }
                    _=>{}
                }
            }
            // Prevent Tab/arrows from navigating widgets after sending to PTY.
            ui.input_mut(|i|i.events.retain(|e|!matches!(e,Event::Key{..}|Event::Text(_)|Event::Paste(_)|Event::Copy|Event::Cut|Event::Ime(_))));
            if !action.input.is_empty() { self.parser.screen_mut().set_scrollback(0); self.selection=None; }
        }
        if response.secondary_clicked() {
            if let Some(text)=self.selected_text().filter(|s|!s.is_empty()) { ui.ctx().copy_text(text); self.selection=None; }
        }
        let painter=ui.painter().with_clip_rect(rect);
        painter.rect_filled(rect,0.0,Color32::from_rgb(17,19,21));
        let screen=self.parser.screen();
        for row in 0..rows {
            for col in 0..cols {
                let Some(cell)=screen.cell(row,col) else {continue;};
                let point=rect.min+egui::vec2(f32::from(col)*cell_width,f32::from(row)*cell_height);
                let width=if cell.is_wide() {cell_width*2.0} else {cell_width};
                let cell_rect=egui::Rect::from_min_size(point,egui::vec2(width,cell_height));
                let mut foreground=color(cell.fgcolor(),Color32::from_rgb(231,231,231));
                let mut background=color(cell.bgcolor(),Color32::from_rgb(17,19,21));
                if cell.inverse() {std::mem::swap(&mut foreground,&mut background);}
                if let Some((a,b))=self.selection {
                    let (a,b)=if a<=b {(a,b)}else{(b,a)};
                    if a<=(row,col)&&(row,col)<=b {background=Color32::from_rgb(199,162,179);foreground=Color32::from_rgb(44,43,63);}
                }
                if cell.is_wide_continuation() {continue;}
                painter.rect_filled(cell_rect,0.0,background);
                if cell.dim() {foreground=foreground.gamma_multiply(0.65);}
                let text=cell.contents();
                painter.text(point,egui::Align2::LEFT_TOP,text,font.clone(),foreground);
                if cell.bold() {painter.text(point+egui::vec2(0.5,0.0),egui::Align2::LEFT_TOP,text,font.clone(),foreground);}
                if cell.underline() {painter.line_segment([cell_rect.left_bottom(),cell_rect.right_bottom()],egui::Stroke::new(1.0,foreground));}
            }
        }
        if response.has_focus() && !screen.hide_cursor() && screen.scrollback()==0 {
            let (row,col)=screen.cursor_position();
            let point=rect.min+egui::vec2(f32::from(col)*cell_width,f32::from(row)*cell_height);
            painter.rect_stroke(egui::Rect::from_min_size(point,egui::vec2(cell_width,cell_height)),0.0,egui::Stroke::new(1.0,Color32::WHITE),egui::StrokeKind::Inside);
        }
        action
    }
}
fn paste_bytes(text: &str, bracketed: bool) -> Vec<u8> {
    // Paste is text, not a source of escape sequences or bracket delimiters.
    let text=text.replace('\x1b',"").replace('\0',"");
    if bracketed {format!("\x1b[200~{text}\x1b[201~").into_bytes()}
    else {text.replace("\r\n","\n").replace('\n',"\r").into_bytes()}
}
fn key_bytes(key:Key,m:Modifiers,application:bool)->Option<Vec<u8>> {
    if m.ctrl {
        let value=match key {
            Key::A=>1,Key::B=>2,Key::C=>3,Key::D=>4,Key::E=>5,Key::F=>6,Key::G=>7,
            Key::H=>8,Key::I=>9,Key::J=>10,Key::K=>11,Key::L=>12,Key::M=>13,Key::N=>14,
            Key::O=>15,Key::P=>16,Key::Q=>17,Key::R=>18,Key::S=>19,Key::T=>20,
            Key::U=>21,Key::V=>22,Key::W=>23,Key::X=>24,Key::Y=>25,Key::Z=>26,
            Key::Space|Key::Num2=>0,Key::OpenBracket=>27,Key::Backslash=>28,
            Key::CloseBracket=>29,Key::Num6=>30,Key::Minus=>31,_=>255,
        };
        if value!=255 {let mut bytes=vec![value];if m.alt {bytes.insert(0,27);}return Some(bytes);}
    }
    let modifier=1+u8::from(m.shift)+2*u8::from(m.alt)+4*u8::from(m.ctrl);
    let cursor=match key {Key::ArrowUp=>Some('A'),Key::ArrowDown=>Some('B'),Key::ArrowRight=>Some('C'),Key::ArrowLeft=>Some('D'),Key::Home=>Some('H'),Key::End=>Some('F'),_=>None};
    if let Some(c)=cursor {return Some(if modifier!=1 {format!("\x1b[1;{modifier}{c}")}else if application {format!("\x1bO{c}")}else {format!("\x1b[{c}")}.into_bytes());}
    let tilde=match key {Key::Insert=>Some(2),Key::Delete=>Some(3),Key::PageUp=>Some(5),Key::PageDown=>Some(6),Key::F5=>Some(15),Key::F6=>Some(17),Key::F7=>Some(18),Key::F8=>Some(19),Key::F9=>Some(20),Key::F10=>Some(21),Key::F11=>Some(23),Key::F12=>Some(24),_=>None};
    if let Some(n)=tilde {return Some(if modifier==1 {format!("\x1b[{n}~")}else{format!("\x1b[{n};{modifier}~")}.into_bytes());}
    let mut bytes=match key {
        Key::Enter=>vec![13],Key::Backspace=>vec![127],Key::Tab=>if m.shift {b"\x1b[Z".to_vec()}else{vec![9]},Key::Escape=>vec![27],
        Key::F1=>b"\x1bOP".to_vec(),Key::F2=>b"\x1bOQ".to_vec(),Key::F3=>b"\x1bOR".to_vec(),Key::F4=>b"\x1bOS".to_vec(),
        _ if m.alt && !m.ctrl => {
            let name=key.name();
            if name.len()!=1 {return None;}
            if m.shift {name.to_uppercase().into_bytes()}else{name.to_lowercase().into_bytes()}
        }
        _=>return None,
    };
    if m.alt {bytes.insert(0,27);} Some(bytes)
}
fn color(value:vt100::Color,default:Color32)->Color32 {
    match value {
        vt100::Color::Default=>default,
        vt100::Color::Rgb(r,g,b)=>Color32::from_rgb(r,g,b),
        vt100::Color::Idx(index)=>{
            const ANSI:[u32;16]=[0x171717,0xe24848,0x33c266,0xd9a72e,0x4b8fe2,0xc678dd,0x46b8c7,0xd8d8d8,0x73777e,0xff5f5f,0x55e182,0xffc857,0x76aeed,0xd99ae8,0x72d5df,0xffffff];
            if index<16 {let c=ANSI[usize::from(index)];Color32::from_rgb((c>>16)as u8,(c>>8)as u8,c as u8)}
            else if index<232 {let n=index-16;let part=|n|if n==0 {0}else{55+40*n};Color32::from_rgb(part(n/36),part(n/6%6),part(n%6))}
            else {Color32::from_gray(8+(index-232)*10)}
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn byte_boundaries_and_ansi_colours_survive_output_chunks() {
        let mut terminal=Terminal::default();
        for b in "\x1b[31mé\x1b[0m".as_bytes() {terminal.process(&[*b]);}
        assert_eq!(terminal.parser.screen().cell(0,0).unwrap().contents(),"é");
        assert_eq!(terminal.parser.screen().cell(0,0).unwrap().fgcolor(),vt100::Color::Idx(1));
    }
    #[test]
    fn readline_and_fullscreen_application_keys_are_sent_to_host() {
        assert_eq!(key_bytes(Key::A,Modifiers::CTRL,false),Some(vec![1]));
        assert_eq!(key_bytes(Key::W,Modifiers::CTRL,false),Some(vec![23]));
        assert_eq!(key_bytes(Key::Minus,Modifiers::CTRL,true),Some(vec![31]));
        assert_eq!(key_bytes(Key::ArrowUp,Modifiers::NONE,true),Some(b"\x1bOA".to_vec()));
        assert_eq!(key_bytes(Key::F,Modifiers::ALT,false),Some(b"\x1bf".to_vec()));
    }
    #[test]
    fn terminal_queries_are_answered_without_reading_clipboard() {
        let mut terminal=Terminal::default();
        assert_eq!(terminal.process(b"\x1b[6n"),b"\x1b[1;1R");
        assert!(terminal.process(b"\x1b]52;c;?\x07").is_empty());
        assert_eq!(paste_bytes("a\x1b[201~b",true),b"\x1b[200~a[201~b\x1b[201~");
    }
}
