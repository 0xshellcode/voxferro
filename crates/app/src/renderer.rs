//! Renders the voxel mesh with wgpu, into a texture egui can display.
//!
//! Drawing into egui's own render pass would be simpler, but that pass has no
//! depth attachment — fine for a flat triangle, useless for solid geometry.
//! So the scene goes into an offscreen colour+depth target, which is then
//! registered as an egui texture and shown like an image.

use eframe::{egui, egui_wgpu};
use glam::{Mat4, Vec3};
use voxel_core::{Mesh, Vertex};
use wgpu::util::DeviceExt;

/// Values shared by every vertex and fragment of a frame.
///
/// `Pod` lets us hand the struct straight to the GPU as bytes. The layout must
/// match the WGSL `Uniforms`, and WGSL aligns a `mat4x4` and each `vec4` to 16
/// bytes, which is why the light and ambient are `vec4` and not `vec3`.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniforms {
    view_proj: [[f32; 4]; 4],
    light_dir: [f32; 4],
    ambient: [f32; 4],
    /// x: draw voxel edges (1) or not (0); y: shadows.
    options: [f32; 4],
    /// World to the shadow map's clip space.
    light_view_proj: [[f32; 4]; 4],
    /// x: constant shading, y: how much a faded object keeps, z: frame
    /// spacing (0 none).
    view: [f32; 4],
    background: [f32; 4],
    edge_color: [f32; 4],
    grid_color: [f32; 4],
    select_color: [f32; 4],
}

/// The editor options the voxel shader draws with.
#[derive(Debug, Clone, Copy)]
pub struct Shading {
    pub constant: bool,
    pub fade: f32,
    pub frame_spacing: u32,
    /// Colours as display values, 0 to 1.
    pub background: [f32; 3],
    pub edge: [f32; 3],
    pub grid: [f32; 3],
    pub select: [f32; 3],
}

/// The shadow map's side, in texels.
const SHADOW_SIZE: u32 = 2048;

/// 256 RGBA entries. 4 KiB, well inside the uniform buffer limit.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct PaletteUniform {
    colors: [[f32; 4]; 256],
}

/// A vertex of a guide line, in world space.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct LineVertex {
    pub position: [f32; 3],
    pub color: [f32; 4],
}

/// The offscreen target and the egui texture pointing at it.
struct Target {
    size: [u32; 2],
    color_texture: wgpu::Texture,
    color_view: wgpu::TextureView,
    depth_view: wgpu::TextureView,
    texture_id: egui::TextureId,
}

pub struct SceneRenderer {
    pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    uniform_buffer: wgpu::Buffer,
    palette_buffer: wgpu::Buffer,
    color_format: wgpu::TextureFormat,

    vertex_buffer: Option<wgpu::Buffer>,
    index_buffer: Option<wgpu::Buffer>,
    index_count: u32,

    /// Guide lines: the selected model's box and floor grid.
    line_pipeline: wgpu::RenderPipeline,
    line_buffer: Option<wgpu::Buffer>,
    line_vertices: u32,
    /// The mesh's quad outlines, when wireframes are shown.
    wire_buffer: Option<wgpu::Buffer>,
    wire_vertices: u32,

    target: Option<Target>,

    /// Viewport shadows: a depth map from the light, drawn before the
    /// scene when shown.
    shadow_pipeline: wgpu::RenderPipeline,
    shadow_bind_group: wgpu::BindGroup,
    shadow_view: wgpu::TextureView,
    /// The mesh's world bounds, which the shadow map covers.
    bounds: Option<(Vec3, Vec3)>,
}

const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

