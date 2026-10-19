//! Native editor operations. Ranges are UTF-8 bytes internally and egui characters
//! at the UI boundary; LSP positions are strictly UTF-16. No operation writes files.
use ad_desktop_model::files::MAX_EDITABLE_BYTES;
use eframe::egui::{self, text::LayoutJob, Color32, FontId, TextFormat};
use regex::{Regex, RegexBuilder};
use serde_json::Value;
use std::{ops::Range, sync::OnceLock};
use syntect::{easy::HighlightLines, highlighting::ThemeSet, parsing::SyntaxSet};

pub const LANGUAGES: &[(&str, &str)] = &[
    ("Plain text", "txt"),
    ("Bash / Shell", "sh"),
    ("C", "c"),
    ("C++", "cpp"),
    ("CSS", "css"),
    ("Go", "go"),
    ("HTML", "html"),
    ("INI", "ini"),
    ("Java", "java"),
    ("JavaScript", "js"),
    ("JSON", "json"),
    ("Markdown", "md"),
    ("PowerShell", "ps1"),
    ("Python", "py"),
    ("Rust", "rs"),
    ("SQL", "sql"),
    ("SystemVerilog", "sv"),
    ("Tcl", "tcl"),
    ("TOML", "toml"),
    ("TypeScript", "ts"),
    ("Verilog", "v"),
    ("XML", "xml"),
    ("YAML", "yaml"),
];
pub fn language(path: &str) -> String {
    let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "h" | "hpp" | "cc" => "cpp",
        "bash" => "sh",
        "yml" => "yaml",
        _ => &ext,
    }
    .to_owned()
}
pub fn language_id(ext: &str) -> &str {
    match ext {
        "py" => "python",
        "rs" => "rust",
        "js" => "javascript",
        "ts" => "typescript",
        "v" => "verilog",
        "sv" => "systemverilog",
        "ps1" => "powershell",
        "sh" => "shell",
        "md" => "markdown",
        "txt" => "plaintext",
        other if LANGUAGES.iter().any(|(_, ext)| *ext == other) => other,
        _ => "plaintext",
    }
}
pub fn byte_at(text: &str, character: usize) -> usize {
    text.char_indices()
        .nth(character)
        .map_or(text.len(), |(byte, _)| byte)
}
pub fn characters(text: &str, byte: usize) -> usize {
    text[..byte].chars().count()
}
pub fn checked(text: String) -> Result<String, String> {
    if text.len() > MAX_EDITABLE_BYTES {
        Err("Edit exceeds the 8 MiB limit".into())
    } else {
        Ok(text)
    }
}

