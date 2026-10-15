//! Progressive path tracing of the voxel volume.

use eframe::egui;
use glam::{Mat3, Vec3};
use vox_format::Palette;
use voxel_core::Volume;
use wgpu::util::DeviceExt;

/// Lighting and film settings, read from the file's `rOBJ` chunks where
/// available.
#[derive(Debug, Clone, Copy)]
pub struct Environment {
    pub sun_direction: Vec3,
    pub sun_color: Vec3,
    pub sun_intensity: f32,
    pub sky_color: Vec3,
    pub sky_intensity: f32,
    /// Angular radius of the sun disc, radians. A larger disc softens
    /// shadows, which is what the editor's `_area` controls.
    pub sun_angular_radius: f32,
    pub exposure: f32,
    pub bounces: u32,
}

impl Default for Environment {
    fn default() -> Self {
        Self {
            sun_direction: Vec3::new(0.4, 0.3, 1.0).normalize(),
            sun_color: Vec3::new(1.0, 0.95, 0.85),
            sun_intensity: 1.8,
            sky_color: Vec3::new(0.45, 0.6, 0.9),
            sky_intensity: 0.4,
            sun_angular_radius: 0.08,
            exposure: 0.35,
            bounces: 3,
        }
    }
}

impl Environment {
    /// Reads what it can from the file's render objects.
    ///
    /// The editor stores lighting as `_inf` (sun), `_uni` (sky) and `_film`
    /// (exposure), all as string key/values. Anything absent keeps its
    /// default rather than rendering black.
    pub fn from_render_objects(objects: &[vox_format::Dict]) -> Self {
        let mut env = Self::default();

        for object in objects {
            match object.get("_type") {
                Some("_inf") => {
                    if let Some(k) = object.get_parsed::<f32, 3>("_k") {
                        env.sun_color = Vec3::from_array(k) / 255.0;
                    }
                    if let Some(i) = object.get("_i").and_then(|v| v.parse::<f32>().ok()) {
                        // The editor's intensity is a multiplier around 1;
                        // scale it so a default scene is not nearly black.
                        env.sun_intensity = (i * 1.8).max(0.0);
                    }
                    // `_angle` is "azimuth elevation" in degrees.
                    if let Some([azimuth, elevation]) = object.get_parsed::<f32, 2>("_angle") {
                        let (az, el) = (azimuth.to_radians(), elevation.to_radians());
                        env.sun_direction =
                            Vec3::new(el.cos() * az.cos(), el.cos() * az.sin(), el.sin())
                                .normalize_or(Vec3::Z);
                    }
                    if let Some(area) = object.get("_area").and_then(|v| v.parse::<f32>().ok()) {
                        env.sun_angular_radius = area.clamp(0.01, 0.5);
                    }
                }
                Some("_uni") => {
                    if let Some(k) = object.get_parsed::<f32, 3>("_k") {
                        env.sky_color = Vec3::from_array(k) / 255.0;
                    }
                    if let Some(i) = object.get("_i").and_then(|v| v.parse::<f32>().ok()) {
                        env.sky_intensity = i.max(0.0);
                    }
                }
                Some("_film") => {
                    if let Some(e) = object.get("_expo").and_then(|v| v.parse::<f32>().ok()) {
                        // The editor's exposure is an EV-like stop value.
                        env.exposure = (e / 3.0).clamp(0.05, 20.0);
                    }
                }
                _ => {}
            }
        }
        env
    }
}

/// One palette index's material, matching the WGSL `Material`.
///
/// 32 bytes: four scalars, then a vec4 that WGSL aligns to 16, which the four
/// scalars already reach -- no hidden padding to get wrong.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct GpuMaterial {
    kind: u32,
    roughness: f32,
    ior: f32,
    amount: f32,
    emission: [f32; 4],
}

/// Converts an emitter's `_emit` and `_flux` into a radiance multiplier.
///
/// Taken from the editor itself, not guessed: `the reference renderer`
/// computes `1 + 3 * emit * powf(10, flux)` (the constants 3.0, 10.0 and 1.0
/// read straight out of the binary). So `_flux` counts powers of ten, and the
/// `+ 1` means an emitter at `_emit = 0` still glows with its own colour.
pub fn emission_strength(emission: f32, flux: f32) -> f32 {
    1.0 + 3.0 * emission * 10f32.powf(flux)
}

