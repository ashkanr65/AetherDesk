//! Immutable lossless tiles survive skipped UI frames without whole-window copies.
use super::*;
use std::collections::BTreeMap;

#[derive(Clone)]
pub(crate) struct Frame {
    pub sequence: u64,
    pub size: [usize; 2],
    pub tiles: Arc<BTreeMap<(u32, u32), Arc<egui::ColorImage>>>,
}
impl Frame {
    /// Flatten only for an explicit screenshot/AI attachment, never for painting.
    pub(crate) fn bgra(&self) -> Vec<u8> {
        let mut output = vec![0; self.size[0] * self.size[1] * 4];
        for (&(x, y), image) in self.tiles.iter() {
            for row in 0..image.size[1] {
                let start = ((y as usize + row) * self.size[0] + x as usize) * 4;
                for (col, pixel) in image.pixels[row * image.size[0]..(row + 1) * image.size[0]]
                    .iter()
                    .enumerate()
                {
                    output[start + col * 4..start + col * 4 + 4].copy_from_slice(&[
                        pixel.b(),
                        pixel.g(),
                        pixel.r(),
                        255,
                    ]);
                }
            }
        }
        output
    }
    fn apply(
        previous: Option<&Self>,
        sequence: u64,
        base: Option<u64>,
        width: u32,
        height: u32,
        tiles: Vec<ad_stream_protocol::lossless::LosslessTile>,
    ) -> Result<Self, String> {
        let decoded = wire::decode_tiles(
            previous.map(|p| (p.sequence, p.size[0] as u32, p.size[1] as u32)),
            sequence,
            base,
            width,
            height,
            tiles,
        )?;
        let mut images = if base.is_some() {
            previous.unwrap().tiles.as_ref().clone()
        } else {
            BTreeMap::new()
        };
        for (region, bytes) in decoded {
            let image = egui::ColorImage::new(
                [region.width as usize, region.height as usize],
                bytes
                    .chunks_exact(4)
                    .map(|p| egui::Color32::from_rgb(p[2], p[1], p[0]))
                    .collect(),
            );
            images.insert((region.x, region.y), Arc::new(image));
        }
        Ok(Self {
            sequence,
            size: [width as usize, height as usize],
            tiles: Arc::new(images),
        })
    }
}

