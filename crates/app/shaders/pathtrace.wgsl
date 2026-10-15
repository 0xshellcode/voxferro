// Progressive path tracer over the voxel volume.
//
// One sample per pixel per frame, accumulated into a float buffer and
// averaged. The image refines while the camera holds still and resets when it
// moves — the same progressive behaviour the editor's renderer has.
//
// The volume is a 3D texture addressed directly, so marching a ray is a plain
// DDA walk. the reference editor reaches the same place through a 2D packing because
// OpenGL 2.x gave it no other option.

struct Params {
    // Camera basis, as three columns plus the eye position.
    right: vec4<f32>,
    up: vec4<f32>,
    forward: vec4<f32>,
    origin: vec4<f32>,

    volume_size: vec4<u32>,
    resolution: vec2<u32>,
    frame: u32,
    sample_count: u32,

    // Sun: direction, colour and intensity. Matches the _inf render object.
    sun_direction: vec4<f32>,
    sun_color: vec4<f32>,

    // Uniform sky, matching _uni.
    sky_color: vec4<f32>,

    tan_half_fov: f32,
    aspect: f32,
    exposure: f32,
    bounces: u32,

    sun_intensity: f32,
    sky_intensity: f32,
    sun_angular_radius: f32,
    reset: u32,

    // World -> voxel offset. The mesher centres a model on its transform, so
    // the rasterised view and the raw volume live half a model apart; without
    // this the traced image is offset from the one being framed.
    volume_offset: vec4<f32>,
};

@group(0) @binding(0) var<uniform> p: Params;
@group(0) @binding(1) var volume: texture_3d<u32>;
@group(0) @binding(2) var<storage, read> palette: array<vec4<f32>, 256>;
// Linear-light accumulator, one vec4 per pixel: rgb sum plus sample count.
@group(0) @binding(3) var<storage, read_write> accumulator: array<vec4<f32>>;
@group(0) @binding(4) var output: texture_storage_2d<rgba8unorm, write>;

// PCG hash. Cheap, and good enough that consecutive pixels and frames do not
// correlate -- a weak hash shows up immediately as structured noise.
fn pcg(state: ptr<function, u32>) -> u32 {
    let s = *state * 747796405u + 2891336453u;
    *state = s;
    let word = ((s >> ((s >> 28u) + 4u)) ^ s) * 277803737u;
    return (word >> 22u) ^ word;
}

fn random(state: ptr<function, u32>) -> f32 {
    return f32(pcg(state)) * (1.0 / 4294967296.0);
}

// Cosine-weighted hemisphere sample. Weighting by cosine means the lambertian
// throughput term cancels, so no division by pdf is needed below.
fn sample_hemisphere(normal: vec3<f32>, state: ptr<function, u32>) -> vec3<f32> {
    let r1 = random(state);
    let r2 = random(state);
    let radius = sqrt(r2);
    let theta = 6.283185307 * r1;

    // Build a basis around the normal without a branchy special case.
    var tangent: vec3<f32>;
    if (abs(normal.x) > 0.9) {
        tangent = normalize(cross(normal, vec3<f32>(0.0, 1.0, 0.0)));
    } else {
        tangent = normalize(cross(normal, vec3<f32>(1.0, 0.0, 0.0)));
    }
    let bitangent = cross(normal, tangent);

    return normalize(
        tangent * (radius * cos(theta))
        + bitangent * (radius * sin(theta))
        + normal * sqrt(max(0.0, 1.0 - r2))
    );
}

fn voxel_at(cell: vec3<i32>) -> u32 {
    if (any(cell < vec3<i32>(0)) || any(cell >= vec3<i32>(p.volume_size.xyz))) {
        return 0u;
    }
    return textureLoad(volume, cell, 0).r;
}

struct Hit {
    hit: bool,
    cell: vec3<i32>,
    normal: vec3<f32>,
    distance: f32,
};