/// The editor's roughness slider is far gentler than a raw GGX alpha:
/// `CalcMaterialProp` scales it by 0.15 before use. Without this, every
/// "slightly rough" metal from a real file renders visibly blurred.
pub fn effective_roughness(slider: f32) -> f32 {
    slider * 0.15
}

fn srgb_to_linear(v: u8) -> f32 {
    let v = v as f32 / 255.0;
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

/// Must match the WGSL `Params` exactly, padding included.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    right: [f32; 4],
    up: [f32; 4],
    forward: [f32; 4],
    origin: [f32; 4],

    volume_size: [u32; 4],
    resolution: [u32; 2],
    frame: u32,
    sample_count: u32,

    sun_direction: [f32; 4],
    sun_color: [f32; 4],
    sky_color: [f32; 4],

    tan_half_fov: f32,
    aspect: f32,
    exposure: f32,
    bounces: u32,

    sun_intensity: f32,
    sky_intensity: f32,
    sun_angular_radius: f32,
    reset: u32,

    volume_offset: [f32; 4],

    emitter_info: [u32; 4],
}

struct Target {
    size: [u32; 2],
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    texture_id: egui::TextureId,
    accumulator: wgpu::Buffer,
}

pub struct PathTracer {
    pipeline: wgpu::ComputePipeline,
    layout: wgpu::BindGroupLayout,
    params: wgpu::Buffer,
    palette: wgpu::Buffer,
    materials: wgpu::Buffer,
    emitters: wgpu::Buffer,
    emitter_count: u32,
    volume: Option<(wgpu::TextureView, [u32; 3])>,
    target: Option<Target>,
    /// Frames accumulated since the last reset.
    pub samples: u32,
    reset_pending: bool,
}