#[derive(Default, Clone, PartialEq, Eq)]
pub struct SearchOptions {
    pub case: bool,
    pub word: bool,
    pub regex: bool,
    pub selection: bool,
}
pub fn expression(query: &str, options: &SearchOptions) -> Result<Regex, String> {
    let pattern = if options.regex {
        query.to_owned()
    } else {
        regex::escape(query)
    };
    let pattern = if options.word {
        format!(r"\b(?:{pattern})\b")
    } else {
        pattern
    };
    RegexBuilder::new(&pattern)
        .case_insensitive(!options.case)
        .multi_line(true)
        .size_limit(2 * 1024 * 1024)
        .build()
        .map_err(|e| e.to_string())
}
pub fn matches(
    text: &str,
    query: &str,
    options: &SearchOptions,
    scope: Range<usize>,
) -> Result<Vec<Range<usize>>, String> {
    if query.is_empty() {
        return Ok(vec![]);
    }
    let regex = expression(query, options)?;
    // Captures are evaluated against the complete document, so ^/$ and look-around
    // never invent a line boundary at the selected substring's edges.
    let mut found = Vec::new();
    for m in regex.find_iter(text) {
        if m.start() >= scope.start && m.end() <= scope.end {
            found.push(m.start()..m.end());
        }
        if found.len() > 100_000 {
            return Err("More than 100,000 matches; narrow the search".into());
        }
    }
    Ok(found)
}
pub fn replace(
    text: &str,
    query: &str,
    replacement: &str,
    options: &SearchOptions,
    ranges: &[Range<usize>],
) -> Result<String, String> {
    let regex = expression(query, options)?;
    let mut edits = Vec::new();
    for captures in regex.captures_iter(text) {
        let m = captures.get(0).expect("full match");
        if ranges
            .binary_search_by_key(&m.start(), |r| r.start)
            .is_ok_and(|i| ranges[i].end == m.end())
        {
            let mut value = String::new();
            if options.regex {
                captures.expand(replacement, &mut value)
            } else {
                value.push_str(replacement)
            }
            edits.push((m.start()..m.end(), value));
        }
    }
    apply_byte_edits(text, edits)
}
pub fn apply_byte_edits(
    text: &str,
    mut edits: Vec<(Range<usize>, String)>,
) -> Result<String, String> {
    if edits.len() > 100_000 {
        return Err("Too many edits".into());
    }
    edits.sort_by_key(|(r, _)| (r.start, r.end));
    let mut end = 0;
    let mut size = text.len();
    for (i, (r, value)) in edits.iter().enumerate() {
        if r.start > r.end
            || r.end > text.len()
            || !text.is_char_boundary(r.start)
            || !text.is_char_boundary(r.end)
            || (i > 0 && (r.start < end || (r.start == r.end && r.start == edits[i - 1].0.start)))
        {
            return Err("Edits overlap or contain an invalid range; nothing was changed".into());
        }
        end = r.end;
        size = size
            .checked_sub(r.len())
            .and_then(|n| n.checked_add(value.len()))
            .ok_or("Edit size overflow")?;
        if size > MAX_EDITABLE_BYTES {
            return Err("Edit exceeds the 8 MiB limit".into());
        }
    }
    let mut result = text.to_owned();
    for (range, value) in edits.into_iter().rev() {
        result.replace_range(range, &value)
    }
    checked(result)
}
pub fn utf16_offset(text: &str, position: &Value) -> Result<usize, String> {
    let line = position["line"].as_u64().ok_or("Missing line")?;
    let column = position["character"]
        .as_u64()
        .ok_or("Missing UTF-16 column")?;
    let (mut row, mut col) = (0, 0);
    for (byte, c) in text.char_indices() {
        if row == line && col == column {
            return Ok(byte);
        }
        if c == '\n' {
            if row == line {
                return Err("Position exceeds the line".into());
            }
            row += 1;
            col = 0;
        } else if c != '\r' {
            col += c.len_utf16() as u64;
        }
    }
    if row == line && col == column {
        Ok(text.len())
    } else {
        Err("Invalid UTF-16 position".into())
    }
}
pub fn lsp_edits(text: &str, edits: &[Value]) -> Result<String, String> {
    let mut changes = Vec::new();
    for edit in edits {
        changes.push((
            utf16_offset(text, &edit["range"]["start"])?
                ..utf16_offset(text, &edit["range"]["end"])?,
            edit["newText"]
                .as_str()
                .ok_or("Missing edit text")?
                .to_owned(),
        ));
    }
    apply_byte_edits(text, changes)
}
pub fn indent(
    text: &str,
    selected: Range<usize>,
    width: usize,
    tabs: bool,
    outdent: bool,
) -> Result<(String, Range<usize>), String> {
    if selected.start > selected.end
        || selected.end > text.len()
        || !text.is_char_boundary(selected.start)
        || !text.is_char_boundary(selected.end)
    {
        return Err("Selection is no longer valid".into());
    }
    let first = text[..selected.start].rfind('\n').map_or(0, |n| n + 1);
    let stop = if selected.end > selected.start && text[..selected.end].ends_with('\n') {
        selected.end - 1
    } else {
        selected.end
    };
    let last = text[stop..].find('\n').map_or(text.len(), |n| stop + n);
    let unit = if tabs {
        "\t".to_owned()
    } else {
        " ".repeat(width.clamp(1, 8))
    };
    let mut edits = Vec::new();
    let mut offset = first;
    for line in text[first..last].split_inclusive('\n') {
        if outdent {
            let count = if line.starts_with('\t') {
                1
            } else {
                line.chars().take(width).take_while(|c| *c == ' ').count()
            };
            if count > 0 {
                edits.push((offset..offset + count, String::new()));
            }
        } else {
            edits.push((offset..offset, unit.clone()));
        }
        offset += line.len();
    }
    if first == last && !outdent {
        edits.push((first..first, unit));
    }
    let result = apply_byte_edits(text, edits)?;
    let new_last =
        (last as isize + result.len() as isize - text.len() as isize).max(first as isize) as usize;
    Ok((result, first..new_last))
}
pub fn newline(
    text: &str,
    range: Range<usize>,
    width: usize,
    tabs: bool,
) -> Result<(String, usize), String> {
    if range.start > range.end
        || range.end > text.len()
        || !text.is_char_boundary(range.start)
        || !text.is_char_boundary(range.end)
    {
        return Err("Selection is no longer valid".into());
    }
    let start = text[..range.start].rfind('\n').map_or(0, |i| i + 1);
    let before = &text[start..range.start];
    let indent = before
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .collect::<String>();
    let mut insert = if text.contains("\r\n") {
        "\r\n".to_owned()
    } else {
        "\n".to_owned()
    };
    insert.push_str(&indent);
    if before.trim_end().ends_with(['{', '[', '(', ':']) {
        insert.push_str(&if tabs {
            "\t".into()
        } else {
            " ".repeat(width.clamp(1, 8))
        });
    }
    let cursor = range.start + insert.len();
    Ok((apply_byte_edits(text, vec![(range, insert)])?, cursor))
}

