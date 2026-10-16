//! Voxel scripts: user kernels loaded from files, run over a model on the
//! GPU.
//!
//! This is the reference editor's `xs` command. A script defines one function,
//! `map`, which is evaluated once per voxel -- at the voxel's centre, so
//! `v` is `(1.5, 2.5, 4.5)` and so on -- and returns the palette index to
//! put there (0 for empty). A header names the script's arguments:
//!
//! ```text
//! // xs_begin
//! // arg : { var = 'm_teeth' name = 'Teeth' value = '6' range = '3 64' step = '1' precision = '0' }
//! // xs_end
//! ```
//!
//! Two languages are accepted:
//!
//! - **GLSL**, `.txt`: the reference editor's own scripts, unchanged. The original
//!   evaluates them in a fragment shader over a 2D-packed volume; here naga's
//!   GLSL front end compiles them into a compute kernel, and the result is
//!   handed to wgpu as WGSL.
//! - **WGSL**, `.wgsl`: `fn map(v: vec3<f32>) -> f32`, with the same names.
//!
//! Built-ins, in both (taken from the declarations in the reference editor's binary):
//! `i_volume_size`, `i_color_index`, `i_num_color_sels`, `i_mirror`,
//! `i_axis`, `i_args[16]`, `i_frame`, `i_num_frames`, `i_rand`, `i_iter`,
//! and the functions `voxel(v)`, `color_sel(k)` and `palette(index)`. Each
//! argument named in the header becomes a float variable of that name.
//!
//! `voxel` reads the model as it was before the pass, never cells the pass
//! has already written, so a script can grow, erode or shift a model. With
//! `-n count` the kernel runs that many times, each pass reading the last,
//! with `i_iter` counting from 0.

use std::path::{Path, PathBuf};
use voxel_core::Volume;
use wgpu::util::DeviceExt;

/// One argument from a script's header.
#[derive(Debug, Clone, PartialEq)]
pub struct Arg {
    /// The variable the script reads, such as `m_teeth`.
    pub var: String,
    /// The label for it.
    pub name: String,
    pub value: f32,
    pub range: (f32, f32),
    pub step: f32,
    pub precision: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Glsl,
    Wgsl,
}

#[derive(Debug, Clone)]
pub struct Script {
    /// The file name without its extension: what `xs <name>` takes.
    pub name: String,
    pub path: PathBuf,
    pub lang: Lang,
    pub source: String,
    pub args: Vec<Arg>,
}

impl Script {
    pub fn load(path: &Path) -> Result<Self, String> {
        let lang = match path.extension().and_then(|e| e.to_str()) {
            Some("wgsl") => Lang::Wgsl,
            Some("txt" | "glsl") => Lang::Glsl,
            _ => return Err(format!("{}: a script is a .wgsl or .txt file", path.display())),
        };
        let source = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let name = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let args = parse_header(&source)?;
        Ok(Self {
            name,
            path: path.to_owned(),
            lang,
            source,
            args,
        })
    }
}

/// Most arguments a script can have: `i_args` holds 16.
pub const MAX_ARGS: usize = 16;

/// The arguments between `xs_begin` and `xs_end`.
pub fn parse_header(source: &str) -> Result<Vec<Arg>, String> {
    let mut args = Vec::new();
    let mut inside = false;
    for line in source.lines() {
        let Some(comment) = line.trim().strip_prefix("//") else { continue };
        let comment = comment.trim();
        if comment.starts_with("xs_begin") {
            inside = true;
        } else if comment.starts_with("xs_end") {
            break;
        } else if inside && let Some(rest) = comment.strip_prefix("arg") {
            let body = rest.trim_start().trim_start_matches(':').trim();
            args.push(parse_arg(body)?);
        }
    }
    if args.len() > MAX_ARGS {
        return Err(format!("{} arguments; at most {MAX_ARGS}", args.len()));
    }
    Ok(args)
}