pub(super) fn accept(
    state: &Arc<Mutex<State>>,
    reply: Reply,
    legacy_request: Option<&Request>,
) -> Result<bool, String> {
    match reply {
        Reply::Ready => return Err("Unexpected repeated application handshake".into()),
        Reply::Windows(windows) => {
            if windows.len() > wire::MAX_WINDOWS || windows.iter().any(|w| w.title.len() > 2048) {
                return Err("Invalid application list".into());
            }
            let mut s = state.lock();
            if s.windows == windows {
                return Ok(false);
            }
            s.windows = windows;
        }
        Reply::Selected(window) => {
            wire::dimensions(window.width, window.height)?;
            let mut s = state.lock();
            s.selected = Some(window);
            s.image = None;
            s.popups.clear();
            s.error = None;
        }
        Reply::Metadata(mut window) => {
            wire::dimensions(window.width, window.height)?;
            if window.title.len() > 2048 {
                return Err("Invalid application title".into());
            }
            let mut s = state.lock();
            if let Some(frame) = &s.image {
                window.width = frame.size[0] as u32;
                window.height = frame.size[1] as u32;
            }
            if s.selected.as_ref().is_some_and(|w| w.id == window.id) {
                s.selected = Some(window);
            } else {
                return Err("Application metadata belongs to another window".into());
            }
        }
        Reply::Frame {
            window,
            sequence,
            base,
            width,
            height,
            tiles,
            popups,
        } => {
            wire::validate_popups(&popups)?;
            if u64::from(width) * u64::from(height)
                + popups
                    .iter()
                    .map(|p| u64::from(p.width) * u64::from(p.height))
                    .sum::<u64>()
                > wire::MAX_PIXELS as u64
            {
                return Err("Application and menus exceed the capture budget".into());
            }
            let old = state.lock().clone();
            if !old.selected.as_ref().is_some_and(|w| w.id == window) {
                return Err("Application frame belongs to another window".into());
            }
            let started = Instant::now();
            let count = tiles.len();
            let frame = Frame::apply(old.image.as_ref(), sequence, base, width, height, tiles)?;
            let mut popup_images = Vec::with_capacity(popups.len());
            for popup in popups {
                let image = if let Some(old) = old.popups.iter().find(|p| p.info == popup) {
                    old.image.clone()
                } else {
                    let bytes = wire::apply_frame(
                        None,
                        1,
                        None,
                        popup.width,
                        popup.height,
                        popup.tiles.clone(),
                    )?;
                    Arc::new(egui::ColorImage::new(
                        [popup.width as usize, popup.height as usize],
                        bytes
                            .chunks_exact(4)
                            .map(|p| egui::Color32::from_rgba_premultiplied(p[2], p[1], p[0], p[3]))
                            .collect(),
                    ))
                };
                popup_images.push(PopupState { info: popup, image });
            }
            let mut s = state.lock();
            s.image = Some(frame);
            s.popups = popup_images;
            s.error = None;
            if let Some(w) = &mut s.selected {
                w.width = width;
                w.height = height;
            }
            tracing::debug!(
                window,
                tiles = count,
                decode_us = started.elapsed().as_micros() as u64,
                "Independent application tile update"
            );
        }
        Reply::Error(error) => {
            let mut s = state.lock();
            if quiet_focus_error(&error) {
                s.input_epoch = s.input_epoch.wrapping_add(1);
                s.error = None;
            } else {
                s.error = Some(error.chars().take(240).collect());
            }
            if legacy_request.is_some_and(|r| matches!(r, Request::Poll | Request::Select(_))) {
                s.selected = None;
                s.image = None;
                s.popups.clear();
            }
        }
        Reply::ViewClosed(error) => {
            let mut s = state.lock();
            s.error = Some(error.chars().take(240).collect());
            s.selected = None;
            s.image = None;
            s.popups.clear();
        }
        Reply::Ack | Reply::Idle => return Ok(false),
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ad_stream_protocol::{lossless::LosslessTile, Rectangle};
    #[test]
    fn sparse_frames_preserve_exact_colours_and_reuse_unchanged_tiles_across_skipped_paints() {
        let tile = |x, colour| {
            LosslessTile::encode(
                Rectangle {
                    x,
                    y: 0,
                    width: 256,
                    height: 1,
                },
                [colour, 17, 31, 255].repeat(256),
            )
            .unwrap()
        };
        let a = Frame::apply(None, 1, None, 512, 1, vec![tile(0, 8), tile(256, 9)]).unwrap();
        let b = Frame::apply(Some(&a), 2, Some(1), 512, 1, vec![tile(0, 44)]).unwrap();
        let c = Frame::apply(Some(&b), 3, Some(2), 512, 1, vec![tile(256, 55)]).unwrap();
        assert!(Arc::ptr_eq(&a.tiles[&(256, 0)], &b.tiles[&(256, 0)]));
        assert!(Arc::ptr_eq(&b.tiles[&(0, 0)], &c.tiles[&(0, 0)]));
        assert_eq!(
            c.tiles[&(0, 0)].pixels[0],
            egui::Color32::from_rgb(31, 17, 44)
        );
        assert_eq!(
            c.tiles[&(256, 0)].pixels[0],
            egui::Color32::from_rgb(31, 17, 55)
        );
        let screenshot = c.bgra();
        assert_eq!(&screenshot[..4], &[44, 17, 31, 255]);
        assert_eq!(&screenshot[256 * 4..256 * 4 + 4], &[55, 17, 31, 255]);
        assert!(Frame::apply(Some(&c), 4, Some(2), 512, 1, vec![tile(0, 88)]).is_err());
        assert_eq!(
            c.tiles[&(0, 0)].pixels[0],
            egui::Color32::from_rgb(31, 17, 44)
        );
    }
}
