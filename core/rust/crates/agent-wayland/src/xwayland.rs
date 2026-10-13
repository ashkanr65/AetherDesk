use std::io::Write;
use std::os::fd::OwnedFd;
use std::process::Stdio;
use std::thread;

use smithay::delegate_xwayland_shell;
use smithay::desktop::Window;
use smithay::utils::{Logical, Rectangle};
use smithay::wayland::compositor::CompositorHandler;
use smithay::wayland::selection::SelectionTarget;
use smithay::wayland::xwayland_shell::{XWaylandShellHandler, XWaylandShellState};
use smithay::xwayland::xwm::{Reorder, ResizeEdge, X11Window, XwmId};
use smithay::xwayland::{X11Surface, X11Wm, XWayland, XWaylandEvent, XwmHandler};
use tracing::{error, info, warn};

use crate::compositor::{
    capture_clipboard, preferred_clipboard_mime, AgentCompositorState, CompositorError,
};

impl AgentCompositorState {
    pub(crate) fn start_xwayland(&mut self) -> Result<(), CompositorError> {
        let (source, client) = XWayland::spawn(
            &self.display_handle,
            None,
            std::env::var_os("LD_LIBRARY_PATH").map(|path| ("LD_LIBRARY_PATH", path)),
            true,
            Stdio::null(),
            Stdio::inherit(),
            |_| (),
        )
        .map_err(|error| CompositorError::XWayland(error.to_string()))?;
        self.loop_handle
            .insert_source(source, move |event, _, data| match event {
                XWaylandEvent::Ready {
                    x11_socket,
                    display_number,
                } => match X11Wm::start_wm(data.loop_handle.clone(), x11_socket, client.clone()) {
                    Ok(window_manager) => {
                        data.client_compositor_state(&client).set_client_scale(1.0);
                        data.xwm = Some(window_manager);
                        data.xdisplay = Some(display_number);
                        info!(display_number, "XWayland is ready");
                    }
                    Err(error) => error!(%error, "failed to start XWayland window manager"),
                },
                XWaylandEvent::Error => {
                    data.enable_xwayland = false;
                    data.xdisplay = None;
                    data.xwm = None;
                    warn!("XWayland exited; native Wayland applications remain available");
                }
            })
            .map_err(|error| CompositorError::XWayland(error.to_string()))?;
        Ok(())
    }
}

impl XWaylandShellHandler for AgentCompositorState {
    fn xwayland_shell_state(&mut self) -> &mut XWaylandShellState {
        &mut self.xwayland_shell_state
    }
}

impl XwmHandler for AgentCompositorState {
    fn xwm_state(&mut self, _xwm: XwmId) -> &mut X11Wm {
        self.xwm.as_mut().expect("XWM callbacks require a live XWM")
    }

    fn new_window(&mut self, _xwm: XwmId, _window: X11Surface) {}

    fn new_override_redirect_window(&mut self, _xwm: XwmId, _window: X11Surface) {}

    fn map_window_request(&mut self, _xwm: XwmId, window: X11Surface) {
        if let Err(error) = window.set_mapped(true) {
            warn!(%error, "failed to map X11 window");
            return;
        }
        let geometry = window.geometry();
        let element = Window::new_x11_window(window.clone());
        let location = (
            self.window_offset
                .min(self.width.saturating_sub(640) as i32),
            self.window_offset
                .min(self.height.saturating_sub(480) as i32),
        );
        self.window_offset = (self.window_offset + 32) % 256;
        self.space.map_element(element, location, true);
        let configured = Rectangle::new(location.into(), geometry.size);
        if let Err(error) = window.configure(configured) {
            warn!(%error, "failed to configure mapped X11 window");
        }
    }

    fn mapped_override_redirect_window(&mut self, _xwm: XwmId, window: X11Surface) {
        let location = window.geometry().loc;
        self.space
            .map_element(Window::new_x11_window(window), location, true);
    }

