//! Running voxel operations on the GPU.

use voxel_core::Volume;
use wgpu::util::DeviceExt;

/// Which shape a kernel evaluates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    Sphere = 0,
    Torus = 1,
    RoundBox = 2,
    Shell = 3,
}

impl Shape {
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "sphere" => Some(Self::Sphere),
            "torus" => Some(Self::Torus),
            "box" => Some(Self::RoundBox),
            "shell" => Some(Self::Shell),
            _ => None,
        }
    }

    pub const ALL: [&'static str; 4] = ["sphere", "torus", "box", "shell"];
}

/// Must match the WGSL `Params` exactly, including padding.
///
/// WGSL aligns a `vec3<u32>` to 16 bytes, so `size` occupies 16 and the
/// scalars that follow pack into the next slots. Getting this wrong silently
/// shifts every field and produces nonsense rather than an error.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    size: [u32; 3],
    color: u32,
    mode: u32,
    radius: f32,
    thickness: f32,
    replace: u32,
}

pub struct VoxelCompute {
    pipeline: wgpu::ComputePipeline,
    layout: wgpu::BindGroupLayout,
}

impl VoxelCompute {
    pub fn new(device: &wgpu::Device) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("voxel ops"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/voxel_ops.wgsl").into()),
        });

        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("voxel ops layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("voxel ops pipeline layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });

        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("voxel ops pipeline"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });

        Self { pipeline, layout }
    }
}

/// What a kernel run should do.
///
/// Grouped into a struct rather than passed as a long argument list: at nine
/// parameters the call site stops being readable and adjacent floats become
/// easy to swap by accident.
#[derive(Debug, Clone, Copy)]
pub struct Op {
    pub shape: Shape,
    pub color: u8,
    pub radius: f32,
    pub thickness: f32,
    /// Clear cells the shape does not cover, instead of only adding.
    pub replace: bool,
}

impl VoxelCompute {
    /// Evaluates `op.shape` over the whole volume on the GPU.
    ///
    /// The volume is uploaded, processed and read back. A real editor would
    /// keep it resident on the GPU and only read back when saving; round
    /// tripping keeps the CPU-side `Volume` the single source of truth while
    /// the rest of the editor still works on it.
    pub fn run(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        volume: &Volume,
        op: Op,
    ) -> Option<Vec<u8>> {
        let [sx, sy, sz] = volume.size();
        let count = (sx as usize) * (sy as usize) * (sz as usize);
        if count == 0 {
            return None;
        }

        // Upload the current contents so a kernel can build on them.
        let mut data = Vec::with_capacity(count);
        for z in 0..sz {
            for y in 0..sy {
                for x in 0..sx {
                    data.push(volume.get(x, y, z) as u32);
                }
            }
        }

        let storage = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("voxel storage"),
            contents: bytemuck::cast_slice(&data),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        });

        let params = Params {
            size: [sx, sy, sz],
            color: op.color as u32,
            mode: op.shape as u32,
            radius: op.radius,
            thickness: op.thickness,
            replace: op.replace as u32,
        };
        let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("voxel ops params"),
            contents: bytemuck::bytes_of(&params),
            usage: wgpu::BufferUsages::UNIFORM,
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("voxel ops bind group"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: storage.as_entire_binding(),
                },
            ],
        });

        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("voxel readback"),
            size: (count * 4) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_compute_pass(&Default::default());
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            // Round up: the kernel discards invocations past the volume.
            pass.dispatch_workgroups(sx.div_ceil(4), sy.div_ceil(4), sz.div_ceil(4));
        }
        encoder.copy_buffer_to_buffer(&storage, 0, &readback, 0, (count * 4) as u64);
        queue.submit(Some(encoder.finish()));

        let slice = readback.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        device.poll(wgpu::PollType::wait_indefinitely()).ok()?;

        let mapped = slice.get_mapped_range().ok()?;
        let words: &[u32] = bytemuck::cast_slice(&mapped);
        let out = words.iter().map(|&w| w as u8).collect();
        drop(mapped);
        readback.unmap();

        Some(out)
    }
}
