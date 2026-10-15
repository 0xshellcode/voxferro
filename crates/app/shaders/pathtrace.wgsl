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

    // x: number of entries in `emitters`.
    emitter_info: vec4<u32>,

    // x: plane height in voxel space (world z = 0), y: enabled, z: horizon
    // density (`_hor`).
    ground: vec4<f32>,
    ground_color: vec4<f32>,

    // Image-based lighting. x: enabled, y: radiance scale (the editor's
    // ibl_I = _i * 4pi / integral), z: rotation in turns, w: CDF total.
    ibl: vec4<f32>,
    // x, y: map width and height.
    ibl_size: vec4<u32>,
};

@group(0) @binding(0) var<uniform> p: Params;
@group(0) @binding(1) var volume: texture_3d<u32>;
@group(0) @binding(2) var<storage, read> palette: array<vec4<f32>, 256>;
// Linear-light accumulator, one vec4 per pixel: rgb sum plus sample count.
@group(0) @binding(3) var<storage, read_write> accumulator: array<vec4<f32>>;
@group(0) @binding(4) var output: texture_storage_2d<rgba8unorm, write>;

const KIND_DIFFUSE: u32 = 0u;
const KIND_METAL: u32 = 1u;
const KIND_GLASS: u32 = 2u;
const KIND_EMIT: u32 = 3u;

// One entry per palette index, resolved on the CPU from the MATL chunks so
// the shader never sees the two on-disk conventions.
struct Material {
    kind: u32,
    roughness: f32,
    ior: f32,
    // Metalness for metals, transparency for glass, unused otherwise.
    amount: f32,
    // rgb: emitted radiance, already scaled; zero for non-emitters.
    // w: absorption density for transparent materials.
    emission: vec4<f32>,
};

// Beer-Lambert absorption through `distance` voxels of material. The colour
// of glass comes from what it absorbs, so a density of zero is clear glass
// whatever its palette colour -- which is how the editor renders it.
fn absorb(albedo: vec3<f32>, density: f32, distance: f32) -> vec3<f32> {
    return exp(-(vec3<f32>(1.0) - albedo) * density * distance);
}
@group(0) @binding(5) var<storage, read> materials: array<Material, 256>;

// Emissive voxels with at least one face open to the air: xyz is the cell, w
// a bitmask of its exposed faces (bit 2*axis for the negative face, 2*axis+1
// for the positive one). Fully enclosed emitters are left out on the CPU --
// sampling them would only ever produce occluded shadow rays.
@group(0) @binding(6) var<storage, read> emitters: array<vec4<i32>>;

const PI: f32 = 3.14159265;

// Equirectangular environment map, row 0 straight up, and its running-sum
// CDF over max(r, g, b) -- the editor's own sampling table.
@group(0) @binding(7) var<storage, read> env_texels: array<vec4<f32>>;
@group(0) @binding(8) var<storage, read> env_cdf: array<f32>;

fn env_texel(x: i32, y: i32) -> vec3<f32> {
    let w = i32(p.ibl_size.x);
    let h = i32(p.ibl_size.y);
    let xx = ((x % w) + w) % w; // wraps around the horizon
    let yy = clamp(y, 0, h - 1); // clamps at the poles
    return env_texels[u32(yy * w + xx)].rgb;
}

// Radiance from the map in a world direction, bilinear for the visible
// background. Z is up; azimuth turns with the file's rotation.
fn env_radiance(direction: vec3<f32>) -> vec3<f32> {
    let w = f32(p.ibl_size.x);
    let h = f32(p.ibl_size.y);
    // Azimuth runs clockwise seen from above (-y), the editor's convention:
    // rendering the same map in both, side by side, put the bright sky on
    // the opposite side until this was flipped.
    var u = atan2(-direction.y, direction.x) / (2.0 * PI) + p.ibl.z;
    u = u - floor(u);
    let v = acos(clamp(direction.z, -1.0, 1.0)) / PI;
    let fx = u * w - 0.5;
    let fy = v * h - 0.5;
    let x0 = i32(floor(fx));
    let y0 = i32(floor(fy));
    let tx = fx - floor(fx);
    let ty = fy - floor(fy);
    let top = mix(env_texel(x0, y0), env_texel(x0 + 1, y0), tx);
    let bottom = mix(env_texel(x0, y0 + 1), env_texel(x0 + 1, y0 + 1), tx);
    return mix(top, bottom, ty) * p.ibl.y;
}