// Amanatides-Woo DDA: visits every cell the ray touches, in order, so the
// first solid one is genuinely the nearest. Fixed-step sampling would tunnel
// through single-voxel walls.
fn trace(origin: vec3<f32>, direction: vec3<f32>, max_distance: f32) -> Hit {
    var result: Hit;
    result.hit = false;
    result.distance = max_distance;

    var cell = vec3<i32>(floor(origin));
    let step = vec3<i32>(sign(direction));

    let inv = 1.0 / max(abs(direction), vec3<f32>(1e-8));
    var t_max = vec3<f32>(
        select(origin.x - f32(cell.x), f32(cell.x) + 1.0 - origin.x, direction.x > 0.0) * inv.x,
        select(origin.y - f32(cell.y), f32(cell.y) + 1.0 - origin.y, direction.y > 0.0) * inv.y,
        select(origin.z - f32(cell.z), f32(cell.z) + 1.0 - origin.z, direction.z > 0.0) * inv.z
    );

    var travelled = 0.0;
    var normal = vec3<f32>(0.0);
    let limit = i32(p.volume_size.x + p.volume_size.y + p.volume_size.z) * 3 + 32;

    for (var i = 0; i < limit; i = i + 1) {
        if (travelled > max_distance) {
            break;
        }
        if (voxel_at(cell) != 0u) {
            result.hit = true;
            result.cell = cell;
            result.normal = normal;
            result.distance = travelled;
            return result;
        }

        // Advance along whichever axis reaches its next boundary first; that
        // axis is also the face the ray enters through.
        if (t_max.x < t_max.y && t_max.x < t_max.z) {
            travelled = t_max.x;
            t_max.x = t_max.x + inv.x;
            cell.x = cell.x + step.x;
            normal = vec3<f32>(-f32(step.x), 0.0, 0.0);
        } else if (t_max.y < t_max.z) {
            travelled = t_max.y;
            t_max.y = t_max.y + inv.y;
            cell.y = cell.y + step.y;
            normal = vec3<f32>(0.0, -f32(step.y), 0.0);
        } else {
            travelled = t_max.z;
            t_max.z = t_max.z + inv.z;
            cell.z = cell.z + step.z;
            normal = vec3<f32>(0.0, 0.0, -f32(step.z));
        }
    }
    return result;
}

fn sky(direction: vec3<f32>) -> vec3<f32> {
    // Uniform sky plus a sun disc, which is what _uni and _inf describe.
    var light = p.sky_color.rgb * p.sky_intensity;
    let alignment = dot(direction, normalize(p.sun_direction.xyz));
    if (alignment > cos(p.sun_angular_radius)) {
        light = light + p.sun_color.rgb * p.sun_intensity;
    }
    return light;
}

@compute @workgroup_size(8, 8, 1)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= p.resolution.x || gid.y >= p.resolution.y) {
        return;
    }
    let index = gid.y * p.resolution.x + gid.x;

    var rng = gid.x * 1973u + gid.y * 9277u + p.frame * 26699u;
    rng = rng | 1u;

    // Jitter inside the pixel: that is what turns accumulation into
    // antialiasing instead of just averaging the same sample repeatedly.
    let jitter = vec2<f32>(random(&rng), random(&rng));
    let pixel = (vec2<f32>(gid.xy) + jitter) / vec2<f32>(p.resolution);
    let ndc = vec2<f32>(pixel.x * 2.0 - 1.0, 1.0 - pixel.y * 2.0);

    var direction = normalize(
        p.forward.xyz
        + p.right.xyz * (ndc.x * p.tan_half_fov * p.aspect)
        + p.up.xyz * (ndc.y * p.tan_half_fov)
    );
    var origin = p.origin.xyz + p.volume_offset.xyz;

    var radiance = vec3<f32>(0.0);
    var throughput = vec3<f32>(1.0);
    let far = f32(p.volume_size.x + p.volume_size.y + p.volume_size.z) * 2.0;

    for (var bounce = 0u; bounce <= p.bounces; bounce = bounce + 1u) {
        let hit = trace(origin, direction, far);
        if (!hit.hit) {
            radiance = radiance + throughput * sky(direction);
            break;
        }

        let albedo = palette[voxel_at(hit.cell)].rgb;
        throughput = throughput * albedo;

        // Offset off the surface, or the next ray re-hits the voxel it left.
        let point = origin + direction * hit.distance + hit.normal * 0.001;

        // Direct sun, shadowed by one shadow ray.
        let to_sun = normalize(p.sun_direction.xyz);
        let facing = dot(hit.normal, to_sun);
        if (facing > 0.0) {
            let shadow = trace(point, to_sun, far);
            if (!shadow.hit) {
                radiance = radiance + throughput * p.sun_color.rgb * p.sun_intensity * facing;
            }
        }

        // Diffuse bounce. Cosine weighting cancels the lambertian term, so the
        // throughput needs no further factor here.
        direction = sample_hemisphere(hit.normal, &rng);
        origin = point;
    }

    // Accumulate in linear light. Resetting on camera motion keeps a moved
    // view from averaging against frames of the old one.
    var total = vec4<f32>(radiance, 1.0);
    if (p.reset == 0u) {
        total = total + accumulator[index];
    }
    accumulator[index] = total;

    var color = total.rgb / max(total.a, 1.0);
    color = color * p.exposure;

    // ACES filmic tonemap, the same curve the editor's _film object applies.
    let a = 2.51; let b = 0.03; let c = 2.43; let d = 0.59; let e = 0.14;
    color = clamp((color * (a * color + b)) / (color * (c * color + d) + e),
                  vec3<f32>(0.0), vec3<f32>(1.0));
    // The storage texture is rgba8unorm (no automatic sRGB), so encode here.
    color = pow(color, vec3<f32>(1.0 / 2.2));

    textureStore(output, vec2<i32>(gid.xy), vec4<f32>(color, 1.0));
}