impl PathTracer {
    pub fn new(device: &wgpu::Device) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("path tracer"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/pathtrace.wgsl").into()),
        });

        let uniform = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let storage = |binding, read_only| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };

        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("path tracer layout"),
            entries: &[
                uniform(0),
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Uint,
                        view_dimension: wgpu::TextureViewDimension::D3,
                        multisampled: false,
                    },
                    count: None,
                },
                storage(2, true),
                storage(3, false),
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format: wgpu::TextureFormat::Rgba8Unorm,
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                    count: None,
                },
                storage(5, true),
                storage(6, true),
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("path tracer pipeline layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });

        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("path tracer pipeline"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });

        let params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("path tracer params"),
            size: std::mem::size_of::<Params>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let palette = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("path tracer palette"),
            size: (256 * 16) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // Every slot starts as plain diffuse, so a file without MATL chunks
        // renders exactly as before.
        let defaults = [GpuMaterial {
            kind: 0,
            roughness: 0.1,
            ior: 1.3,
            amount: 0.0,
            emission: [0.0; 4],
        }; 256];
        let materials = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("path tracer materials"),
            contents: bytemuck::cast_slice(&defaults),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });

        Self {
            pipeline,
            layout,
            params,
            palette,
            materials,
            emitters: empty_emitter_buffer(device),
            emitter_count: 0,
            volume: None,
            target: None,
            samples: 0,
            reset_pending: true,
        }
    }

    /// Discards the accumulated image. Call whenever the view or the scene
    /// changes, or the average mixes frames of two different pictures.
    pub fn reset(&mut self) {
        self.samples = 0;
        self.reset_pending = true;
    }

    /// Uploads the volume as a 3D texture.
    pub fn set_volume(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, volume: &Volume) {
        let [sx, sy, sz] = volume.size();
        if sx == 0 || sy == 0 || sz == 0 {
            self.volume = None;
            return;
        }

        let mut data = Vec::with_capacity((sx * sy * sz) as usize);
        for z in 0..sz {
            for y in 0..sy {
                for x in 0..sx {
                    data.push(volume.get(x, y, z));
                }
            }
        }

        let texture = device.create_texture_with_data(
            queue,
            &wgpu::TextureDescriptor {
                label: Some("voxel volume"),
                size: wgpu::Extent3d {
                    width: sx,
                    height: sy,
                    depth_or_array_layers: sz,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D3,
                // R8Uint holds a palette index per voxel, read with
                // textureLoad at integer coordinates -- no sampler, no
                // filtering, which is exactly what a DDA walk wants.
                format: wgpu::TextureFormat::R8Uint,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            &data,
        );

        self.volume = Some((texture.create_view(&Default::default()), [sx, sy, sz]));
        self.reset();
    }

    /// Uploads one material per palette index.
    ///
    /// Emission is baked into radiance here, from the palette colour, so the
    /// shader adds it without knowing about amounts or flux.
    pub fn set_materials(
        &mut self,
        queue: &wgpu::Queue,
        palette: &Palette,
        materials: &[vox_format::Material],
    ) {
        let mut table = [GpuMaterial {
            kind: 0,
            roughness: 0.1,
            ior: 1.3,
            amount: 0.0,
            emission: [0.0; 4],
        }; 256];

        for material in materials {
            // MATL ids are palette indices; anything outside 1..=255 cannot
            // be referenced by a voxel.
            let Ok(index) = u8::try_from(material.id) else { continue };
            if index == 0 {
                continue;
            }
            let r = material.resolve();
            let c = palette.color(index);
            let strength = if r.kind == vox_format::MaterialKind::Emissive {
                emission_strength(r.emission, r.flux)
            } else {
                0.0
            };

            table[index as usize] = GpuMaterial {
                kind: r.kind as u32,
                roughness: effective_roughness(r.roughness),
                ior: r.ior,
                amount: match r.kind {
                    vox_format::MaterialKind::Metal => r.metalness,
                    vox_format::MaterialKind::Glass => r.transparency,
                    _ => 0.0,
                },
                // w carries the absorption density; the vec4 had a free lane.
                emission: [
                    srgb_to_linear(c.r) * strength,
                    srgb_to_linear(c.g) * strength,
                    srgb_to_linear(c.b) * strength,
                    r.density,
                ],
            };
        }

        queue.write_buffer(&self.materials, 0, bytemuck::cast_slice(&table));
        self.reset();
    }

    /// Builds the list of emissive voxels the shader samples directly.
    ///
    /// Only cells with at least one face open to the air are kept, with a
    /// bitmask of which faces those are. A solid lamp of 10x10x18 voxels has
    /// 1800 emissive cells but only its shell can light anything; sampling the
    /// interior would spend every one of those shadow rays on occlusion.
    pub fn set_emitters(
        &mut self,
        device: &wgpu::Device,
        volume: &Volume,
        materials: &[vox_format::Material],
    ) {
        let list = exposed_emitters(volume, materials);
        self.emitter_count = list.len() as u32;
        self.emitters = if list.is_empty() {
            empty_emitter_buffer(device)
        } else {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("path tracer emitters"),
                contents: bytemuck::cast_slice(&list),
                usage: wgpu::BufferUsages::STORAGE,
            })
        };
        self.reset();
    }

    pub fn emitter_count(&self) -> u32 {
        self.emitter_count
    }

    pub fn set_palette(&mut self, queue: &wgpu::Queue, palette: &Palette) {
        let mut data = [[0.0f32; 4]; 256];
        for (i, c) in palette.as_slice().iter().enumerate() {
            // Albedo must be linear for the light transport to be correct;
            // palette entries are sRGB-encoded.
            data[i] = [srgb_to_linear(c.r), srgb_to_linear(c.g), srgb_to_linear(c.b), 1.0];
        }
        queue.write_buffer(&self.palette, 0, bytemuck::cast_slice(&data));
    }

    fn ensure_target(
        &mut self,
        device: &wgpu::Device,
        renderer: &mut eframe::egui_wgpu::Renderer,
        size: [u32; 2],
    ) {
        if self.target.as_ref().is_some_and(|t| t.size == size) {
            return;
        }

        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("path traced image"),
            size: wgpu::Extent3d {
                width: size[0],
                height: size[1],
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::STORAGE_BINDING
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());

        let accumulator = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("path tracer accumulator"),
            size: (size[0] as u64) * (size[1] as u64) * 16,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });

        let texture_id = match self.target.take() {
            Some(old) => {
                renderer.update_egui_texture_from_wgpu_texture(
                    device,
                    &view,
                    wgpu::FilterMode::Linear,
                    old.texture_id,
                );
                old.texture_id
            }
            None => renderer.register_native_texture(device, &view, wgpu::FilterMode::Linear),
        };

        self.target = Some(Target {
            size,
            texture,
            view,
            texture_id,
            accumulator,
        });
        self.reset();
    }

    /// Renders one more sample per pixel and returns the image.
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        renderer: &mut eframe::egui_wgpu::Renderer,
        size: [u32; 2],
        eye: Vec3,
        basis: Mat3,
        fov_y: f32,
        environment: &Environment,
        volume_offset: Vec3,
    ) -> Option<egui::TextureId> {
        // Size the target before borrowing the volume: ensure_target needs
        // &mut self, which cannot coexist with a live borrow of self.volume.
        self.volume.as_ref()?;
        self.ensure_target(device, renderer, size);

        let (volume_view, volume_size) = self.volume.as_ref()?;
        let volume_size = *volume_size;
        let target = self.target.as_ref()?;

        let reset = self.reset_pending;
        self.reset_pending = false;
        if reset {
            self.samples = 0;
        }

        let params = Params {
            right: basis.x_axis.extend(0.0).to_array(),
            up: basis.y_axis.extend(0.0).to_array(),
            forward: basis.z_axis.extend(0.0).to_array(),
            origin: eye.extend(0.0).to_array(),
            volume_size: [volume_size[0], volume_size[1], volume_size[2], 0],
            resolution: size,
            frame: self.samples,
            sample_count: self.samples + 1,
            sun_direction: environment.sun_direction.extend(0.0).to_array(),
            sun_color: environment.sun_color.extend(0.0).to_array(),
            sky_color: environment.sky_color.extend(0.0).to_array(),
            tan_half_fov: (fov_y * 0.5).tan(),
            aspect: size[0] as f32 / size[1].max(1) as f32,
            exposure: environment.exposure,
            bounces: environment.bounces,
            sun_intensity: environment.sun_intensity,
            sky_intensity: environment.sky_intensity,
            sun_angular_radius: environment.sun_angular_radius,
            reset: reset as u32,
            volume_offset: volume_offset.extend(0.0).to_array(),
            emitter_info: [self.emitter_count, 0, 0, 0],
        };
        queue.write_buffer(&self.params, 0, bytemuck::bytes_of(&params));

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("path tracer bind group"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.params.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(volume_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: self.palette.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: target.accumulator.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::TextureView(&target.view),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: self.materials.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: self.emitters.as_entire_binding(),
                },
            ],
        });

        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_compute_pass(&Default::default());
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.dispatch_workgroups(size[0].div_ceil(8), size[1].div_ceil(8), 1);
        }
        queue.submit(Some(encoder.finish()));

        self.samples += 1;
        Some(target.texture_id)
    }

    /// Reads the current image back as RGBA bytes.
    pub fn read_frame(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> Option<(u32, u32, Vec<u8>)> {
        let target = self.target.as_ref()?;
        let [width, height] = target.size;

        const ALIGN: u32 = 256;
        let unpadded = width * 4;
        let padded = unpadded.div_ceil(ALIGN) * ALIGN;

        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("path trace readback"),
            size: (padded * height) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        let mut encoder = device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            target.texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        queue.submit(Some(encoder.finish()));

        let slice = buffer.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        device.poll(wgpu::PollType::wait_indefinitely()).ok()?;

        let mapped = slice.get_mapped_range().ok()?;
        let mut out = Vec::with_capacity((unpadded * height) as usize);
        for row in 0..height {
            let start = (row * padded) as usize;
            out.extend_from_slice(&mapped[start..start + unpadded as usize]);
        }
        drop(mapped);
        buffer.unmap();

        Some((width, height, out))
    }
}

