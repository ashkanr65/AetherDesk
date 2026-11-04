//! Read only damaged tiles; preserve exact BGRA values and publish atomically.
use super::*;

fn affected(
    width: u32,
    height: u32,
    damage: &[x11rb::protocol::xproto::Rectangle],
) -> BTreeSet<(u32, u32)> {
    let mut tiles = BTreeSet::new();
    for r in damage {
        let left = i32::from(r.x).max(0) as u32;
        let top = i32::from(r.y).max(0) as u32;
        let right = (i32::from(r.x) + i32::from(r.width)).max(0) as u32;
        let bottom = (i32::from(r.y) + i32::from(r.height)).max(0) as u32;
        if left >= width || top >= height || right <= left || bottom <= top {
            continue;
        }
        for y in (top / 256)..bottom.min(height).div_ceil(256) {
            for x in (left / 256)..right.min(width).div_ceil(256) {
                tiles.insert((x * 256, y * 256));
            }
        }
    }
    tiles
}

impl Mirror {
    pub(super) fn capture_tiles(&mut self) -> Result<(Vec<LosslessTile>, Option<u64>), Error> {
        let started = Instant::now();
        let s = self.selected.as_mut().ok_or("No application selected")?;
        let (width, height) = (s.info.width, s.info.height);
        if s.rebind || s.size != (width, height) {
            self.connection.free_pixmap(s.pixmap)?.check()?;
            s.pixmap = self.connection.generate_id()?;
            composite::name_window_pixmap(&self.connection, s.id, s.pixmap)?.check()?;
            s.size = (width, height);
            s.pixels.clear();
            s.rebind = false;
        }
        // Snapshot and clear the accumulated damage in one server operation.
        // Draws after this request remain damaged for the following transaction.
        damage::subtract(&self.connection, s.damage, 0u32, s.region)?.check()?;
        let rectangles = xfixes::fetch_region(&self.connection, s.region)?
            .reply()?
            .rectangles;
        let base = (!s.pixels.is_empty()).then_some(s.sequence);
        let positions = if base.is_none() {
            (0..height)
                .step_by(256)
                .flat_map(|y| (0..width).step_by(256).map(move |x| (x, y)))
                .collect()
        } else {
            affected(width, height, &rectangles)
        };
        let mut regions: Vec<_> = positions
            .into_iter()
            .map(|(x, y)| Rectangle {
                x,
                y,
                width: (width - x).min(256),
                height: (height - y).min(256),
            })
            .collect();
        let changed_pixels: u64 = regions
            .iter()
            .map(|r| u64::from(r.width) * u64::from(r.height))
            .sum();
        let full = changed_pixels * 2 >= u64::from(width) * u64::from(height);
        let reads = if full {
            vec![Rectangle {
                x: 0,
                y: 0,
                width,
                height,
            }]
        } else {
            regions.clone()
        };
        self.last_read_pixels = reads
            .iter()
            .map(|r| u64::from(r.width) * u64::from(r.height))
            .sum();
        // Issue all local X requests before reading replies, avoiding a local
        // request/reply barrier for every tile in a drawing burst.
        let cookies = reads
            .iter()
            .map(|r| {
                self.connection.get_image(
                    ImageFormat::Z_PIXMAP,
                    s.pixmap,
                    r.x as i16,
                    r.y as i16,
                    r.width as u16,
                    r.height as u16,
                    u32::MAX,
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut images = Vec::with_capacity(cookies.len());
        for (r, cookie) in reads.iter().zip(cookies) {
            let image = cookie.reply()?;
            let format = self
                .connection
                .setup()
                .pixmap_formats
                .iter()
                .find(|f| f.depth == image.depth)
                .ok_or("Unsupported window pixel format")?;
            images.push(decode_pixels(
                r.width,
                r.height,
                format.bits_per_pixel,
                format.scanline_pad,
                self.connection.setup().image_byte_order == ImageOrder::LSB_FIRST,
                &image.data,
            )?);
        }
        let mut patches = Vec::with_capacity(regions.len());
        for (index, r) in regions.drain(..).enumerate() {
            let mut bytes = Vec::with_capacity((r.width * r.height * 4) as usize);
            for row in 0..r.height {
                let (image, start) = if full {
                    (&images[0], ((r.y + row) * width + r.x) as usize * 4)
                } else {
                    (&images[index], (row * r.width) as usize * 4)
                };
                bytes.extend_from_slice(&image[start..start + r.width as usize * 4]);
            }
            let changed = base.is_none()
                || (0..r.height).any(|row| {
                    let start = ((r.y + row) * width + r.x) as usize * 4;
                    s.pixels[start..start + r.width as usize * 4]
                        != bytes[(row * r.width) as usize * 4..((row + 1) * r.width) as usize * 4]
                });
            if changed {
                patches.push((r, bytes));
            }
        }
        let tiles = patches
            .iter()
            .map(|(r, bytes)| LosslessTile::encode(*r, bytes.clone()))
            .collect::<Result<Vec<_>, _>>()?;
        if base.is_none() {
            s.pixels = vec![0; wire::dimensions(width, height)?];
        }
        for (r, bytes) in patches {
            for row in 0..r.height {
                let start = ((r.y + row) * width + r.x) as usize * 4;
                s.pixels[start..start + r.width as usize * 4].copy_from_slice(
                    &bytes[(row * r.width) as usize * 4..((row + 1) * r.width) as usize * 4],
                );
            }
        }
        tracing::debug!(
            window = s.id,
            width,
            height,
            pixels_read = self.last_read_pixels,
            tiles = tiles.len(),
            capture_us = started.elapsed().as_micros() as u64,
            "Independent application damage capture"
        );
        Ok((tiles, base))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn damage_tiles_clip_negative_rectangles_and_preserve_every_touched_edge() {
        let rect = |x, y, width, height| x11rb::protocol::xproto::Rectangle {
            x,
            y,
            width,
            height,
        };
        let tiles = affected(
            1024,
            768,
            &[
                rect(-3, -2, 5, 4),
                rect(255, 255, 2, 2),
                rect(1000, 750, 100, 100),
                rect(3000, 0, 1, 1),
            ],
        );
        assert_eq!(
            tiles,
            BTreeSet::from([(0, 0), (0, 256), (256, 0), (256, 256), (768, 512)])
        );
        assert_eq!(affected(3840, 2160, &[rect(10, 10, 5, 5)]).len(), 1);
    }
}

#[cfg(test)]
pub(super) fn verify_partial(
    mirror: &mut Mirror,
    controller: &RustConnection,
    window: u32,
    gc: u32,
    original_width: u32,
    original_height: u32,
) {
    for (width, height) in [(600, 400), (original_width, original_height)] {
        mirror
            .handle(wire::Request::Resize { width, height })
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let geometry = controller.get_geometry(window).unwrap().reply().unwrap();
            if (u32::from(geometry.width), u32::from(geometry.height)) == (width, height) {
                break;
            }
            assert!(Instant::now() < deadline, "fixture resize failed");
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(matches!(
            mirror.handle(wire::Request::Poll).unwrap(),
            wire::Reply::Frame { base: None, .. }
        ));
        if width != 600 {
            continue;
        }
        controller
            .change_gc(gc, &ChangeGCAux::new().foreground(0x445566))
            .unwrap()
            .check()
            .unwrap();
        controller
            .poly_fill_rectangle(
                window,
                gc,
                &[x11rb::protocol::xproto::Rectangle {
                    x: 1,
                    y: 1,
                    width: 8,
                    height: 8,
                }],
            )
            .unwrap()
            .check()
            .unwrap();
        loop {
            if let wire::Reply::Frame { tiles, .. } = mirror.handle(wire::Request::Poll).unwrap() {
                assert_eq!(tiles.len(), 1);
                assert_eq!(
                    &tiles[0].clone().decode().unwrap()
                        [((1 * 256 + 1) * 4)..((1 * 256 + 1) * 4 + 4)],
                    &[0x66, 0x55, 0x44, 255]
                );
                assert!(
                    mirror.last_read_pixels <= 256 * 256
                        && mirror.last_read_pixels < u64::from(width) * u64::from(height)
                );
                eprintln!(
                    "PERF: small change read {} pixels versus previous full-window {} pixels",
                    mirror.last_read_pixels,
                    width * height
                );
                break;
            }
            assert!(Instant::now() < deadline, "partial damage was not captured");
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}