/// `{ var = 'm_r' name = 'Radius' value = '5' range = '0 64' step = '1' precision = '0' }`
fn parse_arg(body: &str) -> Result<Arg, String> {
    let field = |key: &str| -> Option<String> {
        // `key = 'value'`, with any spacing; keys are whole words.
        let mut rest = body;
        while let Some(at) = rest.find(key) {
            let before = rest[..at].chars().next_back();
            let after = &rest[at + key.len()..];
            rest = after;
            if before.is_some_and(|c| c.is_alphanumeric() || c == '_') {
                continue;
            }
            let Some(after) = after.trim_start().strip_prefix('=') else { continue };
            let after = after.trim_start().strip_prefix('\'')?;
            return after.find('\'').map(|end| after[..end].to_owned());
        }
        None
    };
    let var = field("var").ok_or_else(|| format!("argument without a var: {body}"))?;
    if var.is_empty() || !var.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') || var.starts_with(|c: char| c.is_ascii_digit()) {
        return Err(format!("argument variable '{var}' is not a name"));
    }
    let number = |key: &str, default: f32| field(key).and_then(|s| s.trim().parse::<f32>().ok()).unwrap_or(default);
    let range = field("range")
        .map(|r| {
            let mut it = r.split_whitespace().filter_map(|n| n.parse::<f32>().ok());
            (it.next().unwrap_or(0.0), it.next().unwrap_or(1.0))
        })
        .unwrap_or((0.0, 1.0));
    Ok(Arg {
        name: field("name").unwrap_or_else(|| var.clone()),
        value: number("value", range.0),
        range,
        step: number("step", 1.0),
        precision: field("precision").and_then(|s| s.trim().parse().ok()).unwrap_or(0),
        var,
    })
}

/// Where scripts are looked for, in order. Like environment maps, the
/// search ends in any local the reference editor install, whose `shader/` folder
/// holds its samples.
pub fn dirs(assets: &Path, reference_editor: &[PathBuf]) -> Vec<PathBuf> {
    let mut out = vec![assets.join("scripts")];
    if let Some(user) = crate::recent::settings_dir() {
        out.push(user.join("scripts"));
    }
    out.extend(reference_editor.iter().map(|d| d.join("shader")));
    out
}

/// Every script in `dirs`, by name; an earlier folder wins a name clash.
pub fn list(dirs: &[PathBuf]) -> Vec<(String, PathBuf)> {
    let mut out: Vec<(String, PathBuf)> = Vec::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else { continue };
        let mut found: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| matches!(p.extension().and_then(|e| e.to_str()), Some("wgsl" | "txt")))
            .collect();
        found.sort();
        for path in found {
            let Some(name) = path.file_stem().map(|s| s.to_string_lossy().into_owned()) else { continue };
            if !out.iter().any(|(n, _)| *n == name) {
                out.push((name, path));
            }
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// A script by name, or a path to one.
pub fn find(name: &str, dirs: &[PathBuf]) -> Option<PathBuf> {
    let as_path = Path::new(name);
    if as_path.extension().is_some() && as_path.is_file() {
        return Some(as_path.to_owned());
    }
    list(dirs).into_iter().find(|(n, _)| n == name).map(|(_, p)| p)
}

/// The uniforms both languages see. Must match `XsParams` in the preludes
/// below byte for byte: arrays in a uniform buffer have a 16-byte stride, so
/// the 16 arguments, 256 colour selections and the palette are packed into
/// `vec4`s.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Params {
    pub volume_size: [f32; 3],
    pub color_index: f32,
    pub mirror: [f32; 3],
    pub num_color_sels: i32,
    pub axis: [f32; 3],
    pub frame: f32,
    pub rand: [f32; 4],
    pub num_frames: f32,
    pub iter: f32,
    pub _pad: [f32; 2],
    pub args: [f32; 16],
    pub color_sels: [f32; 256],
    /// RGBA in 0..1.
    pub palette: [[f32; 4]; 256],
}

const WGSL_PRELUDE: &str = r#"
// ---- xs prelude ----
struct XsParams {
    volume_size: vec3<f32>,
    color_index: f32,
    mirror: vec3<f32>,
    num_color_sels: i32,
    axis: vec3<f32>,
    frame: f32,
    rand: vec4<f32>,
    num_frames: f32,
    iter: f32,
    pad: vec2<f32>,
    args: array<vec4<f32>, 4>,
    color_sels: array<vec4<f32>, 64>,
    palette: array<vec4<f32>, 256>,
};

