//! Choosing the graphics backend on Windows.
//!
//! The path tracer does not survive FXC, the old DirectX shader compiler
//! wgpu falls back to when DXC is missing: CI on WARP showed a black render
//! and a crash with FXC, and the right image with DXC (`dxcompiler.dll`).
//! Windows does not ship DXC, so:
//!
//! 1. `dxcompiler.dll` next to `voxferro.exe` (a package built with
//!    `package-windows.ps1 -WithDxc`, or one you copied there): DirectX 12
//!    with DXC.
//! 2. Otherwise Vulkan, when a Vulkan driver is present (practically every
//!    GPU's): no DirectX shader compiler involved.
//! 3. Otherwise DirectX 12 as wgpu chooses, with a warning that Render mode
//!    needs DXC.
//!
//! `WGPU_BACKEND` and `WGPU_DX12_COMPILER`, if set, win over all of this.

use eframe::egui_wgpu;

/// Adjusts eframe's wgpu setup; returns a message for the log.
pub fn configure(options: &mut eframe::NativeOptions) -> Option<String> {
    if std::env::var_os("WGPU_BACKEND").is_some() || std::env::var_os("WGPU_DX12_COMPILER").is_some() {
        return None;
    }
    let egui_wgpu::WgpuSetup::CreateNew(setup) = &mut options.wgpu_options.wgpu_setup else {
        return None;
    };
    let dxc = std::env::current_exe().ok()?.parent()?.join("dxcompiler.dll");
    if dxc.is_file() {
        setup.instance_descriptor.backends = wgpu::Backends::DX12;
        setup.instance_descriptor.backend_options.dx12.shader_compiler =
            wgpu::Dx12Compiler::DynamicDxc { dxc_path: dxc.to_string_lossy().into_owned() };
        return Some(format!("graphics: DirectX 12 with DXC ({})", dxc.display()));
    }
    if vulkan_available() {
        setup.instance_descriptor.backends = wgpu::Backends::VULKAN;
        return Some("graphics: Vulkan (no dxcompiler.dll next to the program)".to_owned());
    }
    Some(
        "graphics: DirectX 12 without DXC -- Render mode will not work. Put dxcompiler.dll and \
         dxil.dll (from Microsoft's DirectXShaderCompiler releases) next to voxferro.exe"
            .to_owned(),
    )
}

/// Whether a Vulkan driver offers an adapter.
fn vulkan_available() -> bool {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::VULKAN,
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    // Enumerating is synchronous on native; poll the future once.
    let mut future = std::pin::pin!(instance.enumerate_adapters(wgpu::Backends::VULKAN));
    let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
    match future.as_mut().poll(&mut cx) {
        std::task::Poll::Ready(adapters) => !adapters.is_empty(),
        std::task::Poll::Pending => false,
    }
}