    fn unmapped_window(&mut self, _xwm: XwmId, window: X11Surface) {
        let element = {
            self.space
                .elements()
                .find(|element| {
                    element
                        .x11_surface()
                        .is_some_and(|candidate| candidate == &window)
                })
                .cloned()
        };
        if let Some(element) = element {
            self.space.unmap_elem(&element);
        }
        if !window.is_override_redirect() {
            let _ = window.set_mapped(false);
        }
    }

    fn destroyed_window(&mut self, _xwm: XwmId, window: X11Surface) {
        self.unmapped_window(_xwm, window);
    }

    fn configure_request(
        &mut self,
        _xwm: XwmId,
        window: X11Surface,
        x: Option<i32>,
        y: Option<i32>,
        width: Option<u32>,
        height: Option<u32>,
        _reorder: Option<Reorder>,
    ) {
        let mut geometry = window.geometry();
        if let Some(x) = x {
            geometry.loc.x = x;
        }
        if let Some(y) = y {
            geometry.loc.y = y;
        }
        if let Some(width) = width.and_then(|value| i32::try_from(value).ok()) {
            geometry.size.w = width.max(1);
        }
        if let Some(height) = height.and_then(|value| i32::try_from(value).ok()) {
            geometry.size.h = height.max(1);
        }
        let _ = window.configure(geometry);
    }

    fn configure_notify(
        &mut self,
        _xwm: XwmId,
        window: X11Surface,
        geometry: Rectangle<i32, Logical>,
        _above: Option<X11Window>,
    ) {
        let element = {
            self.space
                .elements()
                .find(|element| {
                    element
                        .x11_surface()
                        .is_some_and(|candidate| candidate == &window)
                })
                .cloned()
        };
        if let Some(element) = element {
            self.space.map_element(element, geometry.loc, false);
        }
    }

    fn resize_request(&mut self, _xwm: XwmId, window: X11Surface, _button: u32, _edge: ResizeEdge) {
        let _ = window.configure(window.geometry());
    }

    fn move_request(&mut self, _xwm: XwmId, window: X11Surface, _button: u32) {
        let _ = window.configure(window.geometry());
    }

    fn allow_selection_access(&mut self, _xwm: XwmId, selection: SelectionTarget) -> bool {
        selection == SelectionTarget::Clipboard && self.clipboard_selection.is_some()
    }

    fn send_selection(
        &mut self,
        _xwm: XwmId,
        selection: SelectionTarget,
        mime_type: String,
        fd: OwnedFd,
    ) {
        if selection != SelectionTarget::Clipboard {
            return;
        }
        let Some(data) = self
            .clipboard_selection
            .as_ref()
            .and_then(|selection| selection.data(&mime_type))
        else {
            return;
        };
        let _ = thread::Builder::new()
            .name("aetherdesk-x11-clipboard-write".into())
            .spawn(move || {
                let mut file = std::fs::File::from(fd);
                let _ = file.write_all(&data);
                let _ = file.flush();
            });
    }

    fn new_selection(&mut self, _xwm: XwmId, selection: SelectionTarget, mime_types: Vec<String>) {
        if selection != SelectionTarget::Clipboard {
            return;
        }
        let Some(mime_type) = preferred_clipboard_mime(&mime_types) else {
            return;
        };
        let Ok((read_fd, write_fd)) = smithay::reexports::rustix::pipe::pipe() else {
            warn!("failed to allocate XWayland clipboard transfer pipe");
            return;
        };
        let Some(xwm) = self.xwm.as_mut() else {
            return;
        };
        if let Err(error) = xwm.send_selection(
            SelectionTarget::Clipboard,
            mime_type.clone(),
            write_fd,
            self.loop_handle.clone(),
        ) {
            warn!(%error, "failed to request XWayland clipboard contents");
            return;
        }
        capture_clipboard(read_fd, mime_type, self.clipboard_tx.clone());
    }

    fn disconnected(&mut self, _xwm: XwmId) {
        self.xwm = None;
        self.xdisplay = None;
        warn!("XWayland window manager disconnected");
    }
}

delegate_xwayland_shell!(AgentCompositorState);
