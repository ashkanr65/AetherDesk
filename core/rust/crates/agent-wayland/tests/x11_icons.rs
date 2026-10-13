#![cfg(target_os = "linux")]

use agent_wayland::compositor::{ApplicationSpec, CompositorConfig, HeadlessCompositor};
use smithay::reexports::x11rb::{
    connection::Connection, protocol::xproto::*, wrapper::ConnectionExt as _,
};
use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[test]
fn supplies_missing_icons_on_a_real_x_display_and_preserves_existing_icons() {
    let runtime = tempfile::tempdir().unwrap();
    std::fs::set_permissions(runtime.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let display_file = runtime.path().join("display");
    let mut compositor = HeadlessCompositor::spawn(CompositorConfig {
        width: 320,
        height: 200,
        refresh_millihz: 10_000,
        runtime_dir: runtime.path().to_owned(),
        enable_xwayland: true,
        host_window_manager: false,
    })
    .unwrap();
    let mut probe = ApplicationSpec::new("/bin/sh");
    probe.arguments = vec![
        "-c".into(),
        "printf '%s' \"$DISPLAY\" > \"$1\"".into(),
        "probe".into(),
        display_file.as_os_str().to_owned(),
    ];
    compositor.launch(probe).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !display_file.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    let display = std::fs::read_to_string(display_file).unwrap();
    let (connection, screen_number) = smithay::reexports::x11rb::connect(Some(&display)).unwrap();
    let screen = &connection.setup().roots[screen_number];
    let icon = connection
        .intern_atom(false, b"_NET_WM_ICON")
        .unwrap()
        .reply()
        .unwrap()
        .atom;
    let mut windows = Vec::new();
    for class in [
        b"xclock\0XClock\0".as_slice(),
        b"virtuoso\0Virtuoso\0",
        b"existing\0Existing\0",
    ] {
        let window = connection.generate_id().unwrap();
        connection
            .create_window(
                screen.root_depth,
                window,
                screen.root,
                0,
                0,
                32,
                32,
                0,
                WindowClass::INPUT_OUTPUT,
                screen.root_visual,
                &CreateWindowAux::new(),
            )
            .unwrap()
            .check()
            .unwrap();
        connection
            .change_property8(
                PropMode::REPLACE,
                window,
                AtomEnum::WM_CLASS,
                AtomEnum::STRING,
                class,
            )
            .unwrap();
        connection.map_window(window).unwrap();
        windows.push(window);
    }
    connection
        .change_property32(
            PropMode::REPLACE,
            windows[2],
            icon,
            AtomEnum::CARDINAL,
            &[1, 1, 0xff123456],
        )
        .unwrap();
    connection.flush().unwrap();
    let applications = runtime.path().join("applications");
    std::fs::create_dir(&applications).unwrap();
    let artwork = runtime.path().join("virtuoso.ppm");
    std::fs::write(&artwork, b"P6\n1 1\n255\n\xff\x00\x00").unwrap();
    std::fs::write(
        applications.join("virtuoso.desktop"),
        format!(
            "[Desktop Entry]\nName=Virtuoso\nStartupWMClass=Virtuoso\nIcon={}\n",
            artwork.display()
        ),
    )
    .unwrap();
    // Cargo can reuse this test binary after the checkout moves. Embed the same
    // helper as ad-ssh instead of opening an absolute path from the old checkout.
    let helper = runtime.path().join("x11-icons.py");
    std::fs::write(&helper, include_str!("../../../../../tools/x11-icons.py")).unwrap();
    let helper_stderr = runtime.path().join("x11-icons.stderr");
    let mut child = Command::new("python3")
        .arg(helper)
        .current_dir(runtime.path())
        .env("DISPLAY", display)
        .env("XDG_DATA_HOME", runtime.path())
        .env("XDG_CONFIG_HOME", runtime.path())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(std::fs::File::create(&helper_stderr).unwrap())
        .spawn()
        .expect("start the Python X11 icon helper");
    let deadline = Instant::now() + Duration::from_secs(8);
    let mut icons = Vec::new();
    let mut helper_exit = None;
    while Instant::now() < deadline {
        helper_exit = child.try_wait().expect("check the X11 icon helper status");
        if helper_exit.is_some() {
            break;
        }
        icons = windows
            .iter()
            .map(|window| {
                connection
                    .get_property(false, *window, icon, AtomEnum::CARDINAL, 0, 4096)
                    .unwrap()
                    .reply()
                    .unwrap()
                    .value32()
                    .map(|values| values.collect::<Vec<_>>())
                    .unwrap_or_default()
            })
            .collect();
        if icons.iter().all(|value| !value.is_empty()) {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    if helper_exit.is_none() {
        let _ = child.kill();
    }
    let _ = child.wait();
    compositor.shutdown();
    let stderr = std::fs::read_to_string(helper_stderr).unwrap();
    assert!(
        helper_exit.is_none(),
        "X11 icon helper exited before supplying icons: {helper_exit:?}\n{stderr}"
    );
    assert!(
        icons[0].len() >= 2,
        "clock icon missing: {icons:?}\nX11 icon helper stderr: {stderr}"
    );
    assert_eq!(&icons[0][..2], &[48, 48]);
    assert!(icons[1].len() >= 3, "Cadence desktop-entry icon missing");
    assert!(icons[1][2..].iter().all(|pixel| *pixel == 0xffff0000));
    assert_eq!(icons[2], [1, 1, 0xff123456]);
}