fn syntaxes() -> &'static SyntaxSet {
    static SET: OnceLock<SyntaxSet> = OnceLock::new();
    SET.get_or_init(|| {
        let defaults=SyntaxSet::load_defaults_newlines();
        let mut builder=defaults.clone().into_builder();
        // Add languages used by hardware/Windows workflows when absent from the
        // standard embedded set. These declarative grammars execute no user code.
        for (name,ext,keywords) in [
            ("Verilog","v","module|endmodule|input|output|wire|reg|always|assign|begin|end|parameter|if|else|case|endcase"),
            ("SystemVerilog","sv","module|endmodule|logic|interface|endinterface|class|endclass|always_ff|always_comb|typedef|enum|struct|package|endpackage|input|output|begin|end|if|else"),
            ("PowerShell","ps1","function|param|if|else|elseif|foreach|while|return|switch|try|catch|finally|throw|class"),
            ("Tcl","tcl","proc|set|if|else|elseif|foreach|while|return|switch|namespace|variable|puts"),
            ("TypeScript","ts","const|let|var|function|class|interface|type|enum|export|import|async|await|return|if|else|extends|implements|public|private"),
            ("TOML","toml","true|false"),
            ("INI","ini","true|false|on|off"),
        ] {
            if defaults.find_syntax_by_extension(ext).is_some(){continue}
            let comment=if matches!(ext,"v"|"sv"|"ts"){r"//.*$"}else{r"#.*$"};
            let single_quote=if matches!(ext,"v"|"sv"){r"(?!)"}else{"'"};
            let grammar=format!(r#"%YAML 1.2
---
name: {name}
file_extensions: [{ext}]
scope: source.{ext}
contexts:
  main:
    - match: '{comment}'
      scope: comment.line
    - match: '/\*'
      push: comment
    - match: '"'
      push: string
    - match: "{single_quote}"
      push: single-string
    - match: '\b({keywords})\b'
      scope: keyword.control
    - match: '\b[0-9][0-9a-fA-FxX_.]*\b'
      scope: constant.numeric
  comment:
    - meta_scope: comment.block
    - match: '\*/'
      pop: true
  string:
    - meta_scope: string.quoted.double
    - match: '\\.'
      scope: constant.character.escape
    - match: '"'
      pop: true
  single-string:
    - meta_scope: string.quoted.single
    - match: "{single_quote}"
      pop: true
"#);
            builder.add(syntect::parsing::SyntaxDefinition::load_from_str(&grammar,true,None).expect("embedded supplemental syntax"));
        }
        builder.build()
    })
}
pub fn highlight(text: &str, language: &str, dark: bool, font: f32) -> LayoutJob {
    static THEMES: OnceLock<ThemeSet> = OnceLock::new();
    let themes = THEMES.get_or_init(ThemeSet::load_defaults);
    let syntax = syntaxes()
        .find_syntax_by_extension(language)
        .unwrap_or_else(|| syntaxes().find_syntax_plain_text());
    let theme = &themes.themes[if dark {
        "base16-ocean.dark"
    } else {
        "InspiredGitHub"
    }];
    let mut highlighter = HighlightLines::new(syntax, theme);
    let mut job = LayoutJob::default();
    job.wrap.max_width = f32::INFINITY;
    for line in syntect::util::LinesWithEndings::from(text) {
        match highlighter.highlight_line(line, syntaxes()) {
            Ok(tokens) => {
                for (style, token) in tokens {
                    job.append(
                        token,
                        0.0,
                        TextFormat {
                            font_id: FontId::monospace(font),
                            color: Color32::from_rgb(
                                style.foreground.r,
                                style.foreground.g,
                                style.foreground.b,
                            ),
                            italics: style
                                .font_style
                                .contains(syntect::highlighting::FontStyle::ITALIC),
                            ..Default::default()
                        },
                    );
                }
            }
            Err(_) => job.append(
                line,
                0.0,
                TextFormat {
                    font_id: FontId::monospace(font),
                    color: crate::theme::colors(dark).text,
                    ..Default::default()
                },
            ),
        }
    }
    job
}
fn mark_matches(job: &mut LayoutJob, matches: &[Range<usize>], dark: bool) {
    let mut sections = Vec::new();
    let mut cursor = 0;
    for section in std::mem::take(&mut job.sections) {
        let mut start = section.byte_range.start;
        while cursor < matches.len() && matches[cursor].end <= start {
            cursor += 1;
        }
        let mut index = cursor;
        while index < matches.len() && matches[index].start < section.byte_range.end {
            let range = &matches[index];
            let a = range.start.max(start);
            let b = range.end.min(section.byte_range.end);
            if a > start {
                let mut part = section.clone();
                part.byte_range = start..a;
                sections.push(part);
            }
            if b > a {
                let mut part = section.clone();
                part.byte_range = a..b;
                part.format.background = crate::theme::colors(dark).selection;
                part.format.color = crate::theme::colors(dark).selection_text;
                sections.push(part);
            }
            start = b.max(a);
            index += 1;
        }
        if start < section.byte_range.end {
            let mut part = section;
            part.byte_range = start..part.byte_range.end;
            sections.push(part);
        }
    }
    job.sections = sections;
}