// What the sky is behind everything, without the sun disc: the map when
// image-based lighting is on, the uniform sky otherwise.
fn background(direction: vec3<f32>) -> vec3<f32> {
    if (p.ibl.x > 0.5) {
        return env_radiance(direction);
    }
    return p.sky_color.rgb * p.sky_intensity;
}

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
fn is_glass(cell: vec3<i32>) -> bool {
    let index = voxel_at(cell);
    return index != 0u && materials[index].kind == KIND_GLASS;
}

// mode 0: stop at the first solid voxel.
// mode 1: stop at the first voxel that is not glass, empty included. A ray
//         that has entered glass starts inside solid material, where mode 0
//         would stop immediately; this is how it finds the far side.
fn trace(origin: vec3<f32>, direction: vec3<f32>, max_distance: f32, mode: u32) -> Hit {
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
        var stop = voxel_at(cell) != 0u;
        if (mode == 1u) {
            stop = !is_glass(cell);
        }
        if (stop) {
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

// How much sunlight reaches `origin`: zero behind opaque voxels, tinted and
// attenuated behind glass. Refraction is ignored for shadow rays -- no
// caustics -- which is the usual approximation and keeps glass from casting
// a solid black shadow.
fn shadow(origin: vec3<f32>, direction: vec3<f32>, far: f32) -> vec3<f32> {
    var transmittance = vec3<f32>(1.0);
    var p = origin;
    for (var k = 0; k < 6; k = k + 1) {
        let h = trace(p, direction, far, 0u);
        if (!h.hit) {
            return transmittance;
        }
        let index = voxel_at(h.cell);
        let m = materials[index];
        if (m.kind != KIND_GLASS) {
            return vec3<f32>(0.0);
        }
        let inside = p + direction * h.distance - h.normal * 0.001;
        let out = trace(inside, direction, far, 1u);
        transmittance = transmittance * m.amount
            * absorb(palette[index].rgb, m.emission.w, out.distance);
        if (!out.hit) {
            return transmittance;
        }
        // out.normal faces back into the glass; step past the boundary.
        p = inside + direction * out.distance - out.normal * 0.001;
    }
    return transmittance;
}

// Next event estimation toward emissive voxels.
//
// Picks one emitter uniformly, then samples each of its faces that is both
// exposed and turned toward the shading point (at most three on a cube), with
// one shadow ray per face. Returns radiance arriving at a lambertian surface,
// still to be multiplied by its albedo (already folded into throughput).
fn sample_emitters(point: vec3<f32>, normal: vec3<f32>, state: ptr<function, u32>) -> vec3<f32> {
    let count = p.emitter_info.x;
    if (count == 0u) {
        return vec3<f32>(0.0);
    }
    let pick = min(u32(random(state) * f32(count)), count - 1u);
    let e = emitters[pick];
    let cell = e.xyz;
    let exposed = u32(e.w);
    let emitted = materials[voxel_at(cell)].emission.rgb;
    let to_light = vec3<f32>(cell) + vec3<f32>(0.5) - point;

    var total = vec3<f32>(0.0);
    for (var axis = 0; axis < 3; axis = axis + 1) {
        // The face that looks back at the point: if the light lies toward
        // +axis, that is its negative face.
        let positive = to_light[axis] < 0.0;
        let bit = u32(axis * 2) + select(0u, 1u, positive);
        if ((exposed & (1u << bit)) == 0u) {
            continue;
        }

        var on_face = vec3<f32>(cell) + vec3<f32>(random(state), random(state), random(state));
        on_face[axis] = f32(cell[axis]) + select(0.0, 1.0, positive);
        var face_normal = vec3<f32>(0.0);
        face_normal[axis] = select(-1.0, 1.0, positive);

        let offset = on_face - point;
        let dist2 = dot(offset, offset);
        let dist = sqrt(dist2);
        let dir = offset / dist;
        let cos_surface = dot(normal, dir);
        let cos_light = dot(face_normal, -dir);
        if (cos_surface <= 0.0 || cos_light <= 0.0) {
            continue;
        }

        // Visible only if the first thing the ray meets is this very cell.
        let h = trace(point, dir, dist + 1.0, 0u);
        if (!h.hit || any(h.cell != cell)) {
            continue;
        }

        // Unit-area face sampled uniformly: pdf_area = 1. The 1/pi is the
        // lambertian BRDF without its albedo.
        total = total + emitted * cos_surface * cos_light / (PI * dist2);
    }
    // Selecting one of `count` emitters uniformly: divide by its 1/count pdf.
    return total * f32(count);
}

// Distance along the ray to the ground plane, or -1 when it is off, behind,
// or the ray runs parallel to it. The plane is infinite, so it is solved
// analytically rather than walked by the DDA, which stops at the grid edge.
fn ground_distance(origin: vec3<f32>, direction: vec3<f32>) -> f32 {
    if (p.ground.y < 0.5 || direction.z >= -1e-6 || origin.z <= p.ground.x) {
        return -1.0;
    }
    return (p.ground.x - origin.z) / direction.z;
}

// How much of the ground at `distance` has dissolved into the sky.
//
// The editor stores `_hor` as `m_hor_density` and blends the ground into the
// horizon with it. The exact curve lives in its runtime-generated shader,
// which the binary does not carry as text, so this exponential fog is an
// approximation of the effect, not a transcription of it.
fn horizon_fade(distance: f32) -> f32 {
    return 1.0 - exp(-p.ground.z * distance * 0.003);
}

// Next event estimation toward the environment map.
//
// Picks a texel with probability proportional to its weight by binary search
// over the CDF, then a direction inside it uniform in cos θ -- uniform in
// solid angle within the texel, which makes the pdf exact instead of
// approximate near the poles.
fn sample_env(point: vec3<f32>, normal: vec3<f32>, state: ptr<function, u32>) -> vec3<f32> {
    if (p.ibl.x < 0.5 || p.ibl.w <= 0.0) {
        return vec3<f32>(0.0);
    }
    let w = p.ibl_size.x;
    let h = p.ibl_size.y;
    let count = w * h;

    let wanted = random(state) * p.ibl.w;
    var lo = 0u;
    var hi = count - 1u;
    loop {
        if (lo >= hi) {
            break;
        }
        let mid = (lo + hi) / 2u;
        if (env_cdf[mid] < wanted) {
            lo = mid + 1u;
        } else {
            hi = mid;
        }
    }
    let index = lo;
    let x = index % w;
    let y = index / w;

    var weight = env_cdf[index];
    if (index > 0u) {
        weight = weight - env_cdf[index - 1u];
    }
    if (weight <= 0.0) {
        return vec3<f32>(0.0);
    }

    let cos0 = cos(PI * f32(y) / f32(h));
    let cos1 = cos(PI * f32(y + 1u) / f32(h));
    let cos_t = mix(cos0, cos1, random(state));
    let sin_t = sqrt(max(0.0, 1.0 - cos_t * cos_t));
    let u = (f32(x) + random(state)) / f32(w);
    let phi = 2.0 * PI * (u - p.ibl.z); // undo the rotation env_radiance applies
    // Mirror of env_radiance's clockwise azimuth.
    let dir = vec3<f32>(sin_t * cos(phi), -sin_t * sin(phi), cos_t);

    let cos_surface = dot(normal, dir);
    if (cos_surface <= 0.0) {
        return vec3<f32>(0.0);
    }
    // Below the horizon the ground plane is in the way.
    if (p.ground.y > 0.5 && dir.z < 0.0) {
        return vec3<f32>(0.0);
    }

    let solid_angle = (2.0 * PI / f32(w)) * (cos0 - cos1);
    let pdf = (weight / p.ibl.w) / solid_angle;
    let visible = shadow(point, dir, 1e5);
    // The sampled texel's own value: consistent with the weight it was
    // chosen by, where a bilinear lookup would blur across neighbours.
    let radiance = env_texel(i32(x), i32(y)) * p.ibl.y;
    return radiance * visible * cos_surface / (PI * pdf);
}

fn random_in_sphere(state: ptr<function, u32>) -> vec3<f32> {
    // Rejection sampling: simple, unbiased, and converges in ~2 tries.
    for (var i = 0; i < 8; i = i + 1) {
        let v = vec3<f32>(random(state), random(state), random(state)) * 2.0 - 1.0;
        if (dot(v, v) <= 1.0) {
            return v;
        }
    }
    return vec3<f32>(0.0);
}

// Schlick's approximation of dielectric reflectance.
fn fresnel(cos_theta: f32, ior: f32) -> f32 {
    var r0 = (1.0 - ior) / (1.0 + ior);
    r0 = r0 * r0;
    return r0 + (1.0 - r0) * pow(1.0 - clamp(cos_theta, 0.0, 1.0), 5.0);
}

// `include_sun` is false after a diffuse bounce: the sun is already sampled
// directly there, and adding the disc again when a bounce happens to escape
// toward it would count that light twice.
fn sky(direction: vec3<f32>, include_sun: bool) -> vec3<f32> {
    // Uniform sky or environment map, plus a sun disc (_uni / _ibl, _inf).
    // After a diffuse bounce (`include_sun` false) an environment map is
    // left out too: it is importance-sampled at the surface, and adding it
    // again when a bounce escapes would count it twice.
    var light = vec3<f32>(0.0);
    if (p.ibl.x < 0.5 || include_sun) {
        light = background(direction);
    }
    let alignment = dot(direction, normalize(p.sun_direction.xyz));
    if (include_sun && alignment > cos(p.sun_angular_radius)) {
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
    // Camera rays and mirror/glass bounces see emitters and the sun disc
    // directly; after a diffuse bounce both are already counted by direct
    // sampling, so they are skipped to avoid adding the same light twice.
    var specular = true;
    let far = f32(p.volume_size.x + p.volume_size.y + p.volume_size.z) * 2.0;

    for (var bounce = 0u; bounce <= p.bounces; bounce = bounce + 1u) {
        let hit = trace(origin, direction, far, 0u);

        // The ground wins when it is nearer than any voxel, or when the ray
        // left the grid without hitting one.
        let t_ground = ground_distance(origin, direction);
        if (t_ground > 0.0 && (!hit.hit || t_ground < hit.distance)) {
            let point = origin + direction * t_ground;
            let up = vec3<f32>(0.0, 0.0, 1.0);
            let lifted = point + up * 0.001;

            // Distant ground dissolves into the sky *at the horizon*. Not
            // the background straight along the ray: below the horizon that
            // is the photo's ground in an HDR, or a short dark path into the
            // planet under the atmosphere, and the editor keeps its ground
            // grey in both.
            let fade = horizon_fade(t_ground);
            let horizon = normalize(vec3<f32>(direction.xy, 0.02 * length(direction.xy)));
            radiance = radiance + throughput * fade * background(horizon);
            throughput = throughput * (1.0 - fade) * p.ground_color.rgb;

            // A diffuse surface like any other: sun, emitters, a bounce.
            let to_sun = normalize(p.sun_direction.xyz);
            if (to_sun.z > 0.0) {
                let visible = shadow(lifted, to_sun, 1e5);
                radiance = radiance + throughput * visible * p.sun_color.rgb * p.sun_intensity * to_sun.z;
            }
            radiance = radiance + throughput * sample_emitters(lifted, up, &rng);
            radiance = radiance + throughput * sample_env(lifted, up, &rng);

            direction = sample_hemisphere(up, &rng);
            origin = lifted;
            specular = false;
            continue;
        }

        if (!hit.hit) {
            radiance = radiance + throughput * sky(direction, specular);
            break;
        }

        let index = voxel_at(hit.cell);
        let albedo = palette[index].rgb;
        let m = materials[index];
        let surface = origin + direction * hit.distance;
        // Offset off the surface, or the next ray re-hits the voxel it left.
        let outside = surface + hit.normal * 0.001;

        if (m.kind == KIND_EMIT && specular) {
            radiance = radiance + throughput * m.emission.rgb;
        }

        // Pick a lobe with probability equal to its weight. Throughput needs
        // no correction for that choice: each lobe is sampled exactly as
        // often as it contributes.
        let lobe = random(&rng);

        if (m.kind == KIND_METAL && lobe < m.amount) {
            // Mirror reflection, blurred by roughness. The CPU already
            // applied the editor's 0.15 scale, so this is used as-is.
            let fuzz = m.roughness;
            let reflected = normalize(
                reflect(direction, hit.normal) + random_in_sphere(&rng) * fuzz
            );
            if (dot(reflected, hit.normal) <= 0.0) {
                break; // scattered into the surface: absorbed
            }
            throughput = throughput * albedo;
            direction = reflected;
            origin = outside;
            specular = true;
            continue;
        }

        if (m.kind == KIND_GLASS && lobe < m.amount) {
            let cos_i = -dot(direction, hit.normal);
            if (random(&rng) < fresnel(cos_i, m.ior)) {
                // Specular reflection off the glass, untinted.
                direction = reflect(direction, hit.normal);
                origin = outside;
                specular = true;
                continue;
            }

            // Refract in, walk through the glass, refract out.
            var d = refract(direction, hit.normal, 1.0 / m.ior);
            var q = surface - hit.normal * 0.001;
            var escaped = false;
            var inside = 0.0;
            for (var k = 0; k < 8; k = k + 1) {
                let out = trace(q, d, far, 1u);
                inside = inside + out.distance;
                if (!out.hit) {
                    escaped = true; // left the grid while inside
                    break;
                }
                let exit_point = q + d * out.distance;
                // out.normal faces back into the glass, which is the side
                // the ray arrives from -- what refract() expects.
                let bent = refract(d, out.normal, m.ior);
                if (dot(bent, bent) < 1e-6) {
                    // Total internal reflection: bounce and keep going.
                    d = reflect(d, out.normal);
                    q = exit_point + out.normal * 0.001;
                    continue;
                }
                d = bent;
                q = exit_point - out.normal * 0.001;
                escaped = true;
                break;
            }
            if (!escaped) {
                break;
            }
            throughput = throughput * absorb(albedo, m.emission.w, inside);
            direction = normalize(d);
            origin = q;
            specular = true;
            continue;
        }

        // Diffuse (also the non-metal / non-glass share of mixed materials,
        // and the reflective part of emitters).
        throughput = throughput * albedo;

        // Direct sun through one shadow ray.
        let to_sun = normalize(p.sun_direction.xyz);
        let facing = dot(hit.normal, to_sun);
        if (facing > 0.0) {
            let visible = shadow(outside, to_sun, far);
            radiance = radiance + throughput * visible * p.sun_color.rgb * p.sun_intensity * facing;
        }

        // Direct light from emissive voxels.
        radiance = radiance + throughput * sample_emitters(outside, hit.normal, &rng);
        radiance = radiance + throughput * sample_env(outside, hit.normal, &rng);

        // Cosine-weighted bounce; the lambertian term cancels.
        direction = sample_hemisphere(hit.normal, &rng);
        origin = outside;
        specular = false;
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

    // Triangular dither of +-1 code before 8-bit quantisation. In bright
    // sky the ACES shoulder squeezes a wide radiance range into two or three
    // output codes, which shows up as contour lines; noise below one code
    // trades those steps for grain the eye averages away.
    let dither = (random(&rng) + random(&rng) - 1.0) / 255.0;
    color = color + vec3<f32>(dither);

    textureStore(output, vec2<i32>(gid.xy), vec4<f32>(color, 1.0));
}
