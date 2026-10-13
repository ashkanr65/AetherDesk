//! Keep ordinary X11 windows reachable after the remote monitor layout changes.
use smithay::reexports::x11rb::{
    connection::Connection, protocol::xproto::*, rust_connection::RustConnection,
};

type Error = Box<dyn std::error::Error + Send + Sync>;

#[derive(Clone, Copy, Debug, PartialEq)]
struct Bounds {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
}

impl Bounds {
    fn right(self) -> i32 {
        self.x + self.width as i32
    }
    fn bottom(self) -> i32 {
        self.y + self.height as i32
    }
    fn intersection(self, other: Self) -> Option<Self> {
        let x = self.x.max(other.x);
        let y = self.y.max(other.y);
        let width = self.right().min(other.right()) - x;
        let height = self.bottom().min(other.bottom()) - y;
        (width > 0 && height > 0).then_some(Self {
            x,
            y,
            width: width.max(0) as u32,
            height: height.max(0) as u32,
        })
    }
}

fn fit(frame: Bounds, areas: &[Bounds]) -> Bounds {
    let Some(area) = areas.iter().max_by_key(|area| {
        let overlap = frame
            .intersection(**area)
            .map_or(0, |r| u64::from(r.width) * u64::from(r.height));
        let cx = i64::from(frame.x) + i64::from(frame.width) / 2;
        let cy = i64::from(frame.y) + i64::from(frame.height) / 2;
        let dx = cx - cx.clamp(i64::from(area.x), i64::from(area.right()));
        let dy = cy - cy.clamp(i64::from(area.y), i64::from(area.bottom()));
        (overlap, -(dx * dx + dy * dy))
    }) else {
        return frame;
    };
    let width = frame.width.min(area.width);
    let height = frame.height.min(area.height);
    Bounds {
        x: frame.x.clamp(area.x, area.right() - width as i32),
        y: frame.y.clamp(area.y, area.bottom() - height as i32),
        width,
        height,
    }
}

fn values(
    connection: &RustConnection,
    window: Window,
    atom: Atom,
    kind: AtomEnum,
    count: u32,
) -> Result<Vec<u32>, Error> {
    let reply = connection
        .get_property(false, window, atom, kind, 0, count)?
        .reply()?;
    Ok(reply.value32().map(|v| v.collect()).unwrap_or_default())
}