@group(0) @binding(0) var<uniform> xs_params: XsParams;
@group(0) @binding(1) var<storage, read> xs_src: array<u32>;
@group(0) @binding(2) var<storage, read_write> xs_dst: array<u32>;

var<private> i_volume_size: vec3<f32>;
var<private> i_color_index: f32;
var<private> i_num_color_sels: i32;
var<private> i_mirror: vec3<f32>;
var<private> i_axis: vec3<f32>;
var<private> i_args: array<f32, 16>;
var<private> i_frame: f32;
var<private> i_num_frames: f32;
var<private> i_rand: vec4<f32>;
var<private> i_iter: f32;

fn voxel(v: vec3<f32>) -> f32 {
    let c = floor(v);
    if (any(c < vec3<f32>(0.0)) || any(c >= i_volume_size)) {
        return 0.0;
    }
    let size = vec3<u32>(i_volume_size);
    let p = vec3<u32>(c);
    return f32(xs_src[p.x + p.y * size.x + p.z * size.x * size.y]);
}

fn color_sel(k: f32) -> f32 {
    let i = clamp(i32(k), 0, max(i_num_color_sels - 1, 0));
    return xs_params.color_sels[i / 4][i % 4];
}

fn palette(index: f32) -> vec4<f32> {
    return xs_params.palette[u32(clamp(floor(index), 0.0, 255.0))];
}

@compute @workgroup_size(4, 4, 4)
fn xs_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let size = vec3<u32>(xs_params.volume_size);
    if (gid.x >= size.x || gid.y >= size.y || gid.z >= size.z) {
        return;
    }
    i_volume_size = xs_params.volume_size;
    i_color_index = xs_params.color_index;
    i_num_color_sels = xs_params.num_color_sels;
    i_mirror = xs_params.mirror;
    i_axis = xs_params.axis;
    for (var k = 0; k < 16; k++) {
        i_args[k] = xs_params.args[k / 4][k % 4];
    }
    i_frame = xs_params.frame;
    i_num_frames = xs_params.num_frames;
    i_rand = xs_params.rand;
    i_iter = xs_params.iter;
    xs_bind_args();
    let index = map(vec3<f32>(gid) + vec3<f32>(0.5));
    xs_dst[gid.x + gid.y * size.x + gid.z * size.x * size.y] = u32(clamp(floor(index + 0.5), 0.0, 255.0));
}
"#;

const GLSL_PRELUDE: &str = r#"#version 450
layout(local_size_x = 4, local_size_y = 4, local_size_z = 4) in;
layout(std140, set = 0, binding = 0) uniform XsParams {
    vec3 i_volume_size;
    float i_color_index;
    vec3 i_mirror;
    int i_num_color_sels;
    vec3 i_axis;
    float i_frame;
    vec4 i_rand;
    float i_num_frames;
    float i_iter;
    vec2 xs_pad;
    vec4 xs_args[4];
    vec4 xs_color_sels[64];
    vec4 xs_palette[256];
};
layout(std430, set = 0, binding = 1) readonly buffer XsSrc { uint xs_src[]; };
layout(std430, set = 0, binding = 2) buffer XsDst { uint xs_dst[]; };
#define iVolumeSize i_volume_size
#define iColorIndex i_color_index
#define iMirror i_mirror
#define iAxis i_axis
#define iArgs i_args
#define iFrame i_frame
#define iNumFrames i_num_frames
#define iRand i_rand
#define iIter i_iter
float i_args[16];
float voxel(vec3 v) {
    vec3 c = floor(v);
    if (any(lessThan(c, vec3(0.0))) || any(greaterThanEqual(c, i_volume_size))) {
        return 0.0;
    }
    uvec3 size = uvec3(i_volume_size);
    uvec3 p = uvec3(c);
    return float(xs_src[p.x + p.y * size.x + p.z * size.x * size.y]);
}
float color_sel(float k) {
    int i = clamp(int(k), 0, max(i_num_color_sels - 1, 0));
    return xs_color_sels[i / 4][i % 4];
}
vec4 palette(float index) {
    return xs_palette[int(clamp(floor(index), 0.0, 255.0))];
}
"#;

