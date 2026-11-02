//! Choosing the graphics backend on Windows.
//!
//! Measured on Windows with real GPUs (ROADMAP, "Checked on Windows"): the
//! path tracer's build with voxel shapes runs 6 to 12 times slower under
//! DirectX 12 than under Vulkan (a sphere sample at 790x730 on an RTX 4070
//! Ti: 148 ms against 22.5; an integrated AMD, 10 to 12 times slower), and
//! compiling it through DXC costs ~6 s a launch, where Vulkan's driver
//! caches it. With cubes the two are even. So, in order:
//!
//! 1. Vulkan, when a Vulkan driver offers a hardware adapter -- unless the
//!    machine has a discrete GPU that only DirectX 12 offers, which Vulkan
//!    would trade for the integrated one.
//! 2. Otherwise `dxcompiler.dll` next to `voxferro.exe` (the release zip
//!    ships it, `package-windows.ps1 -WithDxc`): DirectX 12 with DXC. The
//!    path tracer does not survive FXC, the old compiler wgpu falls back to
//!    without DXC: CI on WARP showed a black render and a crash with FXC,
//!    and the right image with DXC. CI runners have no GPU, hence no Vulkan
//!    adapter, and land here.
//! 3. Otherwise DirectX 12 as wgpu chooses, with a warning that Render mode
//!    needs DXC.
//!
//! `WGPU_BACKEND` and `WGPU_DX12_COMPILER`, if set, win over all of this.

use eframe::egui_wgpu;

/// The backend [`configure`] settles on.
#[derive(Debug, PartialEq)]
enum Choice {
    Vulkan,
    Dx12WithDxc,
    Dx12WithoutDxc,
}

/// The choice, from the kinds of adapter each API offers and whether DXC
/// sits next to the program.
fn choose(vulkan: &[wgpu::DeviceType], dx12: &[wgpu::DeviceType], dxc: bool) -> Choice {
    let hardware = |t: &&wgpu::DeviceType| !matches!(t, wgpu::DeviceType::Cpu);
    let discrete = |types: &[wgpu::DeviceType]| types.contains(&wgpu::DeviceType::DiscreteGpu);
    let vulkan_has_hardware = vulkan.iter().any(|t| hardware(&t));
    if vulkan_has_hardware && (discrete(vulkan) || !discrete(dx12)) {
        Choice::Vulkan
    } else if dxc {
        Choice::Dx12WithDxc
    } else if vulkan_has_hardware {
        // No DXC: Vulkan on the integrated GPU still renders; DirectX 12
        // without DXC does not.
        Choice::Vulkan
    } else {
        Choice::Dx12WithoutDxc
    }
}

/// Adjusts eframe's wgpu setup; returns a message for the log.
pub fn configure(options: &mut eframe::NativeOptions) -> Option<String> {
    if std::env::var_os("WGPU_BACKEND").is_some() || std::env::var_os("WGPU_DX12_COMPILER").is_some() {
        return None;
    }
    let egui_wgpu::WgpuSetup::CreateNew(setup) = &mut options.wgpu_options.wgpu_setup else {
        return None;
    };
    let dxc = std::env::current_exe().ok()?.parent()?.join("dxcompiler.dll");
    match choose(&adapter_types(wgpu::Backends::VULKAN), &adapter_types(wgpu::Backends::DX12), dxc.is_file()) {
        Choice::Vulkan => {
            setup.instance_descriptor.backends = wgpu::Backends::VULKAN;
            Some("graphics: Vulkan".to_owned())
        }
        Choice::Dx12WithDxc => {
            setup.instance_descriptor.backends = wgpu::Backends::DX12;
            setup.instance_descriptor.backend_options.dx12.shader_compiler =
                wgpu::Dx12Compiler::DynamicDxc { dxc_path: dxc.to_string_lossy().into_owned() };
            Some(format!("graphics: DirectX 12 with DXC ({})", dxc.display()))
        }
        Choice::Dx12WithoutDxc => Some(
            "graphics: DirectX 12 without DXC -- Render mode will not work. Put dxcompiler.dll and \
             dxil.dll (from Microsoft's DirectXShaderCompiler releases) next to voxferro.exe"
                .to_owned(),
        ),
    }
}

/// The kinds of adapter one API offers, none when it has no driver.
fn adapter_types(backends: wgpu::Backends) -> Vec<wgpu::DeviceType> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends,
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    // Enumerating is synchronous on native; poll the future once.
    let mut future = std::pin::pin!(instance.enumerate_adapters(backends));
    let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
    match future.as_mut().poll(&mut cx) {
        std::task::Poll::Ready(adapters) => adapters.iter().map(|a| a.get_info().device_type).collect(),
        std::task::Poll::Pending => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::{Choice, choose};
    use wgpu::DeviceType::{Cpu, DiscreteGpu, IntegratedGpu};

    #[test]
    fn vulkan_first_when_it_reaches_the_best_gpu() {
        // This machine: an RTX and an integrated AMD, both in both APIs.
        let both = [DiscreteGpu, IntegratedGpu];
        assert_eq!(choose(&both, &both, true), Choice::Vulkan);
        assert_eq!(choose(&both, &both, false), Choice::Vulkan);
        // A laptop with only its integrated GPU.
        assert_eq!(choose(&[IntegratedGpu], &[IntegratedGpu], true), Choice::Vulkan);
    }

    #[test]
    fn dx12_when_vulkan_would_lose_the_discrete_gpu_or_is_missing() {
        // The discrete GPU has no Vulkan driver, the integrated one does.
        assert_eq!(choose(&[IntegratedGpu], &[DiscreteGpu, IntegratedGpu], true), Choice::Dx12WithDxc);
        // Without DXC DirectX 12 cannot render: the integrated GPU it is.
        assert_eq!(choose(&[IntegratedGpu], &[DiscreteGpu, IntegratedGpu], false), Choice::Vulkan);
        // CI: no GPU, WARP under DirectX 12, maybe a software Vulkan.
        assert_eq!(choose(&[], &[Cpu], true), Choice::Dx12WithDxc);
        assert_eq!(choose(&[Cpu], &[Cpu], true), Choice::Dx12WithDxc);
        assert_eq!(choose(&[Cpu], &[Cpu], false), Choice::Dx12WithoutDxc);
    }
}
