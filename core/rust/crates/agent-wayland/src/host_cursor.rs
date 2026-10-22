//! The host X server owns cursor theme, resize edges and toolkit pointer shapes.
//! Forward shape changes only; the client draws at its local pointer position.
use smithay::reexports::x11rb::{protocol::xfixes, rust_connection::RustConnection};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CursorImage {
    pub width: u32,
    pub height: u32,
    pub hotspot_x: u32,
    pub hotspot_y: u32,
    /// XFixes premultiplied ARGB converted to straight RGBA for ARPP.
    pub rgba: Vec<u8>,
}

pub(crate) fn capture(
    connection: &RustConnection,
) -> Result<CursorImage, Box<dyn std::error::Error + Send + Sync>> {
    // The caller must negotiate XFixes on this exact connection first.
    let image = xfixes::get_cursor_image(connection)?.reply()?;
    convert(
        image.width.into(),
        image.height.into(),
        image.xhot.into(),
        image.yhot.into(),
        &image.cursor_image,
    )
    .map_err(Into::into)
}

fn convert(
    width: u32,
    height: u32,
    hotspot_x: u32,
    hotspot_y: u32,
    pixels: &[u32],
) -> Result<CursorImage, &'static str> {
    if width == 0
        || height == 0
        || width > 256
        || height > 256
        || hotspot_x >= width
        || hotspot_y >= height
        || pixels.len() != width as usize * height as usize
    {
        return Err("Invalid or oversized host cursor");
    }
    let rgba = pixels
        .iter()
        .flat_map(|&p| {
            let alpha = p >> 24;
            let channel = |shift: u32| -> u8 {
                let value = (p >> shift) & 255;
                if alpha == 0 {
                    0
                } else {
                    ((value * 255 + alpha / 2) / alpha).min(255) as u8
                }
            };
            [channel(16), channel(8), channel(0), alpha as u8]
        })
        .collect();
    Ok(CursorImage {
        width,
        height,
        hotspot_x,
        hotspot_y,
        rgba,
    })
}

