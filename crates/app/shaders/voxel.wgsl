// Voxel surface shading.
//
// The mesh carries a palette index per vertex rather than a colour, so the
// palette can be recoloured without rebuilding any geometry. the reference editor does
// the same, smuggling the index through the vertex W coordinate.

struct Uniforms {
    view_proj: mat4x4<f32>,
    light_dir: vec4<f32>,
    ambient: vec4<f32>,
    // x: draw voxel edges; y: shadows.
    options: vec4<f32>,
    light_view_proj: mat4x4<f32>,
};

@group(0) @binding(0) var<uniform> u: Uniforms;

// 256 RGBA entries, indexed by the voxel's palette index.
struct Palette {
    colors: array<vec4<f32>, 256>,
};
@group(0) @binding(1) var<uniform> palette: Palette;
@group(0) @binding(2) var shadow_map: texture_depth_2d;
@group(0) @binding(3) var shadow_sampler: sampler_comparison;

// How lit a point is, 0 to 1: four comparisons against the shadow map.
fn lit_fraction(world: vec3<f32>) -> f32 {
    let p = u.light_view_proj * vec4<f32>(world, 1.0);
    let ndc = p.xyz / p.w;
    let uv = ndc.xy * vec2<f32>(0.5, -0.5) + vec2<f32>(0.5);
    let texel = 1.0 / f32(textureDimensions(shadow_map).x);
    var sum = 0.0;
    for (var i = 0; i < 4; i++) {
        let offset = vec2<f32>(f32(i % 2) - 0.5, f32(i / 2) - 0.5) * texel;
        sum += textureSampleCompareLevel(shadow_map, shadow_sampler, uv + offset, ndc.z);
    }
    return sum * 0.25;
}

struct VertexIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) palette_index: f32,
};

struct VertexOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) color: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) world: vec3<f32>,
};

@vertex
fn vs_main(in: VertexIn) -> VertexOut {
    var out: VertexOut;
    out.clip_position = u.view_proj * vec4<f32>(in.position, 1.0);
    out.normal = in.normal;
    out.world = in.position;

    // Round before indexing: the index arrives as a float through the vertex
    // attribute, and truncation would pick the wrong colour at the boundary.
    // Indices from 256 up are selected voxels: the mesher adds 256 as a
    // flag. Tint them toward a light blue so the selection reads on any
    // colour while the colour itself stays recognisable.
    let raw = u32(in.palette_index + 0.5);
    let color = palette.colors[raw & 255u].rgb;
    if (raw >= 256u) {
        out.color = mix(color, vec3<f32>(0.35, 0.65, 1.0), 0.55);
    } else {
        out.color = color;
    }
    return out;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    // Wrap lighting, matching the editor's own viewport shader: back faces are
    // lit at 80% instead of going black, then the whole thing is scaled up.
    // It keeps unlit sides readable while editing, which a physically correct
    // term would not.
    let d = dot(u.light_dir.xyz, in.normal);
    // In shadow, a face keeps a third of the light that reaches it.
    var front = max(d, 0.0);
    if (u.options.y > 0.5 && d > 0.0) {
        front = d * mix(0.35, 1.0, lit_fraction(in.world));
    }
    let diffuse = (front + max(-d, 0.0) * 0.8) * 1.3;
    var lit = in.color * (u.ambient.rgb + vec3<f32>(diffuse));
    // Voxel edges: voxels meet on whole world coordinates (placements are
    // whole voxels, and quarter turns keep them whole), so a line is where
    // a coordinate across the face is near an integer -- a pixel wide
    // whatever the distance, from the derivative.
    if (u.options.x > 0.5) {
        let near = abs(fract(in.world + 0.5) - 0.5) / max(fwidth(in.world), vec3<f32>(1e-5));
        // Ignore the axis along the normal: it is constant on the face.
        let across = select(near, vec3<f32>(1e9), abs(in.normal) > vec3<f32>(0.5));
        let line = 1.0 - smoothstep(0.5, 1.5, min(across.x, min(across.y, across.z)));
        lit = mix(lit, lit * 0.55, line);
    }
    return vec4<f32>(lit, 1.0);
}
