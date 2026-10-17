// The shadow map: the voxels' depth as the light sees them. Only depth is
// written; the voxel shader compares against it.

struct Uniforms {
    view_proj: mat4x4<f32>,
    light_dir: vec4<f32>,
    ambient: vec4<f32>,
    options: vec4<f32>,
    light_view_proj: mat4x4<f32>,
};

@group(0) @binding(0) var<uniform> u: Uniforms;

@vertex
fn vs_main(@location(0) position: vec3<f32>) -> @builtin(position) vec4<f32> {
    return u.light_view_proj * vec4<f32>(position, 1.0);
}
