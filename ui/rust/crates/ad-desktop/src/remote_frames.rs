//! Prepare decoded frames away from the GUI and input workers. The watch slot
//! keeps the latest decoded frame while one conversion is in progress.
use crate::remote::Image;
use eframe::egui;
use parking_lot::Mutex;
use std::sync::{Arc, OnceLock};
use tokio::sync::watch;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Damage {
    pub x: usize,
    pub y: usize,
    pub width: usize,
    pub height: usize,
}
#[derive(Clone)]
pub(crate) struct PreparedFrame {
    pub sequence: u64,
    pub width: u32,
    pub height: u32,
    pub image: Arc<ColorSnapshot>,
    #[cfg(test)]
    base_sequence: Option<u64>,
    #[cfg(test)]
    damage: Option<Damage>,
    source: Image,
}
/// Only changed colour tiles are allocated. A complete image is assembled lazily
/// for texture recreation, screenshots, or a consumer that skipped a base.
pub(crate) struct ColorSnapshot {
    width: usize,
    height: usize,
    tiles: Vec<Arc<egui::ColorImage>>,
    full: OnceLock<Arc<egui::ColorImage>>,
}
const EDGE: usize = ad_stream_protocol::lossless::TILE_EDGE as usize;
impl ColorSnapshot {
    fn region(&self, index: usize) -> Damage {
        let columns = self.width.div_ceil(EDGE);
        let x = index % columns * EDGE;
        let y = index / columns * EDGE;
        Damage {
            x,
            y,
            width: EDGE.min(self.width - x),
            height: EDGE.min(self.height - y),
        }
    }
    fn extract(&self, r: Damage) -> egui::ColorImage {
        let mut pixels = vec![egui::Color32::BLACK; r.width * r.height];
        for (index, tile) in self.tiles.iter().enumerate() {
            let t = self.region(index);
            let (left, top, right, bottom) = (
                r.x.max(t.x),
                r.y.max(t.y),
                (r.x + r.width).min(t.x + t.width),
                (r.y + r.height).min(t.y + t.height),
            );
            if right <= left || bottom <= top {
                continue;
            }
            for y in top..bottom {
                let source = (y - t.y) * t.width + left - t.x;
                let target = (y - r.y) * r.width + left - r.x;
                pixels[target..target + right - left]
                    .copy_from_slice(&tile.pixels[source..source + right - left]);
            }
        }
        egui::ColorImage::new([r.width, r.height], pixels)
    }
    fn full(&self) -> Arc<egui::ColorImage> {
        self.full
            .get_or_init(|| {
                Arc::new(self.extract(Damage {
                    x: 0,
                    y: 0,
                    width: self.width,
                    height: self.height,
                }))
            })
            .clone()
    }
}
impl std::ops::Deref for ColorSnapshot {
    type Target = egui::ColorImage;
    fn deref(&self) -> &Self::Target {
        self.full.get_or_init(|| {
            Arc::new(self.extract(Damage {
                x: 0,
                y: 0,
                width: self.width,
                height: self.height,
            }))
        })
    }
}
impl PreparedFrame {
    #[cfg(test)]
    pub(crate) fn test_pattern(sequence: u64, width: u32, height: u32) -> Self {
        convert(
            Image {
                width,
                height,
                sequence,
                pixels: Arc::new(
                    [7, 21, 240, 255]
                        .repeat(width as usize * height as usize)
                        .into(),
                ),
                damage: None,
            },
            None,
        )
        .unwrap()
        .unwrap()
    }
    /// A skipped preparation invalidates the incremental base, never the image.
    /// Use the complete image after skipped frames, resize, or texture recreation.
    #[cfg(test)]
    pub(crate) fn upload_after(
        &self,
        sequence: u64,
        has_texture: bool,
    ) -> (Option<[usize; 2]>, Arc<egui::ColorImage>) {
        if has_texture && self.base_sequence == Some(sequence) {
            if let Some(damage) = self.damage {
                return (
                    Some([damage.x, damage.y]),
                    Arc::new(self.image.extract(damage)),
                );
            }
        }
        (None, self.image.full())
    }
    pub(crate) fn uploads_after(
        &self,
        previous: Option<&Self>,
        has_texture: bool,
    ) -> Vec<(Option<[usize; 2]>, Arc<egui::ColorImage>)> {
        if let Some(previous) =
            previous.filter(|p| has_texture && (p.width, p.height) == (self.width, self.height))
        {
            // Compare immutable tile identities, not sequence adjacency. The GUI
            // can skip preparations without either losing damage or uploading
            // a complete multi-monitor texture.
            return self
                .image
                .tiles
                .iter()
                .enumerate()
                .filter_map(|(index, tile)| {
                    if Arc::ptr_eq(tile, &previous.image.tiles[index]) {
                        None
                    } else {
                        let r = self.image.region(index);
                        Some((Some([r.x, r.y]), tile.clone()))
                    }
                })
                .collect();
        }
        vec![(None, self.image.full())]
    }
}
fn changed_region(before: &Image, after: &Image) -> Option<Damage> {
    if let (Some(before), Some(after)) = (before.pixels.tiles(), after.pixels.tiles()) {
        if (before.width, before.height) == (after.width, after.height) {
            let (mut left, mut top, mut right, mut bottom) = (after.width, after.height, 0, 0);
            for (a, b) in before.tiles.iter().zip(&after.tiles) {
                if Arc::ptr_eq(a, b) {
                    continue;
                }
                let r = b.region;
                left = left.min(r.x);
                top = top.min(r.y);
                right = right.max(r.x + r.width);
                bottom = bottom.max(r.y + r.height);
            }
            return (right > left).then_some(Damage {
                x: left as usize,
                y: top as usize,
                width: right.saturating_sub(left) as usize,
                height: bottom.saturating_sub(top) as usize,
            });
        }
    }
    // Validated transaction damage avoids rescanning the entire multi-monitor
    // surface. Skipped preparations still compare the complete immutable images.
    if let Some(damage) = after
        .damage
        .as_ref()
        .filter(|d| d.base_sequence == before.sequence)
    {
        let (mut left, mut top, mut right, mut bottom) = (after.width, after.height, 0, 0);
        let mut valid = true;
        for r in &damage.regions {
            if r.width == 0
                || r.height == 0
                || r.x.checked_add(r.width).is_none_or(|n| n > after.width)
                || r.y.checked_add(r.height).is_none_or(|n| n > after.height)
            {
                valid = false;
                break;
            }
            left = left.min(r.x);
            top = top.min(r.y);
            right = right.max(r.x + r.width);
            bottom = bottom.max(r.y + r.height);
        }
        if valid && right > left && bottom > top {
            return Some(Damage {
                x: left as usize,
                y: top as usize,
                width: (right - left) as usize,
                height: (bottom - top) as usize,
            });
        }
    }
    let width = after.width as usize;
    let height = after.height as usize;
    let (mut left, mut top, mut right, mut bottom) = (width, height, 0, 0);
    for row in 0..height {
        let start = row * width * 4;
        let a = &before.pixels[start..start + width * 4];
        let b = &after.pixels[start..start + width * 4];
        if a == b {
            continue;
        } // optimized slice comparison for unchanged rows
        let first = a
            .chunks_exact(4)
            .zip(b.chunks_exact(4))
            .position(|(a, b)| a != b)
            .unwrap();
        let last = a
            .chunks_exact(4)
            .zip(b.chunks_exact(4))
            .rposition(|(a, b)| a != b)
            .unwrap()
            + 1;
        left = left.min(first);
        right = right.max(last);
        top = top.min(row);
        bottom = row + 1;
        // Stop scanning early for video/full-screen motion.
        if (right - left) * (bottom - top) > width * height / 2 {
            return Some(Damage {
                x: 0,
                y: 0,
                width,
                height,
            });
        }
    }
    (right > left).then_some(Damage {
        x: left,
        y: top,
        width: right.saturating_sub(left),
        height: bottom.saturating_sub(top),
    })
}
fn convert(
    frame: Image,
    previous: Option<&PreparedFrame>,
) -> Result<Option<PreparedFrame>, String> {
    let count = u64::from(frame.width) * u64::from(frame.height);
    if count == 0 || count > 33_554_432 || frame.pixels.len() as u64 != count * 4 {
        return Err("Invalid decoded desktop frame".into());
    }
    let previous = previous.filter(|p| p.width == frame.width && p.height == frame.height);
    let damage = if let Some(previous) = previous {
        let Some(damage) = changed_region(&previous.source, &frame) else {
            return Ok(None);
        };
        (damage.width * damage.height < count as usize / 2).then_some(damage)
    } else {
        None
    };
    let (width, height) = (frame.width as usize, frame.height as usize);
    let mut tiles = Vec::new();
    for y in (0..height).step_by(EDGE) {
        for x in (0..width).step_by(EDGE) {
            let (w, h) = (EDGE.min(width - x), EDGE.min(height - y));
            let index = tiles.len();
            let unchanged = previous.is_some_and(|previous| {
                if let (Some(a), Some(b)) = (previous.source.pixels.tiles(), frame.pixels.tiles()) {
                    Arc::ptr_eq(&a.tiles[index], &b.tiles[index])
                } else {
                    damage.is_some_and(|d| {
                        x + w <= d.x || d.x + d.width <= x || y + h <= d.y || d.y + d.height <= y
                    })
                }
            });
            if unchanged {
                tiles.push(previous.unwrap().image.tiles[index].clone());
                continue;
            }
            let mut pixels = Vec::with_capacity(w * h);
            if let Some(source) = frame.pixels.tiles() {
                pixels.extend(
                    source.tiles[index]
                        .bytes
                        .chunks_exact(4)
                        .map(|p| egui::Color32::from_rgb(p[2], p[1], p[0])),
                );
            } else {
                for row in y..y + h {
                    let start = (row * width + x) * 4;
                    pixels.extend(
                        frame.pixels[start..start + w * 4]
                            .chunks_exact(4)
                            .map(|p| egui::Color32::from_rgb(p[2], p[1], p[0])),
                    );
                }
            }
            tiles.push(Arc::new(egui::ColorImage::new([w, h], pixels)));
        }
    }
    let image = ColorSnapshot {
        width,
        height,
        tiles,
        full: OnceLock::new(),
    };
    Ok(Some(PreparedFrame {
        sequence: frame.sequence,
        width: frame.width,
        height: frame.height,
        image: Arc::new(image),
        #[cfg(test)]
        base_sequence: previous.map(|p| p.sequence),
        #[cfg(test)]
        damage,
        source: frame,
    }))
}
pub(crate) async fn prepare(
    mut frames: watch::Receiver<Option<Image>>,
    output: Arc<Mutex<Option<PreparedFrame>>>,
    ctx: egui::Context,
) -> Result<(), String> {
    let mut previous: Option<PreparedFrame> = None;
    let mut report_at = std::time::Instant::now();
    let (mut count, mut preparation_us) = (0_u64, 0_u128);
    while frames.changed().await.is_ok() {
        let frame = { frames.borrow_and_update().clone() };
        let Some(frame) = frame else {
            continue;
        };
        // Exactly one bounded CPU job per desktop; no blocking work on either
        // the GUI thread or the asynchronous input transport workers.
        let base = previous.take();
        let started = std::time::Instant::now();
        let (prepared, base) = tokio::task::spawn_blocking(move || {
            let prepared = convert(frame, base.as_ref());
            (prepared, base)
        })
        .await
        .map_err(|e| e.to_string())?;
        count += 1;
        preparation_us += started.elapsed().as_micros();
        if report_at.elapsed() >= std::time::Duration::from_secs(5) {
            tracing::info!(
                interval_ms = report_at.elapsed().as_millis() as u64,
                frames = count,
                preparation_us = preparation_us as u64,
                "remote texture preparation performance"
            );
            report_at = std::time::Instant::now();
            (count, preparation_us) = (0, 0);
        }
        if let Some(prepared) = prepared? {
            previous = Some(prepared.clone());
            *output.lock() = Some(prepared);
            ctx.request_repaint();
        } else {
            previous = base;
        }
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    fn frame(sequence: u64, bytes: &[u8]) -> Image {
        Image {
            damage: None,
            width: 1,
            height: 1,
            sequence,
            pixels: Arc::new(bytes.to_vec().into()),
        }
    }
    #[test]
    fn partial_uploads_preserve_colours_and_fall_back_after_skips_or_resize() {
        let initial = Image {
            damage: None,
            width: 8,
            height: 4,
            sequence: 1,
            pixels: Arc::new(vec![0; 8 * 4 * 4].into()),
        };
        let first = convert(initial.clone(), None).unwrap().unwrap();
        let mut second_source = initial.clone();
        second_source.sequence = 2;
        let index = (2 * 8 + 3) * 4;
        Arc::make_mut(&mut second_source.pixels)[index..index + 4]
            .copy_from_slice(&[7, 21, 240, 255]);
        let second = convert(second_source.clone(), Some(&first))
            .unwrap()
            .unwrap();
        let (position, image) = second.upload_after(1, true);
        assert_eq!(position, Some([3, 2]));
        assert_eq!(image.size, [1, 1]);
        assert_eq!(image.pixels[0].to_array(), [240, 21, 7, 255]);
        assert_eq!(second.upload_after(0, true).0, None);
        assert_eq!(second.upload_after(1, false).0, None);
        assert_eq!(second.upload_after(0, true).1.size, [8, 4]);
        assert!(convert(second_source.clone(), Some(&second))
            .unwrap()
            .is_none());
        let mut third_source = second_source.clone();
        third_source.sequence = 3;
        Arc::make_mut(&mut third_source.pixels)[0..4].copy_from_slice(&[1, 2, 3, 255]);
        let third = convert(third_source, Some(&second)).unwrap().unwrap();
        let (position, full) = third.upload_after(1, true); // consumer skipped frame 2
        assert_eq!(position, None);
        assert_eq!(full.pixels[2 * 8 + 3].to_array(), [240, 21, 7, 255]);
        let resized = convert(
            Image {
                damage: None,
                width: 1,
                height: 1,
                sequence: 4,
                pixels: Arc::new(vec![4, 5, 6, 255].into()),
            },
            Some(&third),
        )
        .unwrap()
        .unwrap();
        assert_eq!(resized.upload_after(3, true).0, None);
        assert_eq!(&second_source.pixels[index..index + 4], &[7, 21, 240, 255]);
    }
    #[test]
    fn lossless_damage_uses_only_its_exact_base_and_recovers_after_skipped_preparation() {
        let initial = Image {
            width: 1024,
            height: 512,
            sequence: 1,
            pixels: Arc::new([7, 21, 240, 255].repeat(1024 * 512).into()),
            damage: None,
        };
        let first = convert(initial.clone(), None).unwrap().unwrap();
        let mut second = initial.clone();
        second.sequence = 2;
        Arc::make_mut(&mut second.pixels)[..4].copy_from_slice(&[11, 22, 33, 255]);
        second.damage = Some(crate::remote::ImageDamage {
            base_sequence: 1,
            regions: vec![ad_stream_protocol::Rectangle {
                x: 0,
                y: 0,
                width: 256,
                height: 256,
            }],
        });
        let prepared = convert(second.clone(), Some(&first)).unwrap().unwrap();
        assert_eq!(prepared.upload_after(1, true).1.size, [256, 256]);
        assert_eq!(prepared.image.pixels[0].to_array(), [33, 22, 11, 255]);
        let mut third = second.clone();
        third.sequence = 3;
        Arc::make_mut(&mut third.pixels)[256 * 4..256 * 4 + 4].copy_from_slice(&[44, 55, 66, 255]);
        third.damage = Some(crate::remote::ImageDamage {
            base_sequence: 2,
            regions: vec![ad_stream_protocol::Rectangle {
                x: 256,
                y: 0,
                width: 256,
                height: 256,
            }],
        });
        let recovered = convert(third, Some(&first)).unwrap().unwrap();
        assert_eq!(recovered.image.pixels[0].to_array(), [33, 22, 11, 255]);
        assert_eq!(recovered.image.pixels[256].to_array(), [66, 55, 44, 255]);
        assert_eq!(first.image.pixels[0].to_array(), [240, 21, 7, 255]);
        assert_eq!(recovered.upload_after(0, true).1.size, [1024, 512]);
    }

    #[tokio::test]
    async fn preparation_preserves_bgra_source_and_publishes_isolated_latest_frames() {
        let original = frame(1, &[7, 21, 240, 255]);
        let (tx, rx) = watch::channel(None);
        let output = Arc::new(Mutex::new(None));
        let task = tokio::spawn(prepare(rx, output.clone(), egui::Context::default()));
        tx.send_replace(Some(original.clone()));
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            while output.lock().as_ref().is_none_or(|f| f.sequence != 1) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(
            output.lock().as_ref().unwrap().image.pixels[0].to_array(),
            [240, 21, 7, 255]
        );
        assert_eq!(&original.pixels[..], &[7, 21, 240, 255]);
        for sequence in 2..=100 {
            tx.send_replace(Some(frame(sequence, &[sequence as u8, 5, 9, 255])));
        }
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            while output.lock().as_ref().is_none_or(|f| f.sequence != 100) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        drop(tx);
        task.await.unwrap().unwrap();
    }

    #[test]
    fn tiled_preparation_and_skipped_uploads_keep_exact_colours_without_full_images() {
        use ad_stream_protocol::{pixels::PixelBuffer, Rectangle};
        let (width, height) = (1024, 512);
        let mut raw = [7, 21, 240, 255].repeat(width * height);
        let pixels =
            PixelBuffer::capture(width as u32, height as u32, width * 4, &raw, &[], None).unwrap();
        let first = convert(
            Image {
                width: width as u32,
                height: height as u32,
                sequence: 1,
                pixels: Arc::new(pixels),
                damage: None,
            },
            None,
        )
        .unwrap()
        .unwrap();
        let mut next = first.source.clone();
        next.sequence = 2;
        raw[..4].copy_from_slice(&[11, 22, 33, 255]);
        next.pixels = Arc::new(
            PixelBuffer::capture(
                width as u32,
                height as u32,
                width * 4,
                &raw,
                &[Rectangle {
                    x: 0,
                    y: 0,
                    width: 1,
                    height: 1,
                }],
                Some(&first.source.pixels),
            )
            .unwrap(),
        );
        let second = convert(next, Some(&first)).unwrap().unwrap();
        let mut next = second.source.clone();
        next.sequence = 3;
        let offset = (300 * width + 900) * 4;
        raw[offset..offset + 4].copy_from_slice(&[44, 55, 66, 255]);
        next.pixels = Arc::new(
            PixelBuffer::capture(
                width as u32,
                height as u32,
                width * 4,
                &raw,
                &[Rectangle {
                    x: 900,
                    y: 300,
                    width: 1,
                    height: 1,
                }],
                Some(&second.source.pixels),
            )
            .unwrap(),
        );
        // Both conversion and UI can skip a different frame independently.
        let third = convert(next.clone(), Some(&second)).unwrap().unwrap();
        let skipped = convert(next, Some(&first)).unwrap().unwrap();
        let patches = third.uploads_after(Some(&first), true);
        assert_eq!(patches.len(), 2);
        assert_eq!(patches[0].0, Some([0, 0]));
        assert_eq!(patches[1].0, Some([768, 256]));
        assert_eq!(patches[0].1.pixels[0].to_array(), [33, 22, 11, 255]);
        assert_eq!(
            patches[1].1.pixels[(300 - 256) * 256 + 900 - 768].to_array(),
            [66, 55, 44, 255]
        );
        for image in [&first, &second, &third, &skipped] {
            assert!(image.image.full.get().is_none());
        }
        for index in 1..7 {
            assert!(Arc::ptr_eq(
                &first.image.tiles[index],
                &third.image.tiles[index]
            ));
        }
        assert!(third.uploads_after(Some(&third), true).is_empty());
        let full = skipped.uploads_after(Some(&first), false);
        assert_eq!(full[0].0, None);
        assert_eq!(full[0].1.pixels[0].to_array(), [33, 22, 11, 255]);
        assert_eq!(
            full[0].1.pixels[300 * width + 900].to_array(),
            [66, 55, 44, 255]
        );
    }
}