const GLSL_MAIN: &str = r#"
void main() {
    uvec3 gid = gl_GlobalInvocationID;
    uvec3 size = uvec3(i_volume_size);
    if (gid.x >= size.x || gid.y >= size.y || gid.z >= size.z) {
        return;
    }
    for (int k = 0; k < 16; k++) {
        i_args[k] = xs_args[k / 4][k % 4];
    }
    xs_bind_args();
    float index = map(vec3(gid) + vec3(0.5));
    xs_dst[gid.x + gid.y * size.x + gid.z * size.x * size.y] = uint(clamp(floor(index + 0.5), 0.0, 255.0));
}
"#;

/// The script as one WGSL module, ready for wgpu, or the compiler's
/// complaint with a line number in the script's own file.
pub fn compile(script: &Script) -> Result<String, String> {
    let (module, full, offset) = match script.lang {
        Lang::Wgsl => {
            // WGSL declarations may come in any order, so the script goes
            // first and its line numbers stay its own.
            let vars: String = script.args.iter().map(|a| format!("var<private> {}: f32;\n", a.var)).collect();
            let binds: String = script
                .args
                .iter()
                .enumerate()
                .map(|(k, a)| format!("    {} = i_args[{k}];\n", a.var))
                .collect();
            let full = format!(
                "{}\n{WGSL_PRELUDE}\n{vars}fn xs_bind_args() {{\n{binds}}}\n",
                script.source
            );
            let module = naga::front::wgsl::parse_str(&full).map_err(|e| {
                let line = e.location(&full).map(|l| format!("line {}: ", l.line_number)).unwrap_or_default();
                format!("{}: {line}{}", script.name, e.message())
            })?;
            (module, full, 0)
        }
        Lang::Glsl => {
            let vars: String = script.args.iter().map(|a| format!("float {};\n", a.var)).collect();
            let binds: String = script
                .args
                .iter()
                .enumerate()
                .map(|(k, a)| format!("    {} = i_args[{k}];\n", a.var))
                .collect();
            let head = format!("{GLSL_PRELUDE}{vars}");
            let offset = head.lines().count();
            // The binding function is declared before the script (it only
            // touches globals) so `main` after it can call it.
            let full = format!(
                "{head}{}\nvoid xs_bind_args() {{\n{binds}}}\n{GLSL_MAIN}",
                script.source
            );
            let options = naga::front::glsl::Options::from(naga::ShaderStage::Compute);
            let module = naga::front::glsl::Frontend::default().parse(&options, &full).map_err(|errors| {
                let first = errors.errors.first();
                let line = first
                    .map(|e| e.meta.location(&full).line_number as usize)
                    .filter(|&l| l > offset)
                    .map(|l| format!("line {}: ", l - offset))
                    .unwrap_or_default();
                let message = first.map(|e| e.kind.to_string()).unwrap_or_default();
                format!("{}: {line}{message}", script.name)
            })?;
            (module, full, offset)
        }
    };

    let info = naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::default())
        .validate(&module)
        .map_err(|e| {
            let line = e
                .location(&full)
                .map(|l| l.line_number as usize)
                .filter(|&l| l > offset)
                .map(|l| format!("line {}: ", l - offset))
                .unwrap_or_default();
            format!("{}: {line}{}", script.name, error_chain(&e.into_inner()))
        })?;

    if script.lang == Lang::Wgsl {
        return Ok(full);
    }
    naga::back::wgsl::write_string(&module, &info, naga::back::wgsl::WriterFlags::empty())
        .map_err(|e| format!("{}: {e}", script.name))
}

/// An error and its causes, on one line.
fn error_chain(e: &dyn std::error::Error) -> String {
    let mut out = e.to_string();
    let mut source = e.source();
    while let Some(s) = source {
        out.push_str(": ");
        out.push_str(&s.to_string());
        source = s.source();
    }
    out
}

/// The entry point of every compiled script.
fn entry_point(lang: Lang) -> &'static str {
    match lang {
        Lang::Wgsl => "xs_main",
        // naga keeps the GLSL name.
        Lang::Glsl => "main",
    }
}

