//! Real protocol checks on the packaging test's private, authenticated server.
//! No SSH, user applications, clipboard contents or synthetic keyboard input.
use super::*;
use std::io;

fn word(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes(bytes[at..at + 2].try_into().unwrap())
}
fn dword(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}
fn words(values: &[u32]) -> Vec<u8> {
    values.iter().flat_map(|n| n.to_le_bytes()).collect()
}

struct Client {
    socket: TcpStream,
    sequence: u16,
    next: u32,
    base: u32,
    mask: u32,
}
impl Client {
    fn id(&mut self) -> u32 {
        self.next += 1;
        assert_eq!(self.next & !self.mask, 0);
        self.base | self.next
    }
    async fn send(&mut self, opcode: u8, detail: u8, body: &[u8]) -> io::Result<u16> {
        assert_eq!(body.len() % 4, 0);
        let length = u16::try_from(body.len() / 4 + 1).unwrap().to_le_bytes();
        self.socket
            .write_all(&[opcode, detail, length[0], length[1]])
            .await?;
        self.socket.write_all(body).await?;
        self.sequence = self.sequence.wrapping_add(1);
        Ok(self.sequence)
    }
    async fn reply(&mut self, expected: u16) -> io::Result<Vec<u8>> {
        loop {
            let mut header = [0; 32];
            self.socket.read_exact(&mut header).await?;
            match header[0] & 0x7f {
                0 => {
                    return Err(io::Error::other(format!(
                        "X11 error {}, request {}, sequence {}",
                        header[1],
                        header[10],
                        word(&header, 2)
                    )))
                }
                1 => {
                    assert_eq!(word(&header, 2), expected);
                    let size = dword(&header, 4) as usize * 4;
                    assert!(size <= 1024 * 1024);
                    let mut bytes = header.to_vec();
                    bytes.resize(32 + size, 0);
                    self.socket.read_exact(&mut bytes[32..]).await?;
                    return Ok(bytes);
                }
                35 => {
                    let size = dword(&header, 4) as usize * 4;
                    assert!(size <= 1024 * 1024);
                    self.socket.read_exact(&mut vec![0; size]).await?;
                }
                _ => {} // ordinary 32-byte window events
            }
        }
    }
    async fn fence(&mut self) -> io::Result<()> {
        let sequence = self.send(43, 0, &[]).await?; // GetInputFocus
        self.reply(sequence).await?;
        Ok(())
    }
    async fn fill(
        &mut self,
        pixmap: u32,
        gc: u32,
        colour: u32,
        x: u16,
        width: u16,
    ) -> io::Result<()> {
        self.send(56, 0, &words(&[gc, 1 << 2, colour])).await?; // GC foreground
        let mut body = words(&[pixmap, gc]);
        for value in [x, 0, width, 64] {
            body.extend(value.to_le_bytes());
        }
        self.send(70, 0, &body).await?; // PolyFillRectangle
        Ok(())
    }
    async fn copy(&mut self, pixmap: u32, gc: u32, src: [u16; 2], dst: [u16; 2]) -> io::Result<()> {
        let mut body = words(&[pixmap, pixmap, gc]);
        for value in [src[0], src[1], dst[0], dst[1], 100, 50] {
            body.extend(value.to_le_bytes());
        }
        self.send(62, 0, &body).await?; // CopyArea, deliberately overlapping
        Ok(())
    }
}

