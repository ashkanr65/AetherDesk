//! Read-only aligned comparison, computed off the UI thread.
use crate::{connection::Connections, editor::Location, editor_tools::DiffRow};
use eframe::egui;
use tokio::sync::mpsc;
type ResultData = Result<(String, Vec<DiffRow>), String>;
pub struct Comparison {
    id: uuid::Uuid,
    title: String,
    reply: Option<mpsc::Receiver<ResultData>>,
    rows: Vec<DiffRow>,
    summary: String,
    open: bool,
    changes: Vec<usize>,
    change: usize,
    scroll: bool,
}
impl Comparison {
    fn empty(title: String, receiver: mpsc::Receiver<ResultData>) -> Self {
        Self {
            id: uuid::Uuid::new_v4(),
            title,
            reply: Some(receiver),
            rows: vec![],
            summary: "Loading comparison…".into(),
            open: true,
            changes: vec![],
            change: 0,
            scroll: false,
        }
    }
    pub fn new(a: Location, b: Location, runtime: &Connections, ctx: &egui::Context) -> Self {
        let title = format!("{} ↔ {}", label(&a), label(&b));
        let (sender, receiver) = mpsc::channel(1);
        let ctx = ctx.clone();
        runtime.spawn(async move {
            let result = async {
                let a = crate::editor::read_comparison(a).await?;
                let b = crate::editor::read_comparison(b).await?;
                tokio::task::spawn_blocking(move || compare_bytes(a, b))
                    .await
                    .map_err(|e| e.to_string())?
            }
            .await;
            let _ = sender.send(result).await;
            ctx.request_repaint();
        });
        Self::empty(title, receiver)
    }
    pub fn text(
        title: String,
        left: String,
        right: String,
        runtime: &Connections,
        ctx: &egui::Context,
    ) -> Self {
        let (sender, receiver) = mpsc::channel(1);
        let ctx = ctx.clone();
        runtime.spawn(async move {
            let result = tokio::task::spawn_blocking(move || compare_text(left, right))
                .await
                .map_err(|e| e.to_string())
                .and_then(|r| r);
            let _ = sender.send(result).await;
            ctx.request_repaint();
        });
        Self::empty(title, receiver)
    }
    pub fn ui(&mut self, ctx: &egui::Context) -> bool {
        if let Some(reply) = &mut self.reply {
            if let Ok(result) = reply.try_recv() {
                self.reply = None;
                match result {
                    Ok((summary, rows)) => {
                        self.summary = summary;
                        self.changes = rows
                            .iter()
                            .enumerate()
                            .filter(|(_, r)| different(r))
                            .filter_map(|(i, _)| {
                                if i == 0 || !different(&rows[i - 1]) {
                                    Some(i)
                                } else {
                                    None
                                }
                            })
                            .collect();
                        self.rows = rows;
                    }
                    Err(e) => self.summary = e,
                }
            }
        }
        if !self.open {
            return false;
        }
        let id = egui::ViewportId::from_hash_of(self.id);
        ctx.show_viewport_immediate(
            id,
            crate::app_layout::builder(
                ctx,
                id,
                "AetherDesk · Compare files",
                egui::vec2(1100.0, 700.0),
            ),
            |ctx, _| {
                crate::app_layout::update(ctx, "comparison");
                if ctx.input(|i| i.viewport().close_requested()) {
                    self.open = false;
                    return;
                }
                egui::TopBottomPanel::top("diff-actions").show(ctx, |ui| {
                    ui.label(&self.title);
                    ui.add(egui::Label::new(&self.summary).selectable(true));
                    ui.horizontal_wrapped(|ui| {
                        ui.label(format!("{} change groups", self.changes.len()));
                        if !self.changes.is_empty() {
                            if ui.button("Previous change").clicked() {
                                self.change =
                                    (self.change + self.changes.len() - 1) % self.changes.len();
                                self.scroll = true;
                            }
                            if ui.button("Next change").clicked() {
                                self.change = (self.change + 1) % self.changes.len();
                                self.scroll = true;
                            }
                        }
                    });
                });
                egui::CentralPanel::default().show(ctx, |ui| {
                    let mut scroll = egui::ScrollArea::both();
                    if self.scroll && !self.changes.is_empty() {
                        scroll =
                            scroll.vertical_scroll_offset(self.changes[self.change] as f32 * 22.0);
                        self.scroll = false;
                    }
                    scroll.show_rows(ui, 22.0, self.rows.len(), |ui, range| {
                        for index in range {
                            let row = &self.rows[index];
                            let changed = different(row);
                            let fill = ui.visuals().selection.bg_fill;
                            let fg = ui.visuals().selection.stroke.color;
                            ui.columns(2, |cols| {
                                for (col, line) in [(0, &row.left), (1, &row.right)] {
                                    let value = line.as_ref().map_or("".into(), |(n, s)| {
                                        format!(
                                            "{n:>6} {} {s}",
                                            if changed {
                                                if col == 0 {
                                                    "−"
                                                } else {
                                                    "+"
                                                }
                                            } else {
                                                " "
                                            }
                                        )
                                    });
                                    let text = egui::RichText::new(value).monospace();
                                    let text = if changed {
                                        text.background_color(fill).color(fg)
                                    } else {
                                        text
                                    };
                                    let width = cols[col].available_width();
                                    cols[col].add_sized(
                                        [width, 22.0],
                                        egui::Label::new(text).extend().selectable(true),
                                    );
                                }
                            });
                        }
                    });
                });
            },
        );
        self.open
    }
}
fn different(row: &DiffRow) -> bool {
    row.left.as_ref().map(|p| &p.1) != row.right.as_ref().map(|p| &p.1)
}
fn compare_text(a: String, b: String) -> ResultData {
    if a.bytes().filter(|c| *c == b'\n').count() > 200_000
        || b.bytes().filter(|c| *c == b'\n').count() > 200_000
    {
        return Err("Aligned comparison supports up to 200,000 lines per file".into());
    }
    let summary = if a == b {
        "Identical text"
    } else {
        "Different text"
    }
    .to_owned();
    Ok((summary, crate::editor_tools::diff(&a, &b)))
}
fn compare_bytes(a: Vec<u8>, b: Vec<u8>) -> ResultData {
    let summary = format!(
        "{} · Left: {} bytes, SHA256 {} · Right: {} bytes, SHA256 {}",
        if a == b { "Identical" } else { "Different" },
        a.len(),
        hash(&a),
        b.len(),
        hash(&b)
    );
    if let (Some(a), Some(b)) = (
        ad_desktop_model::files::TextFile::decode(&a),
        ad_desktop_model::files::TextFile::decode(&b),
    ) {
        let (_, rows) = compare_text(a.text, b.text)?;
        Ok((summary + " · Read-only aligned snapshots", rows))
    } else {
        Ok((summary + " · Binary comparison", vec![]))
    }
}
fn hash(bytes: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA256, bytes)
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
fn label(location: &Location) -> String {
    match location {
        Location::Local(p) => p.display().to_string(),
        Location::Remote { path, .. } => path.clone(),
    }
}
