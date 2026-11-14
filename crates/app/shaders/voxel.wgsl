// Voxel surface shading.
//
// The mesh carries a palette index per vertex rather than a colour, so the
// palette can be recoloured without rebuilding any geometry.

struct Uniforms {
    view_proj: mat4x4<f32>,
    light_dir: vec4<f32>,
    ambient: vec4<f32>,
    // x: draw voxel edges; y: shadows.
    options: vec4<f32>,
    light_view_proj: mat4x4<f32>,
    // The editor options: x 1 for constant shading (the palette colour
    // alone), y how much of a faded object's colour stays (0.5), z the frame
    // lines' spacing in voxels (0: none).
    view: vec4<f32>,
    // The background colour, what faded objects fade toward.
    background: vec4<f32>,
    // The colours of the voxel edges, the frame lines and the selection.
    edge_color: vec4<f32>,
    grid_color: vec4<f32>,
    select_color: vec4<f32>,
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
    // The corner's ambient occlusion, 0 to 3 solid cells, interpolated
    // across the face.
    @location(3) occlusion: f32,
    // x 1 for a selected voxel, y 1 for a faded object.
    @location(4) @interpolate(flat) flags: vec2<f32>,
};

@vertex
fn vs_main(in: VertexIn) -> VertexOut {
    var out: VertexOut;
    out.clip_position = u.view_proj * vec4<f32>(in.position, 1.0);
    out.normal = in.normal;
    out.world = in.position;

    // Round before indexing: the index arrives as a float through the vertex
    // attribute, and truncation would pick the wrong colour at the boundary.
    // Above the index the mesher packs flags: 256 a selected voxel, 512 a
    // faded object, 1024 times the corner's occlusion.
    let raw = u32(in.palette_index + 0.5);
    let color = palette.colors[raw & 255u].rgb;
    let selected = (raw & 256u) != 0u;
    // A selected voxel is tinted toward the selection colour, so the
    // selection reads on any colour while the colour stays recognisable.
    out.color = select(color, mix(color, u.select_color.rgb, 0.45), selected);
    out.occlusion = f32((raw >> 10u) & 3u);
    out.flags = vec2<f32>(select(0.0, 1.0, selected), select(0.0, 1.0, (raw & 512u) != 0u));
    return out;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    // Wrap lighting: back faces are lit at 80% instead of going black, then
    // the whole thing is scaled up. It keeps unlit sides readable while
    // editing, which a physically correct term would not.
    let d = dot(u.light_dir.xyz, in.normal);
    // In shadow, a face keeps a third of the light that reaches it.
    var front = max(d, 0.0);
    if (u.options.y > 0.5 && d > 0.0) {
        front = d * mix(0.35, 1.0, lit_fraction(in.world));
    }
    let diffuse = (front + max(-d, 0.0) * 0.8) * 1.3;
    // Ambient occlusion darkens the ambient light alone, a fifth for each
    // solid cell around the corner.
    let ao = 1.0 - 0.2 * in.occlusion;
    var lit = in.color * (u.ambient.rgb * ao + vec3<f32>(diffuse));
    if (u.view.x > 0.5) {
        lit = in.color;
    }
    // Distance to the nearest voxel border across the face, in pixels.
    let near = abs(fract(in.world + 0.5) - 0.5) / max(fwidth(in.world), vec3<f32>(1e-5));
    // Ignore the axis along the normal: it is constant on the face.
    let along = abs(in.normal) > vec3<f32>(0.5);
    // Voxel edges: voxels meet on whole world coordinates (placements are
    // whole voxels, and quarter turns keep them whole), so a line is where
    // a coordinate across the face is near an integer -- a pixel wide
    // whatever the distance, from the derivative.
    if (u.options.x > 0.5 || in.flags.x > 0.5) {
        let across = select(near, vec3<f32>(1e9), along);
        let line = 1.0 - smoothstep(0.5, 1.5, min(across.x, min(across.y, across.z)));
        // Selected voxels show their borders in the selection colour.
        let ink = select(u.edge_color.rgb, u.select_color.rgb, in.flags.x > 0.5);
        lit = mix(lit, ink, line * 0.6);
    }
    // Frame lines every `view.z` voxels, on the surfaces as on the box.
    if (u.view.z > 0.5) {
        let k = u.view.z;
        let far = abs(in.world - floor(in.world / k + 0.5) * k) / max(fwidth(in.world), vec3<f32>(1e-5));
        let across = select(far, vec3<f32>(1e9), along);
        let line = 1.0 - smoothstep(0.8, 2.0, min(across.x, min(across.y, across.z)));
        lit = mix(lit, u.grid_color.rgb, line * 0.5);
    }
    // A faded object sinks toward the background.
    if (in.flags.y > 0.5) {
        lit = mix(u.background.rgb, lit, u.view.y);
    }
    return vec4<f32>(lit, 1.0);
}