pub(super) async fn exercise(config: &X11ForwardingConfig, server_pid: u32) -> io::Result<()> {
    let mut socket = TcpStream::connect(("127.0.0.1", config.local_port)).await?;
    socket.set_nodelay(true)?;
    let cookie: Vec<u8> = config
        .local_authentication_cookie
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect();
    socket
        .write_all(&packet(&cookie.try_into().unwrap()))
        .await?;
    let mut prefix = [0; 8];
    socket.read_exact(&mut prefix).await?;
    assert_eq!(prefix[0], 1);
    let mut setup = vec![0; usize::from(word(&prefix, 6)) * 4];
    socket.read_exact(&mut setup).await?;
    let screen = 32 + (usize::from(word(&setup, 16)) + 3) / 4 * 4 + usize::from(setup[21]) * 8;
    let root = dword(&setup, screen);
    let root_width = word(&setup, screen + 20);
    let root_height = word(&setup, screen + 22);
    let visual = dword(&setup, screen + 32);
    let depth = setup[screen + 38];
    let formats = 32 + (usize::from(word(&setup, 16)) + 3) / 4 * 4;
    assert!(setup[formats..screen]
        .chunks_exact(8)
        .any(|f| f[0] == depth && f[1] == 32));
    assert_eq!(setup[22], 0, "expected little-endian ZPixmap pixels");
    let mut pos = screen + 40;
    let mut masks = None;
    for _ in 0..setup[screen + 39] {
        let count = word(&setup, pos + 2);
        pos += 8;
        for _ in 0..count {
            if dword(&setup, pos) == visual {
                assert_eq!(setup[pos + 4], 4, "default visual must remain TrueColor");
                masks = Some([
                    dword(&setup, pos + 8),
                    dword(&setup, pos + 12),
                    dword(&setup, pos + 16),
                ]);
            }
            pos += 24;
        }
    }
    assert_eq!(masks, Some([0xff0000, 0xff00, 0xff]));
    let mut client = Client {
        socket,
        sequence: 0,
        next: 0,
        base: dword(&setup, 4),
        mask: dword(&setup, 8),
    };

    // Preserve application-visible Composite/GLX capabilities; only the private
    // Windows window manager's optional redirection is disabled.
    for extension in [b"Composite".as_slice(), b"GLX".as_slice()] {
        let mut body = Vec::from((extension.len() as u16).to_le_bytes());
        body.extend([0, 0]);
        body.extend(extension);
        body.resize((body.len() + 3) / 4 * 4, 0);
        let sequence = client.send(98, 0, &body).await?;
        assert_eq!(
            client.reply(sequence).await?[8],
            1,
            "X11 extension disappeared"
        );
    }

    let pixmap = client.id();
    let gc = client.id();
    let mut body = words(&[pixmap, root]);
    body.extend([128u16, 64].iter().flat_map(|n| n.to_le_bytes()));
    client.send(53, depth, &body).await?;
    client
        .send(55, 0, &words(&[gc, pixmap, 1 << 16, 0]))
        .await?; // no graphics-exposure events
    let palette = [0xd93a71, 0x36b95d, 0x427ed1, 0x123456];
    let mut expected = vec![0u32; 128 * 64];
    for (index, colour) in palette.into_iter().enumerate() {
        client
            .fill(pixmap, gc, colour, index as u16 * 32, 32)
            .await?;
        for row in expected.chunks_exact_mut(128) {
            row[index * 32..index * 32 + 32].fill(colour);
        }
    }
    for (src, dst) in [([0, 0], [7, 3]), ([7, 3], [0, 0])] {
        client.copy(pixmap, gc, src, dst).await?;
        let before = expected.clone();
        for y in 0..50usize {
            for x in 0..100usize {
                expected[(y + dst[1] as usize) * 128 + x + dst[0] as usize] =
                    before[(y + src[1] as usize) * 128 + x + src[0] as usize];
            }
        }
    }
    let mut image = words(&[pixmap, 0]); // drawable, x=0/y=0
    image.extend([128u16, 64].iter().flat_map(|n| n.to_le_bytes()));
    image.extend(u32::MAX.to_le_bytes());
    let sequence = client.send(73, 2, &image).await?; // GetImage ZPixmap
    let pixels = client.reply(sequence).await?;
    assert_eq!(pixels[1], depth);
    let actual: Vec<u32> = pixels[32..]
        .chunks_exact(4)
        .map(|p| dword(p, 0) & 0xffffff)
        .collect();
    assert_eq!(
        actual, expected,
        "overlapping image copies changed exact RGB colours"
    );

    // Top-level and child window backing is stressed at both root edges,
    // partly off-screen, and after repeatedly growing/shrinking their parents.
    // Override-redirect keeps these disposable windows from taking user focus.
    let parent = client.id();
    let child = client.id();
    for (id, owner) in [(parent, root), (child, parent)] {
        let mut body = words(&[id, owner]);
        for value in [32u16, 32, 180, 120, 0, 1] {
            body.extend(value.to_le_bytes());
        }
        body.extend(words(&[0, (1 << 1) | (1 << 9), palette[0], 1]));
        client.send(1, 0, &body).await?;
        client.send(8, 0, &words(&[id])).await?;
    }
    // Reading a pixmap alone missed the blank-window regression. Check that
    // its pixels actually reach the native window, without reading user windows.
    let mut copy = words(&[pixmap, parent, gc]);
    for value in [0u16, 0, 0, 0, 128, 32] {
        copy.extend(value.to_le_bytes());
    }
    client.send(62, 0, &copy).await?;
    client.fence().await?;
    let mut atom = Vec::from((b"_WINDOWSWM_NATIVE_HWND".len() as u16).to_le_bytes());
    atom.extend([0, 0]);
    atom.extend(b"_WINDOWSWM_NATIVE_HWND");
    atom.resize((atom.len() + 3) / 4 * 4, 0);
    let sequence = client.send(16, 0, &atom).await?;
    let native_atom = dword(&client.reply(sequence).await?, 8);
    let until = tokio::time::Instant::now() + Duration::from_secs(4);
    let hwnd = loop {
        let sequence = client
            .send(20, 0, &words(&[parent, native_atom, 19, 0, 2]))
            .await?;
        let property = client.reply(sequence).await?;
        if property.len() >= 40 && dword(&property, 16) == 2 {
            break u64::from_le_bytes(property[32..40].try_into().unwrap()) as usize;
        }
        if tokio::time::Instant::now() >= until {
            return Err(io::Error::other(
                "owned X11 test window has no native handle",
            ));
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    let samples: Vec<_> = [16usize, 48, 80, 112]
        .into_iter()
        .map(|x| (x as i32, 8, expected[8 * 128 + x]))
        .collect();
    show_owned_fixture(hwnd, server_pid)?;
    let mut observed = Vec::new();
    loop {
        // A real client repaints after expose/map; native window creation is
        // asynchronous with respect to the X protocol connection.
        client.send(62, 0, &copy).await?;
        client.fence().await?;
        observed.clear();
        let matches = samples.iter().all(|(x, y, wanted)| {
            let actual = native_pixel(hwnd, *x, *y);
            observed.push(actual);
            actual == Some(*wanted)
        });
        if matches {
            break;
        }
        if tokio::time::Instant::now() >= until {
            return Err(io::Error::other(format!(
                "native X11 window is blank or miscoloured: {observed:x?}"
            )));
        }
        tokio::time::sleep(Duration::from_millis(40)).await;
    }
    for iteration in 0..48u32 {
        let (x, y) = match iteration % 4 {
            0 => (-80i32, -60i32),
            1 => (i32::from(root_width) - 50, i32::from(root_height) - 30),
            2 => (64, 64),
            _ => (i32::from(root_width) / 2, i32::from(root_height) / 2),
        };
        let width = 90 + (iteration % 5) * 53;
        let height = 70 + (iteration % 3) * 39;
        let mut body = words(&[parent, 15, x as u32, y as u32, width, height]);
        client.send(12, 0, &body).await?;
        body = words(&[child, 15, 15, 15, width + 20, height + 10]);
        client.send(12, 0, &body).await?;
        client
            .send(
                56,
                0,
                &words(&[gc, 1 << 2, palette[(iteration % 4) as usize]]),
            )
            .await?;
        let mut fill = words(&[child, gc]);
        for value in [0u16, 0, 80, 64] {
            fill.extend(value.to_le_bytes());
        }
        client.send(70, 0, &fill).await?;
        client.fence().await?;
    }
    client.send(4, 0, &words(&[parent])).await?; // descendants follow their owned parent
    client.send(54, 0, &words(&[pixmap])).await?;
    client.send(60, 0, &words(&[gc])).await?;
    client.fence().await
}

fn show_owned_fixture(window: usize, server_pid: u32) -> io::Result<()> {
    use windows_sys::Win32::{Foundation::HWND, UI::WindowsAndMessaging::*};
    unsafe {
        let hwnd = window as HWND;
        let mut actual_pid = 0;
        GetWindowThreadProcessId(hwnd, &mut actual_pid);
        // Process ownership is checked before touching any native window.
        if actual_pid != server_pid || IsWindow(hwnd) == 0 {
            return Err(io::Error::other(
                "X11 fixture native window ownership mismatch",
            ));
        }
        // VcXsrv does not implement WM_PRINTCLIENT. Keep just this owned test
        // window visible on the primary screen, without taking keyboard focus.
        if SetWindowPos(
            hwnd,
            HWND_TOPMOST,
            40,
            40,
            0,
            0,
            SWP_NOACTIVATE | SWP_NOSIZE | SWP_SHOWWINDOW,
        ) == 0
        {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

fn native_pixel(window: usize, x: i32, y: i32) -> Option<u32> {
    use windows_sys::Win32::{Foundation::HWND, Graphics::Gdi::*};
    // Package regression only: sample the owned fixture's client DC. No global
    // screen capture, focus change, input injection or remote application access.
    unsafe {
        let hwnd = window as HWND;
        let dc = GetDC(hwnd);
        if dc.is_null() {
            return None;
        }
        let pixel = GetPixel(dc, x, y);
        ReleaseDC(hwnd, dc);
        if pixel == u32::MAX {
            None
        } else {
            Some((pixel & 255) << 16 | (pixel & 0xff00) | (pixel >> 16 & 255))
        }
    }
}
