// Voxel surface shading.
//
// The mesh carries a palette index per vertex rather than a colour, so the
// palette can be recoloured without rebuilding any geometry. the reference editor does
// the same, smuggling the index through the vertex W coordinate.

struct Uniforms {
    view_proj: mat4x4<f32>,
    light_dir: vec4<f32>,
    ambient: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: Uniforms;

// 256 RGBA entries, indexed by the voxel's palette index.
struct Palette {
    colors: array<vec4<f32>, 256>,
};
@group(0) @binding(1) var<uniform> palette: Palette;

struct VertexIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) palette_index: f32,
};

struct VertexOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) color: vec3<f32>,
    @location(1) normal: vec3<f32>,
};

@vertex
fn vs_main(in: VertexIn) -> VertexOut {
    var out: VertexOut;
    out.clip_position = u.view_proj * vec4<f32>(in.position, 1.0);
    out.normal = in.normal;

    // Round before indexing: the index arrives as a float through the vertex
    // attribute, and truncation would pick the wrong colour at the boundary.
    let index = u32(in.palette_index + 0.5);
    out.color = palette.colors[index].rgb;
    return out;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    // Wrap lighting, matching the editor's own viewport shader: back faces are
    // lit at 80% instead of going black, then the whole thing is scaled up.
    // It keeps unlit sides readable while editing, which a physically correct
    // term would not.
    let d = dot(u.light_dir.xyz, in.normal);
    let diffuse = (max(d, 0.0) + max(-d, 0.0) * 0.8) * 1.3;
    let lit = in.color * (u.ambient.rgb + vec3<f32>(diffuse));
    return vec4<f32>(lit, 1.0);
}