#[derive(Clone)]
pub struct DiffRow {
    pub left: Option<(usize, String)>,
    pub right: Option<(usize, String)>,
}
pub fn diff(left: &str, right: &str) -> Vec<DiffRow> {
    use similar::{ChangeTag, TextDiff};
    let result = TextDiff::configure()
        .timeout(std::time::Duration::from_secs(2))
        .diff_lines(left, right);
    let mut rows = Vec::new();
    let mut deleted = Vec::new();
    let mut inserted = Vec::new();
    fn flush(rows: &mut Vec<DiffRow>, a: &mut Vec<(usize, String)>, b: &mut Vec<(usize, String)>) {
        let aa = std::mem::take(a);
        let bb = std::mem::take(b);
        let mut aa = aa.into_iter();
        let mut bb = bb.into_iter();
        loop {
            let left = aa.next();
            let right = bb.next();
            if left.is_none() && right.is_none() {
                break;
            }
            rows.push(DiffRow { left, right });
        }
    }
    for change in result.iter_all_changes() {
        let value = change.value().trim_end_matches(['\n', '\r']).to_owned();
        match change.tag() {
            ChangeTag::Delete => deleted.push((change.old_index().unwrap() + 1, value)),
            ChangeTag::Insert => inserted.push((change.new_index().unwrap() + 1, value)),
            ChangeTag::Equal => {
                flush(&mut rows, &mut deleted, &mut inserted);
                rows.push(DiffRow {
                    left: Some((change.old_index().unwrap() + 1, value.clone())),
                    right: Some((change.new_index().unwrap() + 1, value)),
                });
            }
        }
    }
    flush(&mut rows, &mut deleted, &mut inserted);
    rows
}