#[cfg(test)]
pub(crate) fn verify_forwarding(
    compositor: &crate::compositor::HeadlessCompositor,
    connection: &RustConnection,
    window: u32,
) {
    use smithay::reexports::x11rb::{connection::Connection, protocol::xproto::*};
    use std::time::{Duration, Instant};
    // XFixes version state belongs to each X11 client connection. The bridge
    // negotiated on its own connection; that does not initialize this fixture.
    let version = xfixes::query_version(connection, 5, 0)
        .expect("fixture must request XFixes initialization")
        .reply()
        .expect("fixture must negotiate XFixes before reading cursor images");
    assert!(
        version.major_version >= 1,
        "cursor images require XFixes 1 or newer"
    );
    let cursors = compositor.subscribe_cursors();
    let mut previous = None;
    for horizontal in [true, false] {
        // Use deterministic bitmap cursors; this fixture needs no font package,
        // desktop theme, external image file, or graphics hardware.
        let bitmap = connection.generate_id().unwrap();
        let gc = connection.generate_id().unwrap();
        let cursor = connection.generate_id().unwrap();
        connection
            .create_pixmap(1, bitmap, window, 7, 7)
            .unwrap()
            .check()
            .unwrap();
        connection
            .create_gc(gc, bitmap, &CreateGCAux::new().foreground(0))
            .unwrap()
            .check()
            .unwrap();
        connection
            .poly_fill_rectangle(
                bitmap,
                gc,
                &[Rectangle {
                    x: 0,
                    y: 0,
                    width: 7,
                    height: 7,
                }],
            )
            .unwrap()
            .check()
            .unwrap();
        connection
            .change_gc(gc, &ChangeGCAux::new().foreground(1))
            .unwrap()
            .check()
            .unwrap();
        let line = if horizontal {
            Rectangle {
                x: 0,
                y: 3,
                width: 7,
                height: 1,
            }
        } else {
            Rectangle {
                x: 3,
                y: 0,
                width: 1,
                height: 7,
            }
        };
        connection
            .poly_fill_rectangle(bitmap, gc, &[line])
            .unwrap()
            .check()
            .unwrap();
        connection
            .create_cursor(cursor, bitmap, bitmap, 65535, 0, 0, 0, 0, 0, 3, 3)
            .unwrap()
            .check()
            .unwrap();
        connection.free_gc(gc).unwrap().check().unwrap();
        connection.free_pixmap(bitmap).unwrap().check().unwrap();
        connection
            .change_window_attributes(window, &ChangeWindowAttributesAux::new().cursor(cursor))
            .unwrap()
            .check()
            .unwrap();
        // Derive the oracle from the bitmap we created, not from whichever
        // desktop/WM cursor happens to be active at the time of a single read.
        let expected = CursorImage {
            width: 7,
            height: 7,
            hotspot_x: 3,
            hotspot_y: 3,
            rgba: (0..49)
                .flat_map(|pixel| {
                    if (horizontal && pixel / 7 == 3) || (!horizontal && pixel % 7 == 3) {
                        [255_u8, 0, 0, 255]
                    } else {
                        [0_u8; 4]
                    }
                })
                .collect(),
        };
        if let Some(previous) = previous.as_ref() {
            assert_ne!(previous, &expected, "fixture did not change shape");
        }
        let deadline = Instant::now() + Duration::from_secs(4);
        loop {
            // Move the compositor's seat through the production input route.
            // XWarpPointer acknowledgement alone does not establish Wayland
            // pointer focus. Re-query placement while Xfwm settles its frame.
            let root = connection.setup().roots[0].root;
            let location = connection
                .translate_coordinates(window, root, 16, 16)
                .unwrap()
                .reply()
                .unwrap();
            compositor
                .inject_input(ad_stream_protocol::InputMessage::PointerMove {
                    x: i32::from(location.dst_x),
                    y: i32::from(location.dst_y),
                    monitor: 0,
                })
                .unwrap();
            let observed = capture(connection).expect("negotiated cursor capture must succeed");
            if observed == expected && cursors.borrow().as_deref() == Some(&expected) {
                break;
            }
            let forwarded = cursors
                .borrow()
                .as_ref()
                .map(|image| (image.width, image.height, image.hotspot_x, image.hotspot_y));
            assert!(
                Instant::now() < deadline,
                "expected 7x7 {} red cursor at hotspot (3,3); captured={}x{} hotspot=({},{}), forwarded={forwarded:?}, pointer={:?}",
                if horizontal { "horizontal" } else { "vertical" },
                observed.width, observed.height, observed.hotspot_x, observed.hotspot_y,
                connection.query_pointer(window).unwrap().reply().unwrap()
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        previous = Some(expected);
        connection
            .change_window_attributes(window, &ChangeWindowAttributesAux::new().cursor(0u32))
            .unwrap()
            .check()
            .unwrap();
        connection.free_cursor(cursor).unwrap().check().unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cursor_shapes_preserve_channels_alpha_hotspot_and_bounds() {
        let cursor = convert(2, 1, 1, 0, &[0xff123456, 0x80402010]).unwrap();
        assert_eq!(
            cursor.rgba,
            [0x12, 0x34, 0x56, 0xff, 0x80, 0x40, 0x20, 0x80]
        );
        assert_eq!((cursor.hotspot_x, cursor.hotspot_y), (1, 0));
        for (w, h, x, y, pixels) in [
            (0, 1, 0, 0, &[][..]),
            (257, 1, 0, 0, &[][..]),
            (1, 1, 1, 0, &[0][..]),
            (1, 1, 0, 1, &[0][..]),
            (2, 1, 0, 0, &[0][..]),
        ] {
            assert!(convert(w, h, x, y, pixels).is_err());
        }
    }
}