impl SceneRenderer {
    pub fn new(device: &wgpu::Device, color_format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("voxel shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/voxel.wgsl").into()),
        });

        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("uniforms"),
            size: std::mem::size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let palette_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("palette"),
            size: std::mem::size_of::<PaletteUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("scene bind group layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
                    count: None,
                },
            ],
        });

        let shadow_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("shadow map"),
            size: wgpu::Extent3d { width: SHADOW_SIZE, height: SHADOW_SIZE, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let shadow_view = shadow_texture.create_view(&Default::default());
        let shadow_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("shadow sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            compare: Some(wgpu::CompareFunction::LessEqual),
            ..Default::default()
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("scene bind group"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: palette_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&shadow_view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(&shadow_sampler),
                },
            ],
        });

        // The shadow pass binds the uniforms alone: the map it writes can't
        // be bound for reading in the same pass.
        let shadow_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("shadow bind group layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let shadow_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("shadow bind group"),
            layout: &shadow_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: uniform_buffer.as_entire_binding() }],
        });
        let shadow_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("shadow shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/shadow.wgsl").into()),
        });
        let shadow_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("shadow pipeline"),
            layout: Some(&device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("shadow pipeline layout"),
                bind_group_layouts: &[Some(&shadow_layout)],
                immediate_size: 0,
            })),
            vertex: wgpu::VertexState {
                module: &shadow_shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Vertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x3],
                })],
            },
            fragment: None,
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: Default::default(),
                // Pushed back a little so a face doesn't shadow itself.
                bias: wgpu::DepthBiasState { constant: 2, slope_scale: 2.0, clamp: 0.0 },
            }),
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("scene pipeline layout"),
            bind_group_layouts: &[Some(&layout)],
            // wgpu 30 replaced push_constant_ranges with an immediate-data
            // size; we use none.
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("voxel pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Vertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![
                        0 => Float32x3,  // position
                        1 => Float32x3,  // normal
                        2 => Float32,    // palette index
                    ],
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(color_format.into())],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                // The mesher winds every face counter-clockwise seen from
                // outside, so back-face culling halves the work for free.
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: Some(wgpu::Face::Back),
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });

        let line_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("line shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/lines.wgsl").into()),
        });
        let line_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("line pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &line_shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<LineVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x4],
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &line_shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: color_format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::LineList,
                ..Default::default()
            },
            // Hidden behind voxels, but never hiding anything itself.
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::LessEqual),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });

        Self {
            shadow_pipeline,
            shadow_bind_group,
            shadow_view,
            bounds: None,
            line_pipeline,
            line_buffer: None,
            line_vertices: 0,
            wire_buffer: None,
            wire_vertices: 0,
            pipeline,
            bind_group,
            uniform_buffer,
            palette_buffer,
            color_format,
            vertex_buffer: None,
            index_buffer: None,
            index_count: 0,
            target: None,
        }
    }

    /// Replaces the guide lines: pairs of vertices, one segment each.
    pub fn set_lines(&mut self, device: &wgpu::Device, vertices: &[LineVertex]) {
        self.line_vertices = vertices.len() as u32;
        self.line_buffer = (!vertices.is_empty()).then(|| {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("guide lines"),
                contents: bytemuck::cast_slice(vertices),
                usage: wgpu::BufferUsages::VERTEX,
            })
        });
    }

    /// The outline of every quad of `mesh` as lines, or none. The mesher
    /// writes each quad as four vertices and the indices `0 1 2 0 2 3`.
    pub fn set_wire(&mut self, device: &wgpu::Device, mesh: Option<&Mesh>) {
        const WIRE: [f32; 4] = [0.0, 0.0, 0.0, 0.5];
        let mut lines = Vec::new();
        if let Some(mesh) = mesh {
            for quad in mesh.indices.as_chunks::<6>().0 {
                let corners = [quad[0], quad[1], quad[2], quad[5]];
                for k in 0..4 {
                    for v in [corners[k], corners[(k + 1) % 4]] {
                        // A hair off the face, so the depth test keeps the
                        // line in front of the face it outlines.
                        let vertex = &mesh.vertices[v as usize];
                        let position = std::array::from_fn(|i| vertex.position[i] + vertex.normal[i] * 0.01);
                        lines.push(LineVertex { position, color: WIRE });
                    }
                }
            }
        }
        self.wire_vertices = lines.len() as u32;
        self.wire_buffer = (!lines.is_empty()).then(|| {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("wireframe"),
                contents: bytemuck::cast_slice(&lines),
                usage: wgpu::BufferUsages::VERTEX,
            })
        });
    }

    /// Uploads a mesh, replacing whatever was there.
    pub fn set_mesh(&mut self, device: &wgpu::Device, mesh: &Mesh) {
        self.bounds = mesh.bounds();
        if mesh.is_empty() {
            self.vertex_buffer = None;
            self.index_buffer = None;
            self.index_count = 0;
            return;
        }

        self.vertex_buffer = Some(device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("voxel vertices"),
            contents: bytemuck::cast_slice(&mesh.vertices),
            usage: wgpu::BufferUsages::VERTEX,
        }));
        self.index_buffer = Some(device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("voxel indices"),
            contents: bytemuck::cast_slice(&mesh.indices),
            usage: wgpu::BufferUsages::INDEX,
        }));
        self.index_count = mesh.indices.len() as u32;
    }

    pub fn set_palette(&mut self, queue: &wgpu::Queue, palette: &vox_format::Palette) {
        let mut uniform = PaletteUniform {
            colors: [[0.0; 4]; 256],
        };
        for (i, c) in palette.as_slice().iter().enumerate() {
            // The palette is stored non-linear (sRGB). The render target is an
            // sRGB format, so the hardware converts on write; feeding raw
            // values straight through keeps colours matching the editor.
            uniform.colors[i] = [
                c.r as f32 / 255.0,
                c.g as f32 / 255.0,
                c.b as f32 / 255.0,
                c.a as f32 / 255.0,
            ];
        }
        queue.write_buffer(&self.palette_buffer, 0, bytemuck::bytes_of(&uniform));
    }

    /// Ensures the offscreen target matches `size`, recreating it on resize.
    fn ensure_target(
        &mut self,
        device: &wgpu::Device,
        renderer: &mut egui_wgpu::Renderer,
        size: [u32; 2],
    ) {
        if self.target.as_ref().is_some_and(|t| t.size == size) {
            return;
        }

        let color = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("scene colour"),
            size: wgpu::Extent3d {
                width: size[0],
                height: size[1],
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.color_format,
            // COPY_SRC so a rendered frame can be read back for a screenshot.
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let depth = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("scene depth"),
            size: wgpu::Extent3d {
                width: size[0],
                height: size[1],
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });

        let color_view = color.create_view(&Default::default());
        let depth_view = depth.create_view(&Default::default());

        // Reuse the existing egui texture id on resize, so the image widget
        // does not flicker and no ids leak.
        let texture_id = match self.target.take() {
            Some(old) => {
                renderer.update_egui_texture_from_wgpu_texture(
                    device,
                    &color_view,
                    wgpu::FilterMode::Nearest,
                    old.texture_id,
                );
                old.texture_id
            }
            None => {
                renderer.register_native_texture(device, &color_view, wgpu::FilterMode::Nearest)
            }
        };

        self.target = Some(Target {
            size,
            color_texture: color,
            color_view,
            depth_view,
            texture_id,
        });
    }

    /// Draws the scene and returns the egui texture holding the result.
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        renderer: &mut egui_wgpu::Renderer,
        size: [u32; 2],
        view_proj: Mat4,
        light_dir: Vec3,
        ambient: Vec3,
        edges: bool,
        shadows: bool,
        shading: &Shading,
    ) -> egui::TextureId {
        let clear = shading.background;
        self.ensure_target(device, renderer, size);
        let target = self.target.as_ref().expect("target was just ensured");

        let uniforms = Uniforms {
            view_proj: view_proj.to_cols_array_2d(),
            light_dir: [light_dir.x, light_dir.y, light_dir.z, 0.0],
            ambient: [ambient.x, ambient.y, ambient.z, 1.0],
            options: [edges as u8 as f32, shadows as u8 as f32, 0.0, 0.0],
            light_view_proj: self.light_view_proj(light_dir).to_cols_array_2d(),
            view: [shading.constant as u8 as f32, shading.fade, shading.frame_spacing as f32, 0.0],
            background: [shading.background[0], shading.background[1], shading.background[2], 1.0],
            edge_color: [shading.edge[0], shading.edge[1], shading.edge[2], 1.0],
            grid_color: [shading.grid[0], shading.grid[1], shading.grid[2], 1.0],
            select_color: [shading.select[0], shading.select[1], shading.select[2], 1.0],
        };
        queue.write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&uniforms));

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("scene encoder"),
        });
        if shadows {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("shadow pass"),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.shadow_view,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            if let (Some(vb), Some(ib)) = (&self.vertex_buffer, &self.index_buffer) {
                pass.set_pipeline(&self.shadow_pipeline);
                pass.set_bind_group(0, &self.shadow_bind_group, &[]);
                pass.set_vertex_buffer(0, vb.slice(..));
                pass.set_index_buffer(ib.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..self.index_count, 0, 0..1);
            }
        }
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("scene pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target.color_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: clear[0] as f64,
                            g: clear[1] as f64,
                            b: clear[2] as f64,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &target.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });

            if let (Some(vb), Some(ib)) = (&self.vertex_buffer, &self.index_buffer) {
                pass.set_pipeline(&self.pipeline);
                pass.set_bind_group(0, &self.bind_group, &[]);
                pass.set_vertex_buffer(0, vb.slice(..));
                pass.set_index_buffer(ib.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..self.index_count, 0, 0..1);
            }
            if let Some(wire) = &self.wire_buffer {
                pass.set_pipeline(&self.line_pipeline);
                pass.set_bind_group(0, &self.bind_group, &[]);
                pass.set_vertex_buffer(0, wire.slice(..));
                pass.draw(0..self.wire_vertices, 0..1);
            }
            if let Some(lines) = &self.line_buffer {
                pass.set_pipeline(&self.line_pipeline);
                pass.set_bind_group(0, &self.bind_group, &[]);
                pass.set_vertex_buffer(0, lines.slice(..));
                pass.draw(0..self.line_vertices, 0..1);
            }
        }
        queue.submit(Some(encoder.finish()));

        target.texture_id
    }

    /// The light's view of the mesh's bounds: orthographic, looking along
    /// `-light_dir`, just covering the bounding sphere.
    fn light_view_proj(&self, light_dir: Vec3) -> Mat4 {
        let Some((lo, hi)) = self.bounds else { return Mat4::IDENTITY };
        let centre = (lo + hi) * 0.5;
        let radius = ((hi - lo).length() * 0.5).max(1.0);
        let dir = light_dir.normalize_or(Vec3::Z);
        let up = if dir.z.abs() > 0.99 { Vec3::Y } else { Vec3::Z };
        // The DirectX-style projection, depth 0..1, as the camera uses.
        let view = glam::camera::rh::view::look_at_mat4(centre + dir * radius * 2.0, centre, up);
        let proj = glam::camera::rh::proj::directx::orthographic(-radius, radius, -radius, radius, 0.0, radius * 4.0);
        proj * view
    }

    pub fn triangle_count(&self) -> u32 {
        self.index_count / 3
    }

    /// Reads the last rendered frame back as RGBA8 bytes.
    ///
    /// Row-by-row because a GPU copy pads each row to 256 bytes, so the
    /// buffer is wider than the image unless the width happens to align.
    ///
    /// The channel order is not assumed: this target uses egui's own surface
    /// format, which is BGRA on macOS, and reading those bytes as RGBA swaps
    /// red and blue -- invisible on screen, wrong in every saved image.
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
            label: Some("readback"),
            size: (padded * height) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        let mut encoder = device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            target.color_texture.as_image_copy(),
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
        // wgpu 30's PollType::Wait carries optional fields; this helper
        // builds the no-timeout, latest-submission form.
        device.poll(wgpu::PollType::wait_indefinitely()).ok()?;

        let mapped = slice.get_mapped_range().ok()?;
        let mut out = Vec::with_capacity((unpadded * height) as usize);
        for row in 0..height {
            let start = (row * padded) as usize;
            out.extend_from_slice(&mapped[start..start + unpadded as usize]);
        }
        drop(mapped);
        buffer.unmap();

        if matches!(
            self.color_format,
            wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb
        ) {
            for pixel in out.as_chunks_mut::<4>().0 {
                pixel.swap(0, 2);
            }
        }

        Some((width, height, out))
    }
}
