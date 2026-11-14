// Voxel operations as compute kernels.
//
// The volume is addressed directly in 3D, with edits as compute kernels:
// no packing of the volume into a 2D texture and no edits as fragment
// shaders, which an OpenGL 2.x design would need.
//
// Each kernel plays the role of an `xs_begin` shader in the editor: a
// signed-distance style function evaluated once per voxel, in parallel.

struct Params {
    size: vec3<u32>,
    // Palette index to write where the shape is solid.
    color: u32,
    // Which shape to evaluate.
    mode: u32,
    // Shape parameter, meaning depends on mode.
    radius: f32,
    thickness: f32,
    // Replace the whole volume (1) or only add to it (0).
    replace: u32,
};

@group(0) @binding(0) var<uniform> params: Params;

// One palette index per element. Four voxels could be packed per u32, but a
// plain index keeps the kernel readable; the volumes here are far from any
// memory limit.
@group(0) @binding(1) var<storage, read_write> voxels: array<u32>;

fn sphere_sdf(p: vec3<f32>, r: f32) -> f32 {
    return length(p) - r;
}

fn torus_sdf(p: vec3<f32>, major: f32, minor: f32) -> f32 {
    let q = vec2<f32>(length(p.xy) - major, p.z);
    return length(q) - minor;
}

fn box_sdf(p: vec3<f32>, b: vec3<f32>, rounding: f32) -> f32 {
    let q = abs(p) - b + rounding;
    return length(max(q, vec3<f32>(0.0))) + min(max(q.x, max(q.y, q.z)), 0.0) - rounding;
}

// 4x4x4 = 64 invocations per workgroup, a good fit for a 3D domain.
@compute @workgroup_size(4, 4, 4)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    // The dispatch is rounded up to whole workgroups, so the tail must be
    // discarded or it writes past the end of the buffer.
    if (gid.x >= params.size.x || gid.y >= params.size.y || gid.z >= params.size.z) {
        return;
    }

    let index = gid.x
        + gid.y * params.size.x
        + gid.z * params.size.x * params.size.y;

    // Sample at the voxel centre, matching the editor's convention that a
    // voxel occupies the unit cube around its integer coordinate.
    let size_f = vec3<f32>(params.size);
    let p = vec3<f32>(gid) + vec3<f32>(0.5) - size_f * 0.5;

    var distance: f32 = 1.0;
    switch params.mode {
        case 0u: {
            distance = sphere_sdf(p, params.radius);
        }
        case 1u: {
            distance = torus_sdf(p, params.radius, params.thickness);
        }
        case 2u: {
            distance = box_sdf(p, vec3<f32>(params.radius), params.thickness);
        }
        case 3u: {
            // A hollow shell: the shape minus a smaller copy of itself.
            let outer = sphere_sdf(p, params.radius);
            let inner = sphere_sdf(p, params.radius - params.thickness);
            distance = max(outer, -inner);
        }
        default: {
            distance = 1.0;
        }
    }

    let solid = distance <= 0.0;
    if (solid) {
        voxels[index] = params.color;
    } else if (params.replace == 1u) {
        voxels[index] = 0u;
    }
}