pub(crate) fn fit_windows(
    connection: &RustConnection,
    monitors: &[ad_stream_protocol::Rectangle],
) -> Result<(), Error> {
    let root = connection.setup().roots[0].root;
    let atom = |name: &[u8]| -> Result<Atom, Error> {
        Ok(connection.intern_atom(false, name)?.reply()?.atom)
    };
    let clients = values(
        connection,
        root,
        atom(b"_NET_CLIENT_LIST")?,
        AtomEnum::WINDOW,
        4096,
    )?;
    let extents_atom = atom(b"_NET_FRAME_EXTENTS")?;
    let state_atom = atom(b"_NET_WM_STATE")?;
    let type_atom = atom(b"_NET_WM_WINDOW_TYPE")?;
    let desktop_atom = atom(b"_NET_WM_DESKTOP")?;
    let move_atom = atom(b"_NET_MOVERESIZE_WINDOW")?;
    let wm_managed = [
        atom(b"_NET_WM_STATE_MAXIMIZED_VERT")?,
        atom(b"_NET_WM_STATE_MAXIMIZED_HORZ")?,
        atom(b"_NET_WM_STATE_FULLSCREEN")?,
    ];
    let special = [
        atom(b"_NET_WM_WINDOW_TYPE_DESKTOP")?,
        atom(b"_NET_WM_WINDOW_TYPE_DOCK")?,
    ];
    let workareas = values(
        connection,
        root,
        atom(b"_NET_WORKAREA")?,
        AtomEnum::CARDINAL,
        4096,
    )?;
    let current = values(
        connection,
        root,
        atom(b"_NET_CURRENT_DESKTOP")?,
        AtomEnum::CARDINAL,
        1,
    )?
    .first()
    .copied()
    .unwrap_or(0);
    for window in clients {
        // A client may disappear between queries; do not stop processing the
        // other windows or the monitor/clipboard bridge in that case.
        let result = (|| -> Result<(), Error> {
            if values(connection, window, type_atom, AtomEnum::ATOM, 32)?
                .iter()
                .any(|v| special.contains(v))
                || values(connection, window, state_atom, AtomEnum::ATOM, 32)?
                    .iter()
                    .any(|v| wm_managed.contains(v))
            {
                return Ok(());
            }
            let geometry = connection.get_geometry(window)?.reply()?;
            let pos = connection
                .translate_coordinates(window, root, 0, 0)?
                .reply()?;
            let extents = values(connection, window, extents_atom, AtomEnum::CARDINAL, 4)?;
            let e = |i| extents.get(i).copied().unwrap_or(0).min(16384);
            let frame = Bounds {
                x: i32::from(pos.dst_x) - e(0) as i32,
                y: i32::from(pos.dst_y) - e(2) as i32,
                width: u32::from(geometry.width) + e(0) + e(1),
                height: u32::from(geometry.height) + e(2) + e(3),
            };
            let desktop = values(connection, window, desktop_atom, AtomEnum::CARDINAL, 1)?
                .first()
                .copied()
                .filter(|v| *v != u32::MAX)
                .unwrap_or(current);
            let workarea = workareas
                .as_chunks::<4>()
                .0
                .get(desktop as usize)
                .map(|v| Bounds {
                    x: (v[0] as i32).clamp(-32768, 32767),
                    y: (v[1] as i32).clamp(-32768, 32767),
                    width: v[2].min(32768),
                    height: v[3].min(32768),
                });
            let areas: Vec<_> = monitors
                .iter()
                .map(|r| {
                    let monitor = Bounds {
                        x: r.x as i32,
                        y: r.y as i32,
                        width: r.width,
                        height: r.height,
                    };
                    workarea
                        .and_then(|w| monitor.intersection(w))
                        .unwrap_or(monitor)
                })
                .collect();
            let target = fit(frame, &areas);
            if target == frame {
                return Ok(());
            }
            // Ask the host WM to move/resize the decorated frame. Do not
            // reparent windows, change their workspace, or remove WM states.
            connection
                .send_event(
                    false,
                    root,
                    EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
                    ClientMessageEvent::new(
                        32,
                        window,
                        move_atom,
                        [
                            1 | (0xf << 8) | (2 << 12), // NorthWest gravity, x/y/w/h, pager source.
                            target.x as u32,
                            target.y as u32,
                            target.width.saturating_sub(e(0) + e(1)).max(1),
                            target.height.saturating_sub(e(2) + e(3)).max(1),
                        ],
                    ),
                )?
                .check()?;
            Ok(())
        })();
        if let Err(error) = result {
            tracing::debug!(%error, window, "could not fit a remote window to the new monitors");
        }
    }
    connection.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn disappearing_monitor_and_oversized_window_fit_the_tab() {
        let tab = Bounds {
            x: 0,
            y: 24,
            width: 800,
            height: 576,
        };
        assert_eq!(
            fit(
                Bounds {
                    x: 3300,
                    y: 900,
                    width: 1110,
                    height: 734
                },
                &[tab]
            ),
            tab
        );
    }
    #[test]
    fn visible_windows_stay_put_and_gaps_choose_a_real_monitor() {
        let monitors = [
            Bounds {
                x: 0,
                y: 0,
                width: 1920,
                height: 1080,
            },
            Bounds {
                x: 1920,
                y: 720,
                width: 1920,
                height: 1080,
            },
        ];
        let visible = Bounds {
            x: 2100,
            y: 900,
            width: 500,
            height: 300,
        };
        assert_eq!(fit(visible, &monitors), visible);
        assert_eq!(
            fit(
                Bounds {
                    x: 2000,
                    y: 0,
                    width: 500,
                    height: 300
                },
                &monitors
            ),
            Bounds {
                x: 1420,
                y: 0,
                width: 500,
                height: 300
            }
        );
    }
}