/// Runs scripts on the GPU, keeping the last compiled pipeline so repeated
/// runs of one script (a slider being dragged) skip the compiler.
pub struct XsRunner {
    layout: wgpu::BindGroupLayout,
    pipeline_layout: wgpu::PipelineLayout,
    cached: Option<(String, wgpu::ComputePipeline)>,
}

impl XsRunner {
    pub fn new(device: &wgpu::Device) -> Self {
        let entry = |binding, ty| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("xs layout"),
            entries: &[
                entry(0, wgpu::BufferBindingType::Uniform),
                entry(1, wgpu::BufferBindingType::Storage { read_only: true }),
                entry(2, wgpu::BufferBindingType::Storage { read_only: false }),
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("xs pipeline layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        Self {
            layout,
            pipeline_layout,
            cached: None,
        }
    }

    fn pipeline(&mut self, device: &wgpu::Device, script: &Script) -> Result<&wgpu::ComputePipeline, String> {
        let fresh = !self.cached.as_ref().is_some_and(|(source, _)| *source == script.source);
        if fresh {
            let wgsl = compile(script)?;
            let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("xs script"),
                source: wgpu::ShaderSource::Wgsl(wgsl.into()),
            });
            let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("xs pipeline"),
                layout: Some(&self.pipeline_layout),
                module: &module,
                entry_point: Some(entry_point(script.lang)),
                compilation_options: Default::default(),
                cache: None,
            });
            self.cached = Some((script.source.clone(), pipeline));
        }
        Ok(&self.cached.as_ref().expect("just cached").1)
    }

    /// Runs `script` over `volume` `passes` times and returns the new
    /// palette indices, x fastest, then y, then z.
    pub fn run(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        volume: &Volume,
        script: &Script,
        params: &Params,
        passes: u32,
    ) -> Result<Vec<u8>, String> {
        let [sx, sy, sz] = volume.size();
        let count = (sx as usize) * (sy as usize) * (sz as usize);
        if count == 0 {
            return Err("the model is empty".to_owned());
        }
        self.pipeline(device, script)?;
        let pipeline = &self.cached.as_ref().expect("compiled").1;

        let mut data = Vec::with_capacity(count);
        for z in 0..sz {
            for y in 0..sy {
                for x in 0..sx {
                    data.push(volume.get(x, y, z) as u32);
                }
            }
        }
        let bytes = (count * 4) as u64;
        let usage = wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST;
        // Two buffers, swapped each pass: a pass reads one and writes the
        // other, so `voxel` never sees this pass's own writes.
        let buffers = [
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("xs a"),
                contents: bytemuck::cast_slice(&data),
                usage,
            }),
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("xs b"),
                size: bytes,
                usage,
                mapped_at_creation: false,
            }),
        ];
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("xs readback"),
            size: bytes,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        let passes = passes.max(1);
        let mut encoder = device.create_command_encoder(&Default::default());
        let mut uniforms = Vec::new();
        for pass_index in 0..passes {
            let mut p = *params;
            p.iter = pass_index as f32;
            let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("xs params"),
                contents: bytemuck::bytes_of(&p),
                usage: wgpu::BufferUsages::UNIFORM,
            });
            let (src, dst) = (&buffers[(pass_index % 2) as usize], &buffers[((pass_index + 1) % 2) as usize]);
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("xs bind group"),
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: uniform.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: src.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: dst.as_entire_binding(),
                    },
                ],
            });
            {
                let mut pass = encoder.begin_compute_pass(&Default::default());
                pass.set_pipeline(pipeline);
                pass.set_bind_group(0, &bind_group, &[]);
                pass.dispatch_workgroups(sx.div_ceil(4), sy.div_ceil(4), sz.div_ceil(4));
            }
            uniforms.push(uniform);
        }
        let last = &buffers[(passes % 2) as usize];
        encoder.copy_buffer_to_buffer(last, 0, &readback, 0, bytes);
        queue.submit(Some(encoder.finish()));

        let slice = readback.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|e| format!("the GPU did not finish: {e}"))?;
        let mapped = slice.get_mapped_range().map_err(|e| e.to_string())?;
        let words: &[u32] = bytemuck::cast_slice(&mapped);
        let out = words.iter().map(|&w| w as u8).collect();
        drop(mapped);
        readback.unmap();
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn script(lang: Lang, source: &str) -> Script {
        Script {
            name: "test".to_owned(),
            path: PathBuf::new(),
            lang,
            source: source.to_owned(),
            args: parse_header(source).unwrap(),
        }
    }

    #[test]
    fn header_arguments() {
        let args = parse_header(
            "// xs_begin\n// author : 'x'\n// arg : { var = 'm_teeth' name = 'Teeth' value = '6'   range = '3 64'    step = '1'\tprecision = '0' }\n\
             // arg : { var = 'm_inner' name = 'Inner' value = '0.5' range = '0.0 1.0' step = '0.05' precision = '2' }\n// xs_end\n// arg : { var = 'm_late' }\n",
        )
        .unwrap();
        assert_eq!(args.len(), 2);
        assert_eq!(args[0].var, "m_teeth");
        assert_eq!(args[0].name, "Teeth");
        assert_eq!(args[0].value, 6.0);
        assert_eq!(args[0].range, (3.0, 64.0));
        assert_eq!(args[1].step, 0.05);
        assert_eq!(args[1].precision, 2);
    }

    #[test]
    fn a_bad_variable_name_is_refused() {
        assert!(parse_header("// xs_begin\n// arg : { var = 'm x' }\n// xs_end").is_err());
    }

    #[test]
    fn params_match_the_wgsl_layout() {
        // 5 vec4 rows of scalars, 4 of args, 64 of selections, 256 colours.
        assert_eq!(std::mem::size_of::<Params>(), (5 + 4 + 64 + 256) * 16);
    }

    #[test]
    fn wgsl_script_compiles() {
        let s = script(
            Lang::Wgsl,
            "// xs_begin\n// arg : { var = 'm_r' value = '4' }\n// xs_end\nfn map(v: vec3<f32>) -> f32 {\n    return i_color_index * step(length(v - i_volume_size * 0.5), m_r) + voxel(v) * 0.0 + palette(1.0).x * 0.0 + color_sel(0.0) * 0.0;\n}\n",
        );
        compile(&s).unwrap();
    }

    #[test]
    fn glsl_script_compiles() {
        let s = script(
            Lang::Glsl,
            "// xs_begin\n// arg : { var = 'm_r' value = '4' }\n// xs_end\nfloat map(vec3 v) {\n    v.xy = vec2(v.y, v.x);\n    return i_color_index * step(length(v - iVolumeSize * 0.5), m_r) + mod(voxel(v), 2.0) * 0.0;\n}\n",
        );
        compile(&s).unwrap();
    }

    #[test]
    fn errors_name_the_script_line() {
        let wgsl = compile(&script(Lang::Wgsl, "fn map(v: vec3<f32>) -> f32 {\n    return nope;\n}\n")).unwrap_err();
        assert!(wgsl.contains("line 2"), "{wgsl}");
        let glsl = compile(&script(Lang::Glsl, "float map(vec3 v) {\n    return nope;\n}\n")).unwrap_err();
        assert!(glsl.contains("line 2"), "{glsl}");
    }

    /// The scripts that ship with the editor all compile.
    #[test]
    fn bundled_scripts_compile() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/scripts");
        let found = list(&[dir]);
        assert!(!found.is_empty());
        for (_, path) in found {
            let s = Script::load(&path).unwrap();
            compile(&s).unwrap_or_else(|e| panic!("{e}"));
        }
    }

    /// the reference editor's own samples compile unchanged, when an install is
    /// around to take them from.
    #[test]
    fn reference_editor_scripts_compile() {
        let Some(home) = std::env::var_os("HOME") else { return };
        let apps = Path::new(&home).join("Applications/the reference editor");
        let Ok(entries) = std::fs::read_dir(apps) else { return };
        for install in entries.flatten() {
            for (_, path) in list(&[install.path().join("shader")]) {
                let s = Script::load(&path).unwrap();
                // a recent version's gear.txt stores a vec3 in a float, which Apple's
                // old GLSL compiler let through; a recent version fixed the script.
                // Refusing it, with a message, is the right answer.
                if s.source.contains("float col = d * vec3(") {
                    assert!(compile(&s).is_err());
                    continue;
                }
                compile(&s).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            }
        }
    }
}
