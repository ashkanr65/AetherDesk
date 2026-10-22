//! Client UI renderer selection. This does not change remote pixels or codecs.
use serde::{Deserialize, Serialize};
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Renderer {
    #[default]
    Automatic,
    Direct3D,
    OpenGl,
}
impl Renderer {
    pub fn label(self) -> &'static str {
        match self {
            Self::Automatic => "Automatic",
            Self::Direct3D => "Direct3D 12",
            Self::OpenGl => "OpenGL (compatibility)",
        }
    }
}
fn direct3d(renderer: Renderer) -> bool {
    cfg!(windows) && renderer != Renderer::OpenGl
}
pub fn configure(options: &mut eframe::NativeOptions, renderer: Renderer) {
    // An emergency override is usable even if the driver fails before Preferences opens.
    let renderer = match std::env::var("AETHERDESK_RENDERER").ok().as_deref() {
        Some("opengl") => Renderer::OpenGl,
        Some("direct3d") => Renderer::Direct3D,
        _ => renderer,
    };
    configure_selected(options, renderer);
}
fn configure_selected(options: &mut eframe::NativeOptions, renderer: Renderer) {
    options.renderer = if direct3d(renderer) {
        eframe::Renderer::Wgpu
    } else {
        eframe::Renderer::Glow
    };
    if direct3d(renderer) {
        // Restrict Windows to DX12: do not silently re-enter the OpenGL driver
        // that faulted during native popup resizing. Fxc needs no bundled DXC DLL.
        let mut setup = eframe::egui_wgpu::WgpuSetupCreateNew::default();
        setup.instance_descriptor.backends = eframe::wgpu::Backends::DX12;
        setup
            .instance_descriptor
            .backend_options
            .dx12
            .shader_compiler = eframe::wgpu::Dx12Compiler::Fxc;
        // egui defaults to 8192 even when DX12 supports wider desktops.
        // Match the protocol's per-side ceiling, within the real adapter limit.
        let default_descriptor = setup.device_descriptor.clone();
        setup.device_descriptor = std::sync::Arc::new(move |adapter| {
            let mut descriptor = default_descriptor(adapter);
            descriptor.required_limits.max_texture_dimension_2d =
                adapter.limits().max_texture_dimension_2d.min(16384);
            descriptor
        });
        options.wgpu_options.wgpu_setup = setup.into();
    }
    tracing::info!(target: "window.graphics", requested=?renderer, selected=?options.renderer, "Client renderer selected");
}
pub fn log_adapter(creation: &eframe::CreationContext<'_>) {
    if let Some(state) = &creation.wgpu_render_state {
        // Preserve the actual validation diagnostic before the fatal panic.
        // The default handler only logs "fatal"; the panic hook intentionally
        // omits arbitrary payloads, which previously hid this crash's cause.
        state.device.on_uncaptured_error(std::sync::Arc::new(|error: eframe::wgpu::Error| {
            let detail = crate::logging::redact("graphics_error", &error.to_string());
            crate::crash_diagnostics::graphics_failure(&detail);
            tracing::error!(target: "window.graphics", error=%detail, "Fatal client graphics error");
            panic!("Fatal client graphics error; see the crash log");
        }));
        let info = state.adapter.get_info();
        tracing::info!(target:"window.graphics", backend=?info.backend, adapter=%info.name, driver=%info.driver, driver_info=%info.driver_info, "Client graphics adapter");
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn windows_automatic_avoids_opengl_and_preserves_an_explicit_compatibility_choice() {
        assert_eq!(direct3d(Renderer::Automatic), cfg!(windows));
        assert!(!direct3d(Renderer::OpenGl));
        assert_eq!(direct3d(Renderer::Direct3D), cfg!(windows));
        assert_eq!(
            serde_json::from_str::<Renderer>("\"open_gl\"").unwrap(),
            Renderer::OpenGl
        );
        assert!(serde_json::from_str::<Renderer>("\"unknown\"").is_err());
        let mut options = eframe::NativeOptions::default();
        configure_selected(&mut options, Renderer::Automatic);
        if cfg!(windows) {
            assert_eq!(options.renderer, eframe::Renderer::Wgpu);
            let eframe::egui_wgpu::WgpuSetup::CreateNew(setup) = &options.wgpu_options.wgpu_setup
            else {
                panic!("Expected a new DX12 device")
            };
            assert_eq!(
                setup.instance_descriptor.backends,
                eframe::wgpu::Backends::DX12
            );
            assert!(matches!(
                setup
                    .instance_descriptor
                    .backend_options
                    .dx12
                    .shader_compiler,
                eframe::wgpu::Dx12Compiler::Fxc
            ));
        } else {
            assert_eq!(options.renderer, eframe::Renderer::Glow);
        }
        configure_selected(&mut options, Renderer::OpenGl);
        assert_eq!(options.renderer, eframe::Renderer::Glow);
    }
}