pub struct Tools {
    id: egui::Id,
    revision: u64,
    pub language: String,
    pub width: usize,
    pub tabs: bool,
    auto_indent: bool,
    wrap: bool,
    numbers: bool,
    font: f32,
    goto: usize,
    pub selection: Range<usize>,
    select_next: Option<Range<usize>>,
    pub query: String,
    pub replacement: String,
    pub find: bool,
    options: SearchOptions,
    scope: Option<Range<usize>>,
    search_key: Option<(String, SearchOptions, u64, Range<usize>)>,
    found: Vec<Range<usize>>,
    message: Option<String>,
    history: Vec<String>,
    future: Vec<String>,
    history_bytes: usize,
    observed: Option<String>,
    syntax_key: Option<(u64, String, bool, u32)>,
    syntax_pending: Option<std::sync::mpsc::Receiver<(u64, String, bool, u32, LayoutJob)>>,
    syntax: Option<LayoutJob>,
}
impl Tools {
    pub fn new(path: &str) -> Self {
        Self {
            id: egui::Id::new(uuid::Uuid::new_v4()),
            revision: 0,
            language: language(path),
            width: 4,
            tabs: false,
            auto_indent: true,
            wrap: false,
            numbers: true,
            font: 14.0,
            goto: 1,
            selection: 0..0,
            select_next: None,
            query: String::new(),
            replacement: String::new(),
            find: false,
            options: SearchOptions::default(),
            scope: None,
            search_key: None,
            found: vec![],
            message: None,
            history: vec![],
            future: vec![],
            history_bytes: 0,
            observed: None,
            syntax_key: None,
            syntax_pending: None,
            syntax: None,
        }
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn has_focus(&self, ctx: &egui::Context) -> bool {
        ctx.memory(|m| m.has_focus(self.id))
    }
    pub fn observe(&mut self, text: &str) {
        if self.observed.as_deref() == Some(text) {
            return;
        }
        self.revision += 1;
        if let Some(previous) = self.observed.replace(text.to_owned()) {
            self.history_bytes += previous.len();
            self.history.push(previous);
            self.future.clear();
            while self.history_bytes > 32 * 1024 * 1024 || self.history.len() > 200 {
                if self.history.is_empty() {
                    break;
                }
                self.history_bytes -= self.history.remove(0).len();
            }
        }
        self.search_key = None;
    }
    fn undo(&mut self, text: &mut String, redo: bool) {
        if redo {
            if let Some(next) = self.future.pop() {
                self.history_bytes += text.len();
                self.history.push(std::mem::replace(text, next));
            }
        } else if let Some(previous) = self.history.pop() {
            self.history_bytes -= previous.len();
            self.future.push(std::mem::replace(text, previous));
        }
        self.revision += 1;
        self.observed = Some(text.clone());
        self.select_next = Some(0..0);
        self.search_key = None;
    }
    pub fn select(&mut self, range: Range<usize>) {
        self.select_next = Some(range);
    }
    fn result(&mut self, text: &mut String, result: Result<String, String>) {
        match result {
            Ok(value) => {
                *text = value;
                self.message = None;
            }
            Err(e) => self.message = Some(e),
        }
    }
    pub fn toolbar(&mut self, ui: &mut egui::Ui, text: &mut String) {
        self.observe(text);
        let floor = |mut n: usize| {
            n = n.min(text.len());
            while !text.is_char_boundary(n) {
                n -= 1;
            }
            n
        };
        self.selection = floor(self.selection.start)..floor(self.selection.end);
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(!self.history.is_empty(), egui::Button::new("Undo"))
                .clicked()
            {
                self.undo(text, false)
            }
            if ui
                .add_enabled(!self.future.is_empty(), egui::Button::new("Redo"))
                .clicked()
            {
                self.undo(text, true)
            }
            egui::ComboBox::from_id_salt("editor-syntax")
                .selected_text(
                    LANGUAGES
                        .iter()
                        .find(|(_, ext)| *ext == self.language)
                        .map_or("Plain text", |p| p.0),
                )
                .show_ui(ui, |ui| {
                    for (label, ext) in LANGUAGES {
                        ui.selectable_value(&mut self.language, (*ext).into(), *label);
                    }
                });
            ui.add(
                egui::DragValue::new(&mut self.font)
                    .range(10.0..=28.0)
                    .prefix("Font "),
            );
            ui.add(
                egui::DragValue::new(&mut self.width)
                    .range(1..=8)
                    .prefix("Indent "),
            );
            ui.checkbox(&mut self.tabs, "Tabs");
            ui.checkbox(&mut self.auto_indent, "Auto indent");
            ui.checkbox(&mut self.wrap, "Wrap");
            ui.checkbox(&mut self.numbers, "Line numbers");
            for (label, outdent) in [("Indent", false), ("Outdent", true)] {
                if ui.button(label).clicked() {
                    self.indent(text, outdent);
                }
            }
            ui.add(
                egui::DragValue::new(&mut self.goto)
                    .range(1..=text.lines().count().max(1))
                    .prefix("Line "),
            );
            if ui.button("Go").clicked() {
                let byte = text
                    .split_inclusive('\n')
                    .take(self.goto.saturating_sub(1))
                    .map(str::len)
                    .sum();
                self.select_next = Some(byte..byte);
            }
        });
        let id = self.id;
        let focused = ui.memory(|m| m.has_focus(id));
        if ui.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::F)) {
            self.find = true;
        }
        if ui.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::H)) {
            self.find = true;
        }
        if focused {
            if ui.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::Z)) {
                self.undo(text, false)
            }
            if ui.input_mut(|i| {
                i.consume_key(egui::Modifiers::CTRL, egui::Key::Y)
                    || i.consume_key(egui::Modifiers::CTRL | egui::Modifiers::SHIFT, egui::Key::Z)
            }) {
                self.undo(text, true)
            }
            if ui.input_mut(|i| i.consume_key(egui::Modifiers::SHIFT, egui::Key::Tab)) {
                self.indent(text, true)
            } else if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Tab)) {
                if self.selection.is_empty() {
                    let value = if self.tabs {
                        "\t".into()
                    } else {
                        " ".repeat(self.width)
                    };
                    let result =
                        apply_byte_edits(text, vec![(self.selection.clone(), value.clone())]);
                    let at = self.selection.start + value.len();
                    self.result(text, result);
                    self.select_next = Some(at..at);
                } else {
                    self.indent(text, false)
                }
            }
            if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter)) {
                let result = if self.auto_indent {
                    newline(text, self.selection.clone(), self.width, self.tabs)
                } else {
                    let value = if text.contains("\r\n") { "\r\n" } else { "\n" };
                    apply_byte_edits(text, vec![(self.selection.clone(), value.into())])
                        .map(|s| (s, self.selection.start + value.len()))
                };
                match result {
                    Ok((value, at)) => {
                        *text = value;
                        self.select_next = Some(at..at);
                    }
                    Err(e) => self.message = Some(e),
                }
            }
        }
        if self.find {
            ui.horizontal_wrapped(|ui| {
                ui.label("Find");
                ui.add(egui::TextEdit::singleline(&mut self.query).desired_width(180.0));
                ui.label("Replace");
                ui.add(egui::TextEdit::singleline(&mut self.replacement).desired_width(180.0));
                ui.checkbox(&mut self.options.case, "Case");
                ui.checkbox(&mut self.options.word, "Whole word");
                ui.checkbox(&mut self.options.regex, "Regex");
                if ui
                    .checkbox(&mut self.options.selection, "In selection")
                    .changed()
                {
                    self.scope = if self.options.selection && !self.selection.is_empty() {
                        Some(self.selection.clone())
                    } else {
                        None
                    };
                }
                if ui.button("Hide").clicked() {
                    self.find = false;
                }
            });
        }
        if !self.find {
            if let Some(message) = &self.message {
                ui.label(message);
            }
        }
        self.observe(text);
    }
    fn indent(&mut self, text: &mut String, outdent: bool) {
        match indent(text, self.selection.clone(), self.width, self.tabs, outdent) {
            Ok((value, range)) => {
                *text = value;
                self.select_next = Some(range)
            }
            Err(e) => self.message = Some(e),
        }
    }
    pub fn search_ui(&mut self, ui: &mut egui::Ui, text: &mut String, revision: u64) {
        if !self.find {
            return;
        }
        if self.options.selection
            && self.scope.as_ref().is_some_and(|r| {
                r.end > text.len()
                    || !text.is_char_boundary(r.start)
                    || !text.is_char_boundary(r.end)
            })
        {
            self.scope = None;
            self.found.clear();
        }
        let scope = self
            .scope
            .clone()
            .filter(|r| {
                r.end <= text.len()
                    && text.is_char_boundary(r.start)
                    && text.is_char_boundary(r.end)
            })
            .unwrap_or(0..text.len());
        if self.options.selection && self.scope.is_none() {
            ui.label("Select text and enable In selection to search only that range.");
            return;
        }
        let key = (
            self.query.clone(),
            self.options.clone(),
            revision,
            scope.clone(),
        );
        if self.search_key.as_ref() != Some(&key) {
            match matches(text, &self.query, &self.options, scope) {
                Ok(found) => {
                    self.found = found;
                    self.message = None
                }
                Err(e) => {
                    self.found.clear();
                    self.message = Some(e)
                }
            }
            self.search_key = Some(key);
        }
        ui.horizontal(|ui| {
            ui.label(format!("{} matches", self.found.len()));
            let next_key = ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::F3));
            let previous_key =
                ui.input_mut(|i| i.consume_key(egui::Modifiers::SHIFT, egui::Key::F3));
            if !self.found.is_empty() {
                if ui.button("Previous").clicked() || previous_key {
                    self.select_next = self
                        .found
                        .iter()
                        .rev()
                        .find(|r| r.start < self.selection.start)
                        .or(self.found.last())
                        .cloned();
                }
                if ui.button("Next").clicked() || next_key {
                    self.select_next = self
                        .found
                        .iter()
                        .find(|r| r.start >= self.selection.end && **r != self.selection)
                        .or(self.found.first())
                        .cloned();
                }
                if ui.button("Replace match").clicked() {
                    if let Some(range) = self.found.iter().find(|r| **r == self.selection).cloned()
                    {
                        let result = replace(
                            text,
                            &self.query,
                            &self.replacement,
                            &self.options,
                            &[range],
                        );
                        self.result(text, result);
                        self.scope = None;
                        self.options.selection = false;
                    } else {
                        self.select_next = self.found.first().cloned();
                    }
                }
                if ui.button("Replace all").clicked() {
                    let result = replace(
                        text,
                        &self.query,
                        &self.replacement,
                        &self.options,
                        &self.found,
                    );
                    self.result(text, result);
                    self.scope = None;
                    self.options.selection = false;
                }
            }
        });
        if let Some(message) = &self.message {
            ui.label(message);
        }
        self.observe(text);
    }
    pub fn code(
        &mut self,
        ui: &mut egui::Ui,
        text: &mut String,
        revision: u64,
        runtime: &crate::connection::Connections,
    ) -> usize {
        let dark = ui.visuals().dark_mode;
        if let Some(receiver) = &self.syntax_pending {
            match receiver.try_recv() {
                Ok((revision, language, dark, font, job)) => {
                    self.syntax = Some(job);
                    self.syntax_key = Some((revision, language, dark, font));
                    self.syntax_pending = None;
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.syntax_pending = None;
                    self.syntax_key =
                        Some((revision, self.language.clone(), dark, self.font.to_bits()));
                    self.message =
                        Some("Syntax colouring failed; the text remains editable".into());
                }
                _ => {}
            }
        }
        let key = (revision, self.language.clone(), dark, self.font.to_bits());
        if self.syntax_pending.is_none() && self.syntax_key.as_ref() != Some(&key) {
            let source = text.clone();
            let ext = self.language.clone();
            let font = self.font;
            let ctx = ui.ctx().clone();
            let (sender, receiver) = std::sync::mpsc::channel();
            self.syntax_pending = Some(receiver);
            runtime.spawn(async move {
                let job =
                    tokio::task::spawn_blocking(move || highlight(&source, &ext, dark, font)).await;
                if let Ok(job) = job {
                    let _ = sender.send((revision, key.1, dark, font.to_bits(), job));
                }
                ctx.request_repaint();
            });
        }
        let id = self.id;
        let scroll_to_selection = self.select_next.is_some();
        if let Some(range) = self.select_next.take() {
            let start = range.start.min(text.len());
            let end = range.end.min(text.len());
            if text.is_char_boundary(start) && text.is_char_boundary(end) {
                let mut state =
                    egui::text_edit::TextEditState::load(ui.ctx(), id).unwrap_or_default();
                state
                    .cursor
                    .set_char_range(Some(egui::text::CCursorRange::two(
                        egui::text::CCursor::new(characters(text, start)),
                        egui::text::CCursor::new(characters(text, end)),
                    )));
                state.store(ui.ctx(), id);
                ui.memory_mut(|m| m.request_focus(id));
            }
        }
        let width = if self.wrap {
            ui.available_width().max(100.0)
        } else {
            f32::INFINITY
        };
        let font = self.font;
        let syntax = &self.syntax;
        let matches = if self.find {
            self.found.as_slice()
        } else {
            &[]
        };
        let match_source = self.observed.as_deref();
        let mut layouter = |ui: &egui::Ui, buffer: &dyn egui::TextBuffer, wrap_width: f32| {
            let mut job = if let Some(job) = syntax.as_ref().filter(|j| j.text == buffer.as_str()) {
                job.clone()
            } else {
                LayoutJob::simple(
                    buffer.as_str().into(),
                    FontId::monospace(font),
                    crate::theme::colors(dark).text,
                    wrap_width,
                )
            };
            job.wrap.max_width = if width.is_finite() {
                wrap_width
            } else {
                f32::INFINITY
            };
            job.sections
                .iter_mut()
                .for_each(|s| s.format.background = Color32::TRANSPARENT);
            if match_source == Some(buffer.as_str())
                && matches.iter().all(|r| {
                    r.end <= job.text.len()
                        && job.text.is_char_boundary(r.start)
                        && job.text.is_char_boundary(r.end)
                })
            {
                mark_matches(&mut job, matches, dark);
            }
            ui.fonts_mut(|fonts| fonts.layout_job(job))
        };
        let mut cursor = 0;
        egui::ScrollArea::both()
            .id_salt("editor-code-scroll")
            .show(ui, |ui| {
                ui.horizontal_top(|ui| {
                    if self.numbers && !self.wrap {
                        let count = text.bytes().filter(|b| *b == b'\n').count() + 1;
                        let mut lines = String::new();
                        for n in 1..=count {
                            use std::fmt::Write;
                            let _ = writeln!(lines, "{n:>5}");
                        }
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(lines)
                                    .monospace()
                                    .size(font)
                                    .color(crate::theme::colors(dark).muted),
                            )
                            .extend(),
                        );
                    }
                    let output = egui::TextEdit::multiline(text)
                        .id(id)
                        .code_editor()
                        .font(FontId::monospace(font))
                        .desired_width(width)
                        .desired_rows(35)
                        .layouter(&mut layouter)
                        .show(ui);
                    if scroll_to_selection {
                        if let Some(range) = output.cursor_range {
                            let rect = output
                                .galley
                                .pos_from_cursor(range.primary)
                                .translate(output.galley_pos.to_vec2());
                            ui.scroll_to_rect(rect, Some(egui::Align::Center));
                        }
                    }
                    if let Some(range) = output.cursor_range {
                        cursor = range.primary.index;
                        let range = range.sorted_cursors();
                        self.selection =
                            byte_at(text, range[0].index)..byte_at(text, range[1].index);
                    }
                });
            });
        if self.syntax_pending.is_some() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(50));
        }
        self.observe(text);
        cursor
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn regex_captures_unicode_and_selection_are_safe() {
        let options = SearchOptions {
            regex: true,
            case: true,
            ..Default::default()
        };
        let s = "α1\r\nβ2\r\n";
        let ranges = matches(s, r"(\p{L})(\d)", &options, 0..s.len()).unwrap();
        assert_eq!(
            replace(s, r"(\p{L})(\d)", "$2$1", &options, &ranges).unwrap(),
            "1α\r\n2β\r\n"
        );
        assert!(expression("[", &options).is_err());
    }
    #[test]
    fn indentation_preserves_crlf_and_last_selection_line() {
        let (s, _) = indent("a\r\nb\r\n", 0..3, 2, false, false).unwrap();
        assert_eq!(s, "  a\r\nb\r\n");
        assert_eq!(
            newline("  if ok:", 8..8, 2, false).unwrap().0,
            "  if ok:\n    "
        );
    }
    #[test]
    fn workspace_ranges_reject_surrogates_and_overlaps_atomically() {
        let s = "a😀b\r\n";
        assert!(utf16_offset(s, &json!({"line":0,"character":2})).is_err());
        assert!(apply_byte_edits(s, vec![(0..5, "x".into()), (1..5, "y".into())]).is_err());
        assert_eq!(lsp_edits(s,&[json!({"range":{"start":{"line":0,"character":1},"end":{"line":0,"character":3}},"newText":"z"})]).unwrap(),"azb\r\n");
    }
    #[test]
    fn insertions_do_not_shift_all_following_diff_rows() {
        let rows = diff("a\nb\n", "a\nx\nb\n");
        assert!(rows[1].left.is_none());
        assert_eq!(rows[2].left.as_ref().unwrap().0, 2);
        assert_eq!(rows[2].right.as_ref().unwrap().0, 3);
    }
    #[test]
    fn all_reference_languages_render_exact_text_in_both_modes() {
        for (_, ext) in LANGUAGES {
            if *ext != "txt" {
                assert!(
                    syntaxes().find_syntax_by_extension(ext).is_some(),
                    "Missing syntax: {ext}"
                );
            }
            for dark in [false, true] {
                let source = "/* comment */\nmodule x;\n\"hello\"\n";
                assert_eq!(highlight(source, ext, dark, 14.0).text, source);
            }
        }
    }
    #[test]
    fn zero_width_search_does_not_loop_and_replacement_does_not_grow_without_limit() {
        let options = SearchOptions {
            regex: true,
            ..Default::default()
        };
        let ranges = matches("a\nb", "^", &options, 0..3).unwrap();
        assert_eq!(
            replace("a\nb", "^", ">", &options, &ranges).unwrap(),
            ">a\n>b"
        );
        assert!(apply_byte_edits("x", vec![(0..0, "a".repeat(MAX_EDITABLE_BYTES))]).is_err());
    }
    #[test]
    fn undo_includes_programmatic_edits_and_new_edits_clear_redo() {
        let mut tools = Tools::new("example.rs");
        let mut text = "first".to_owned();
        tools.observe(&text);
        text = "second".into();
        tools.observe(&text);
        tools.undo(&mut text, false);
        assert_eq!(text, "first");
        tools.undo(&mut text, true);
        assert_eq!(text, "second");
        tools.undo(&mut text, false);
        text = "third".into();
        tools.observe(&text);
        tools.undo(&mut text, true);
        assert_eq!(text, "third");
    }
}