/// A storage binding cannot be zero-sized, so "no emitters" is one unused
/// entry plus a count of zero.
fn empty_emitter_buffer(device: &wgpu::Device) -> wgpu::Buffer {
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("path tracer emitters (empty)"),
        contents: bytemuck::cast_slice(&[[0i32; 4]]),
        usage: wgpu::BufferUsages::STORAGE,
    })
}

/// Emissive cells with at least one empty neighbour, as `[x, y, z, mask]`.
///
/// Mask bit `2 * axis` is the negative face, `2 * axis + 1` the positive one,
/// matching how the shader picks the face that looks at a shading point.
pub fn exposed_emitters(volume: &Volume, materials: &[vox_format::Material]) -> Vec<[i32; 4]> {
    let mut emissive = [false; 256];
    for m in materials {
        if let Ok(i) = u8::try_from(m.id) {
            emissive[i as usize] = m.resolve().kind == vox_format::MaterialKind::Emissive;
        }
    }
    emissive[0] = false;

    let [sx, sy, sz] = volume.size();
    let mut out = Vec::new();
    for z in 0..sz as i32 {
        for y in 0..sy as i32 {
            for x in 0..sx as i32 {
                if !emissive[volume.get_i(x, y, z) as usize] {
                    continue;
                }
                let mut mask = 0i32;
                for axis in 0..3 {
                    for (side, step) in [(0, -1), (1, 1)] {
                        let mut n = [x, y, z];
                        n[axis] += step;
                        if volume.get_i(n[0], n[1], n[2]) == 0 {
                            mask |= 1 << (axis * 2 + side);
                        }
                    }
                }
                if mask != 0 {
                    out.push([x, y, z, mask]);
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn emit_material(id: i32) -> vox_format::Material {
        vox_format::Material {
            id,
            properties: vox_format::Dict(vec![("_type".into(), "_emit".into())]),
        }
    }

    #[test]
    fn only_the_shell_of_a_solid_lamp_is_sampled() {
        let mut v = Volume::new([5, 5, 5]);
        for z in 0..3 {
            for y in 0..3 {
                for x in 0..3 {
                    v.set(x + 1, y + 1, z + 1, 9);
                }
            }
        }
        let list = exposed_emitters(&v, &[emit_material(9)]);
        // A 3x3x3 block has 27 cells; the centre one is fully enclosed.
        assert_eq!(list.len(), 26);
        assert!(!list.iter().any(|e| e[0..3] == [2, 2, 2]));
    }

    #[test]
    fn face_mask_marks_open_sides_only() {
        let mut v = Volume::new([3, 1, 1]);
        v.set(0, 0, 0, 9);
        v.set(1, 0, 0, 9);
        let list = exposed_emitters(&v, &[emit_material(9)]);
        let left = list.iter().find(|e| e[0] == 0).unwrap();
        // -X is outside the grid (empty), +X touches the other emitter.
        assert_ne!(left[3] & 0b01, 0, "-x face is open");
        assert_eq!(left[3] & 0b10, 0, "+x face is covered");
    }

    #[test]
    fn non_emissive_voxels_are_ignored() {
        let mut v = Volume::new([2, 2, 2]);
        v.set(0, 0, 0, 3);
        assert!(exposed_emitters(&v, &[emit_material(9)]).is_empty());
    }

    #[test]
    fn emission_matches_the_editor_formula() {
        // room.vox's lamps: _emit 1 (from _weight), _flux 2.
        assert_eq!(emission_strength(1.0, 2.0), 301.0);
        // An emitter with no emission still shows its colour.
        assert_eq!(emission_strength(0.0, 3.0), 1.0);
        // Each flux step is a factor of ten, not two.
        let ratio = (emission_strength(1.0, 2.0) - 1.0) / (emission_strength(1.0, 1.0) - 1.0);
        assert!((ratio - 10.0).abs() < 1e-3);
    }

    #[test]
    fn roughness_is_scaled_like_the_editor() {
        assert!((effective_roughness(1.0) - 0.15).abs() < 1e-6);
        assert_eq!(effective_roughness(0.0), 0.0);
    }
}
