// Progressive path tracer over the voxel volume.
//
// One sample per pixel per frame, accumulated into a float buffer and
// averaged. The image refines while the camera holds still and resets when it
// moves.
//
// The volume is a 3D texture addressed directly, so marching a ray is a plain
// DDA walk.

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

    // x: number of entries in `emitters`; y: a diagnostic view in place of
    // the render (`VOXFERRO_TRACE_DEBUG`, see `debug_view`), 0 for none.
    emitter_info: vec4<u32>,

    // x: plane height in voxel space (world z = 0), y: enabled, z: horizon
    // density (`_hor`).
    ground: vec4<f32>,
    ground_color: vec4<f32>,

    // Image-based lighting. x: enabled, y: radiance scale
    // (`_i * 4pi / integral`), z: rotation in turns, w: CDF total.
    ibl: vec4<f32>,
    // x, y: map width and height.
    ibl_size: vec4<u32>,

    // x: orthographic half-height in world units; 0 means a lens. y: the
    // lens (`_lens._proj`): 0 perspective, 1 stereographic, 2 panorama;
    // z: its scale from the screen.
    projection: vec4<f32>,

    // Depth of field. x: lens radius over focus distance
    // (`_aperture * 0.03`; 0 is a pinhole), y: focus distance, z: number of
    // aperture blades (below 3, a round lens), w: blade rotation, radians.
    lens: vec4<f32>,

    // Fog (`_fog_uni`): x: extinction per voxel (`_d * 0.001`), y: anisotropy
    // g (`_g`); 0 extinction is no fog. z: scattering events allowed in media
    // and fog (`_bounce._scatter`), counted apart from surface bounces.
    fog: vec4<f32>,
    // The fog's colour (`_k`): what fraction of light a scattering keeps.
    fog_color: vec4<f32>,
    // What a camera ray that meets nothing shows (the background option):
    // x 0 the sky, 1 a flat colour, 2 nothing -- transparent; the
    // colour itself is put in by the post pass. Bounces see the sky always.
    look: vec4<f32>,
    // How a voxel is drawn: x 0 a cube, 1 a sphere, 2 a cylinder standing
    // on z, 3 a brick with a stud on top, 4 clay (a smooth surface through
    // the voxels), 5 marching cubes; y the sphere's and cylinder's radius,
    // 0.25 to 0.5.
    shape: vec4<f32>,
    // Lines on the surfaces (Display Grids and Edges): x the
    // grid's width in voxels (0 off), y its spacing, z 1 for the ground
    // only; w the edges' width (0 off). Colours linear.
    lines: vec4<f32>,
    grid_color: vec4<f32>,
    edge_color: vec4<f32>,
    // Render › Sample: x mirror and glass events a path may take
    // (`_bounce._specular`), y the most light one sample may carry
    // (`_bounce._energy`, 0 for no limit), z 1 to spread samples over the
    // pixel (anti-aliasing) or 0 for its centre; w 1 when a diffuse depth
    // of 0 ends paths in the uniform sky's light.
    sample: vec4<f32>,
    // The band of rows this dispatch traces: x its first row, y how many.
    // A sample is split into bands, each submitted on its own, so that no
    // single piece of GPU work runs long enough for Windows to reset the
    // GPU (TDR, 2 s by default).
    tile: vec4<u32>,
};

@group(0) @binding(0) var<uniform> p: Params;
@group(0) @binding(1) var volume: texture_3d<u32>;
@group(0) @binding(2) var<storage, read> palette: array<vec4<f32>, 256>;
// Linear-light accumulator, one vec4 per pixel: rgb sum plus sample count.
@group(0) @binding(3) var<storage, read_write> accumulator: array<vec4<f32>>;
// How many of a pixel's samples met something: summed, then divided by the
// sample count in the post pass, it is the pixel's opacity.
@group(0) @binding(9) var<storage, read_write> coverage: array<f32>;
// Marching cubes cases (`mc_table` in pathtrace.rs): two vec4 a case, the
// triangle count first, then each triangle's corners at six bits apiece.
@group(0) @binding(10) var<uniform> mc_table: array<vec4<u32>, 512>;
// Each cell's 3x3x3 neighbourhood as a mask (`neighbours.wgsl`): bit
// (x + 1) + 3 (y + 1) + 9 (z + 1) for the voxel at that offset. Only bound
// for marching cubes; a single texel otherwise.
@group(0) @binding(11) var neighbours: texture_3d<u32>;

// A cell's neighbourhood mask; 0 outside the grid, where a texture read
// would return anything at all. Walks run on past the grid's edge.
fn mc_around(cell: vec3<i32>) -> u32 {
    if (any(cell < vec3<i32>(0)) || any(cell >= vec3<i32>(p.volume_size.xyz))) {
        return 0u;
    }
    return textureLoad(neighbours, cell, 0).x;
}

const KIND_DIFFUSE: u32 = 0u;
const KIND_METAL: u32 = 1u;
const KIND_GLASS: u32 = 2u;
const KIND_EMIT: u32 = 3u;
// A participating medium (`_media`, the editor's cloud). `amount` holds how
// it interacts (0 absorb, 1 scatter, 2 emit, 3 subsurface), `roughness` its
// phase g, `emission.w` its density per voxel.
const KIND_MEDIA: u32 = 4u;
// Blend: metal, glass and diffuse at once, a share each
// (`amount` the metal, `extra.x` the glass), plus emission.
const KIND_BLEND: u32 = 5u;

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
    // x: Blend's glass share.
    extra: vec4<f32>,
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

// Whether a step from `cell` toward `toward` (a tangent of the face, zero
// when the point is not by that border) crosses an edge of the model: the
// face ends there (nothing beside the cell), or another face rises (a voxel
// beside the cell's open side).
fn edge_toward(cell: vec3<i32>, toward: vec3<i32>, out: vec3<i32>) -> bool {
    if (all(toward == vec3<i32>(0))) {
        return false;
    }
    return voxel_at(cell + toward) == 0u || voxel_at(cell + toward + out) != 0u;
}

// Whether a point `t` from its voxel's border toward `dir` is on an edge
// line: the surface turns `k` voxels away that way and (k - 1) + t is under
// the width. Up to a width of 1 that is a band min(width, 0.5) deep beside
// the edge on each face; wider, the band repeats in the next voxels in (5 at
// most).
fn edge_along(cell: vec3<i32>, dir: vec3<i32>, out: vec3<i32>, t: f32) -> bool {
    for (var k = 1; k <= 5; k = k + 1) {
        if (f32(k - 1) + t >= p.lines.w) {
            return false;
        }
        if (edge_toward(cell + dir * (k - 1), dir, out)) {
            return true;
        }
    }
    return false;
}

// The colour a surface point shows with the render's lines drawn over it.
// `point` is in voxel space; `cell` is the voxel hit, unused on the ground.
// A grid line is twice its width across,
// along world coordinates that are multiples of the spacing; edges follow
// `edge_along`.
fn lined(base: vec3<f32>, point: vec3<f32>, normal: vec3<f32>, cell: vec3<i32>, ground: bool) -> vec3<f32> {
    // The face's own axis, left out: lines run along the other two.
    let across = vec3<f32>(abs(normal) > vec3<f32>(0.5));
    var colour = base;
    if (p.lines.x > 0.0 && (ground || p.lines.z < 0.5)) {
        let q = (point - p.volume_offset.xyz) / p.lines.y;
        let d = abs(q - round(q)) * p.lines.y + across * 1e3;
        if (min(d.x, min(d.y, d.z)) < p.lines.x) {
            colour = p.grid_color.rgb;
        }
    }
    if (!ground && p.lines.w > 0.0) {
        let local = point - vec3<f32>(cell);
        // Per tangent axis, the border the point's half of the voxel faces
        // (-1 or +1, 0 on the face's own axis) and the distance to it.
        let side = select(vec3<i32>(1), vec3<i32>(-1), local < vec3<f32>(0.5)) * vec3<i32>(1.0 - across);
        let t = min(local, vec3<f32>(1.0) - local);
        let out = vec3<i32>(round(normal));
        // A corner where both borders are near and only the diagonal
        // neighbour is missing.
        let close = select(vec3<i32>(0), side, t < vec3<f32>(p.lines.w));
        if (edge_along(cell, side * vec3<i32>(1, 0, 0), out, t.x)
            || edge_along(cell, side * vec3<i32>(0, 1, 0), out, t.y)
            || edge_along(cell, side * vec3<i32>(0, 0, 1), out, t.z)
            || (dot(abs(close), vec3<i32>(1)) == 2 && edge_toward(cell, close, out))) {
            colour = p.edge_color.rgb;
        }
    }
    return colour;
}

struct Hit {
    hit: bool,
    cell: vec3<i32>,
    normal: vec3<f32>,
    distance: f32,
    // Where the ray meets the face, exactly on its plane. Rebuilding it as
    // origin + direction * distance is not good enough: the walk adds up
    // the distance one cell at a time, each sum rounded at the size of the
    // whole distance, and after a few dozen cells the point can sit more
    // than the 0.001 that rays step off a surface -- on the wrong side, so
    // the surface shadowed itself and rendered black.
    point: vec3<f32>,
};

// The hit point of a ray that reached `cell` through the face `normal`
// points back out of, with the face's own coordinate exact.
fn face_point(origin: vec3<f32>, direction: vec3<f32>, distance: f32, cell: vec3<i32>, normal: vec3<f32>) -> vec3<f32> {
    let plane = vec3<f32>(cell) + max(normal, vec3<f32>(0.0));
    return mix(origin + direction * distance, plane, abs(normal));
}

// Amanatides-Woo DDA: visits every cell the ray touches, in order, so the
// first solid one is genuinely the nearest. Fixed-step sampling would tunnel
// through single-voxel walls.
fn is_glass(cell: vec3<i32>) -> bool {
    let index = voxel_at(cell);
    return index != 0u && materials[index].kind == KIND_GLASS;
}

// --- Voxels drawn as other shapes ------------------------------------------
//
// Only in other builds of this shader, one per shape, with SHAPE set to the
// shape's number (as `p.shape.x`: 1 sphere, 2 cylinder, 3 brick, 4 clay, 5
// marching cubes). In the usual build (0) the shape code compiles away, and
// in each shape's build only that shape's code is left. Inlined wherever a
// ray is traced, all five together made a kernel that DirectX compiled
// slowly and ran several times slower than Vulkan, and that its software
// rasteriser (WARP) rendered black.
// Each test looks for the shape inside one cell, between the distances
// t_in (where the ray enters the cell) and t_out (where it leaves); a miss
// lets the walk go on to the next cell. Glass and media stay cubes.

override SHAPE: u32 = 0u;

fn shaped(index: u32) -> bool {
    let k = materials[index].kind;
    return p.shape.x > 0.5 && k != KIND_GLASS && k != KIND_MEDIA;
}

struct ShapeHit {
    hit: bool,
    t: f32,
    normal: vec3<f32>,
    point: vec3<f32>,
    // The voxel whose colour the point takes, for the shapes that reach
    // beyond their own cell (marching cubes).
    cell: vec3<i32>,
};

fn no_shape() -> ShapeHit {
    var h: ShapeHit;
    h.hit = false;
    return h;
}

fn sphere_in(origin: vec3<f32>, dir: vec3<f32>, cell: vec3<i32>, t_in: f32, t_out: f32) -> ShapeHit {
    let r = p.shape.y;
    let c = vec3<f32>(cell) + vec3<f32>(0.5);
    let oc = origin - c;
    let b = dot(oc, dir);
    let disc = b * b - (dot(oc, oc) - r * r);
    if (disc < 0.0) {
        return no_shape();
    }
    let t = -b - sqrt(disc);
    if (t < max(t_in - 1e-4, 0.0) || t > t_out) {
        return no_shape();
    }
    var h: ShapeHit;
    h.hit = true;
    h.t = t;
    h.normal = normalize(origin + dir * t - c);
    // Exactly on the surface, however far the ray came from.
    h.point = c + h.normal * r;
    return h;
}

// A cylinder standing on z, `r` round, from z0 to z1 in the cell's column.
// `cap_low` and `cap_high` say whether its ends are closed: a stack of
// cylinders is one column, capped only where it ends.
fn cylinder_in(origin: vec3<f32>, dir: vec3<f32>, centre: vec2<f32>, r: f32, z0: f32, z1: f32,
               cap_low: bool, cap_high: bool, t_in: f32, t_out: f32) -> ShapeHit {
    var best = no_shape();
    best.t = t_out;
    let o = origin.xy - centre;
    let a = dot(dir.xy, dir.xy);
    if (a > 1e-10) {
        let b = dot(o, dir.xy);
        let disc = b * b - a * (dot(o, o) - r * r);
        if (disc >= 0.0) {
            let t = (-b - sqrt(disc)) / a;
            let z = origin.z + dir.z * t;
            if (t >= max(t_in - 1e-4, 0.0) && t <= best.t && z >= z0 && z <= z1) {
                best.hit = true;
                best.t = t;
                let q = normalize(origin.xy + dir.xy * t - centre);
                best.normal = vec3<f32>(q, 0.0);
                best.point = vec3<f32>(centre + q * r, z);
            }
        }
    }
    if (abs(dir.z) > 1e-8) {
        for (var end = 0; end < 2; end = end + 1) {
            let high = end == 1;
            if ((high && !cap_high) || (!high && !cap_low)) {
                continue;
            }
            let z = select(z0, z1, high);
            let t = (z - origin.z) / dir.z;
            let q = origin.xy + dir.xy * t - centre;
            // Only from outside: a ray going down meets the top.
            let facing = select(dir.z > 0.0, dir.z < 0.0, high);
            if (facing && t >= max(t_in - 1e-4, 0.0) && t <= best.t && dot(q, q) <= r * r) {
                best.hit = true;
                best.t = t;
                best.normal = vec3<f32>(0.0, 0.0, select(-1.0, 1.0, high));
                best.point = vec3<f32>(origin.xy + dir.xy * t, z);
            }
        }
    }
    return best;
}

// Clay: the occupancy of the voxel centres, blended trilinearly; the
// surface is where it crosses one half. It stays inside the solid cells.
fn clay_field(q: vec3<f32>) -> f32 {
    let g = q - vec3<f32>(0.5);
    let base = vec3<i32>(floor(g));
    let f = g - floor(g);
    var sum = 0.0;
    for (var k = 0; k < 8; k = k + 1) {
        let corner = vec3<i32>(k & 1, (k >> 1u) & 1, (k >> 2u) & 1);
        let w = mix(1.0 - f, f, vec3<f32>(corner));
        if (voxel_at(base + corner) != 0u) {
            sum = sum + w.x * w.y * w.z;
        }
    }
    return sum;
}

fn clay_in(origin: vec3<f32>, dir: vec3<f32>, t_in: f32, t_out: f32, entry: vec3<f32>) -> ShapeHit {
    // One loop: eight steps across the cell until the field crosses one
    // half, then halving the last step. A single call to the field keeps
    // the shader small -- it is inlined wherever a ray is traced, and the
    // DirectX software rasteriser would not take a larger one.
    let a = max(t_in, 0.0);
    var lo = a;
    var hi = t_out;
    var t = a;
    var found = false;
    for (var i = 0; i < 18; i = i + 1) {
        if (found) {
            t = 0.5 * (lo + hi);
        } else {
            t = mix(a, t_out, f32(i) / 8.0);
            if (i > 8) {
                return no_shape();
            }
        }
        let inside = clay_field(origin + dir * t) >= 0.5;
        if (found) {
            if (inside) { hi = t; } else { lo = t; }
        } else if (inside) {
            if (i == 0) {
                lo = t;
                hi = t;
                break;
            }
            found = true;
            hi = t;
        } else {
            lo = t;
        }
    }
    var h: ShapeHit;
    h.hit = true;
    h.t = hi;
    h.point = origin + dir * hi;
    // The gradient, an axis at a time. No component is written by a
    // variable index: FXC cannot address one, and unrolls loops to try.
    var grad = vec3<f32>(0.0);
    for (var axis = 0; axis < 3; axis = axis + 1) {
        let unit = vec3<f32>(vec3<i32>(axis) == vec3<i32>(0, 1, 2));
        let e = unit * 0.05;
        grad = grad + unit * (clay_field(h.point + e) - clay_field(h.point - e));
    }
    h.normal = select(entry, -normalize(grad), dot(grad, grad) > 1e-12);
    if (dot(h.normal, dir) > 0.0) {
        h.normal = entry;
    }
    return h;
}

// Marching cubes, as the *mc* export: cells whose corners are voxel centres,
// each holding the polygons of its case through the midpoints of the edges
// between solid and empty corners. A voxel overlaps eight such cells, a
// half-cell of each; the nearest of their triangles within the voxel, met
// from outside, is the hit, coloured as the nearest solid corner.
fn mc_corner(packed: u32) -> vec3<f32> {
    return vec3<f32>(f32(packed & 3u), f32((packed >> 2u) & 3u), f32((packed >> 4u) & 3u)) * 0.5;
}

fn mc_in(origin: vec3<f32>, dir: vec3<f32>, cell: vec3<i32>, t_in: f32, t_out: f32) -> ShapeHit {
    // Which of the voxel and its 26 neighbours are solid.
    let around = mc_around(cell);
    var best = no_shape();
    var best_t = t_out;
    var best_base = vec3<i32>(0);
    var best_case = 0u;
    let near = max(t_in, 0.0);
    for (var d = 0; d < 8; d = d + 1) {
        // The marching cell's first corner, from the voxel: -1 or 0 a side.
        let base = vec3<i32>(d & 1, (d >> 1u) & 1, (d >> 2u) & 1) - vec3<i32>(1);
        var case_index = 0u;
        for (var i = 0; i < 8; i = i + 1) {
            let c = base + vec3<i32>(i & 1, (i >> 1u) & 1, (i >> 2u) & 1) + vec3<i32>(1);
            case_index = case_index | (((around >> u32(c.x + 3 * c.y + 9 * c.z)) & 1u) << u32(i));
        }
        let head = mc_table[case_index * 2u];
        let tail = mc_table[case_index * 2u + 1u];
        let count = head.x;
        let first = vec3<f32>(cell + base) + vec3<f32>(0.5);
        for (var j = 1u; j <= count; j = j + 1u) {
            // Triangle j is in head.yzw, then tail.xy; picked by masks,
            // not by a variable index into a vector.
            let packed = select(select(select(head.w, head.z, j == 2u), head.y, j == 1u), select(tail.y, tail.x, j == 4u), j > 3u);
            let a = first + mc_corner(packed);
            let b = first + mc_corner(packed >> 6u);
            let c = first + mc_corner(packed >> 12u);
            // Moller-Trumbore, the front side only: the polygons wind
            // counter-clockwise seen from outside.
            let e1 = b - a;
            let e2 = c - a;
            let n = cross(e1, e2);
            if (dot(n, dir) >= 0.0) {
                continue;
            }
            let pv = cross(dir, e2);
            let det = dot(e1, pv);
            let inv = 1.0 / det;
            let tv = origin - a;
            let u = dot(tv, pv) * inv;
            let qv = cross(tv, e1);
            let v = dot(dir, qv) * inv;
            let t = dot(e2, qv) * inv;
            if (u < 0.0 || v < 0.0 || u + v > 1.0 || t < near || t > best_t) {
                continue;
            }
            best.hit = true;
            best_t = t;
            best.normal = normalize(n);
            best_base = cell + base;
            best_case = case_index;
        }
    }
    if (best.hit) {
        best.t = best_t;
        best.point = origin + dir * best_t;
        var nearest = 1e9;
        for (var i = 0; i < 8; i = i + 1) {
            let corner = vec3<i32>(i & 1, (i >> 1u) & 1, (i >> 2u) & 1);
            let offset = best.point - (vec3<f32>(best_base + corner) + vec3<f32>(0.5));
            let dist = dot(offset, offset);
            if (((best_case >> u32(i)) & 1u) != 0u && dist < nearest) {
                nearest = dist;
                best.cell = best_base + corner;
            }
        }
    }
    return best;
}

// Whether marching cubes can reach into an empty voxel: only from a solid
// neighbour across one of its faces -- bits 4, 10, 12, 14, 16 and 22 of its
// neighbourhood.
fn mc_reaches(cell: vec3<i32>) -> bool {
    return (mc_around(cell) & 0x415410u) != 0u;
}

// The shape in a solid cell, entered through the face `entry` points out of.
fn shape_in(origin: vec3<f32>, dir: vec3<f32>, cell: vec3<i32>, t_in: f32, t_out: f32, entry: vec3<f32>) -> ShapeHit {
    let kind = SHAPE;
    if (kind == 1u) {
        return sphere_in(origin, dir, cell, t_in, t_out);
    }
    if (kind == 2u) {
        let c = vec2<f32>(cell.xy) + vec2<f32>(0.5);
        let z0 = f32(cell.z);
        let below = voxel_at(cell - vec3<i32>(0, 0, 1)) == 0u;
        let above = voxel_at(cell + vec3<i32>(0, 0, 1)) == 0u;
        return cylinder_in(origin, dir, c, p.shape.y, z0, z0 + 1.0, below, above, t_in, t_out);
    }
    if (kind == 4u) {
        return clay_in(origin, dir, t_in, t_out, entry);
    }
    if (kind == 5u) {
        return mc_in(origin, dir, cell, t_in, t_out);
    }
    // Bricks: the voxel is a cube.
    var h: ShapeHit;
    h.hit = true;
    h.t = t_in;
    h.normal = entry;
    h.point = face_point(origin, dir, t_in, cell, entry);
    return h;
}

// A brick's stud, in the empty cell above it: 0.3 round, 0.1875 high.
fn stud_in(origin: vec3<f32>, dir: vec3<f32>, cell: vec3<i32>, t_in: f32, t_out: f32) -> ShapeHit {
    let c = vec2<f32>(cell.xy) + vec2<f32>(0.5);
    let z0 = f32(cell.z);
    return cylinder_in(origin, dir, c, 0.3, z0, z0 + 0.1875, false, true, t_in, t_out);
}

// mode 0: stop at the first solid voxel.
// mode 1: stop at the first voxel that is not glass, empty included. A ray
//         that has entered glass starts inside solid material, where mode 0
//         would stop immediately; this is how it finds the far side.
fn trace(origin: vec3<f32>, direction: vec3<f32>, max_distance: f32, mode: u32) -> Hit {
    var result: Hit;
    result.hit = false;
    result.distance = max_distance;

    // Jump straight to where the ray enters the grid. The walk's step budget
    // scales with the grid, so a camera pulled far back from a wide scene
    // would otherwise run out of steps in the empty space before reaching
    // it. Mode 1 starts inside glass by definition and needs no clipping.
    var start = origin;
    var skipped = 0.0;
    var normal = vec3<f32>(0.0);
    if (mode == 0u) {
        let size = vec3<f32>(p.volume_size.xyz);
        let safe = select(direction, vec3<f32>(1e-8), abs(direction) < vec3<f32>(1e-8));
        let ta = -origin / safe;
        let tb = (size - origin) / safe;
        let near = min(ta, tb);
        let far_t = max(ta, tb);
        let t_enter = max(max(near.x, near.y), near.z);
        let t_exit = min(min(far_t.x, far_t.y), far_t.z);
        if (t_exit < max(t_enter, 0.0) || t_enter > max_distance) {
            return result;
        }
        if (t_enter > 0.0) {
            skipped = t_enter;
            start = origin + direction * t_enter;
            // The face the ray came in through, for a hit on the first cell.
            if (near.x >= near.y && near.x >= near.z) {
                normal = vec3<f32>(-sign(direction.x), 0.0, 0.0);
            } else if (near.y >= near.z) {
                normal = vec3<f32>(0.0, -sign(direction.y), 0.0);
            } else {
                normal = vec3<f32>(0.0, 0.0, -sign(direction.z));
            }
        }
    }

    var cell = vec3<i32>(floor(start));
    if (skipped > 0.0) {
        // Rounding can leave the entry point a hair outside the box.
        cell = clamp(cell, vec3<i32>(0), vec3<i32>(p.volume_size.xyz) - vec3<i32>(1));
    }
    let step = vec3<i32>(sign(direction));

    let inv = 1.0 / max(abs(direction), vec3<f32>(1e-8));
    var t_max = vec3<f32>(
        max(select(start.x - f32(cell.x), f32(cell.x) + 1.0 - start.x, direction.x > 0.0), 0.0) * inv.x,
        max(select(start.y - f32(cell.y), f32(cell.y) + 1.0 - start.y, direction.y > 0.0), 0.0) * inv.y,
        max(select(start.z - f32(cell.z), f32(cell.z) + 1.0 - start.z, direction.z > 0.0), 0.0) * inv.z
    ) + vec3<f32>(skipped);

    var travelled = skipped;
    let limit = i32(p.volume_size.x + p.volume_size.y + p.volume_size.z) * 3 + 32;

    for (var i = 0; i < limit; i = i + 1) {
        if (travelled > max_distance) {
            break;
        }
        var stop = voxel_at(cell) != 0u;
        if (mode == 1u) {
            stop = !is_glass(cell);
        }
        // Voxels drawn as other shapes: the shape in the cell, or on.
        if (SHAPE != 0u && mode == 0u && p.shape.x > 0.5) {
            let t_out = min(min(t_max.x, t_max.y), t_max.z);
            let marching = SHAPE == 5u;
            if (stop && shaped(voxel_at(cell))) {
                let s = shape_in(origin, direction, cell, travelled, t_out, normal);
                stop = false;
                if (s.hit) {
                    result.hit = true;
                    result.cell = select(cell, s.cell, marching);
                    result.normal = s.normal;
                    result.distance = s.t;
                    result.point = s.point;
                    return result;
                }
            } else if (!stop && marching && mc_reaches(cell)) {
                let s = mc_in(origin, direction, cell, travelled, t_out);
                if (s.hit) {
                    result.hit = true;
                    result.cell = s.cell;
                    result.normal = s.normal;
                    result.distance = s.t;
                    result.point = s.point;
                    return result;
                }
            } else if (!stop && SHAPE == 3u) {
                let under = cell - vec3<i32>(0, 0, 1);
                if (voxel_at(under) != 0u && shaped(voxel_at(under))) {
                    let s = stud_in(origin, direction, cell, travelled, t_out);
                    if (s.hit) {
                        // Coloured as the brick it sits on.
                        result.hit = true;
                        result.cell = under;
                        result.normal = s.normal;
                        result.distance = s.t;
                        result.point = s.point;
                        return result;
                    }
                }
            }
        }
        if (stop) {
            result.hit = true;
            result.cell = cell;
            result.normal = normal;
            result.distance = travelled;
            result.point = face_point(origin, direction, travelled, cell, normal);
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
    for (var k = 0; k < 12; k = k + 1) {
        let h = trace(p, direction, far, 0u);
        if (!h.hit) {
            return transmittance;
        }
        let index = voxel_at(h.cell);
        let m = materials[index];
        if (m.kind == KIND_MEDIA) {
            // Through the medium, attenuated by it, and on from where it
            // ends.
            var unused = 1u;
            let w = walk_medium(h.point + direction * 0.001, direction, false, &unused);
            transmittance = transmittance * w.transmittance;
            if (max(transmittance.x, max(transmittance.y, transmittance.z)) < 1e-4) {
                return vec3<f32>(0.0);
            }
            p = w.point + direction * 0.001;
            continue;
        }
        if (m.kind != KIND_GLASS) {
            return vec3<f32>(0.0);
        }
        let inside = h.point - h.normal * 0.001;
        let out = trace(inside, direction, far, 1u);
        transmittance = transmittance * m.amount
            * absorb(palette[index].rgb, m.emission.w, out.distance);
        if (!out.hit) {
            return transmittance;
        }
        // out.normal faces back into the glass; step past the boundary.
        p = out.point - out.normal * 0.001;
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

        // The face's own axis as a mask, rather than writing `v[axis]`:
        // a dynamically indexed write to a local vector is not
        // addressable in HLSL, so the DirectX shader compiler (FXC) must
        // unroll this loop to compile it -- and cannot, with a whole grid
        // walk inside -- and the path tracer failed to build on Windows.
        let along = vec3<f32>(select(vec3<i32>(0), vec3<i32>(1), vec3<i32>(axis) == vec3<i32>(0, 1, 2)));
        let side = select(0.0, 1.0, positive);
        let random_point = vec3<f32>(cell) + vec3<f32>(random(state), random(state), random(state));
        let on_face = mix(random_point, vec3<f32>(cell) + vec3<f32>(side), along);
        let face_normal = along * select(-1.0, 1.0, positive);

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
// `_hor * 0.0005` is an extinction per voxel, so the fade is Beer-Lambert
// over the distance.
fn horizon_fade(distance: f32) -> f32 {
    return 1.0 - exp(-p.ground.z * 0.0005 * distance);
}

// A direction toward the environment map, and the light it brings to a
// lambertian surface if nothing is in the way (zero when the sample is
// wasted). The shadow ray is the caller's.
struct EnvSample {
    direction: vec3<f32>,
    light: vec3<f32>,
};

// Next event estimation toward the environment map.
//
// Picks a texel with probability proportional to its weight by binary search
// over the CDF, then a direction inside it uniform in cos θ -- uniform in
// solid angle within the texel, which makes the pdf exact instead of
// approximate near the poles.
fn pick_env(normal: vec3<f32>, state: ptr<function, u32>) -> EnvSample {
    var none: EnvSample;
    none.direction = normal;
    none.light = vec3<f32>(0.0);
    if (p.ibl.x < 0.5 || p.ibl.w <= 0.0) {
        return none;
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
        return none;
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
        return none;
    }
    // Below the horizon the ground plane is in the way.
    if (p.ground.y > 0.5 && dir.z < 0.0) {
        return none;
    }

    let solid_angle = (2.0 * PI / f32(w)) * (cos0 - cos1);
    let pdf = (weight / p.ibl.w) / solid_angle;
    // The sampled texel's own value: consistent with the weight it was
    // chosen by, where a bilinear lookup would blur across neighbours.
    let radiance = env_texel(i32(x), i32(y)) * p.ibl.y;
    var sample: EnvSample;
    sample.direction = dir;
    sample.light = radiance * cos_surface / (PI * pdf);
    return sample;
}

// A point on the unit aperture: a disc, or a regular polygon with one corner
// per blade, which is what gives out-of-focus highlights their shape.
fn aperture_sample(state: ptr<function, u32>) -> vec2<f32> {
    let blades = i32(p.lens.z);
    if (blades < 3) {
        let r = sqrt(random(state));
        let t = 6.283185307 * random(state);
        return vec2<f32>(cos(t), sin(t)) * r;
    }
    // Pick one triangle of the fan, then a uniform point inside it.
    let step = 6.283185307 / f32(blades);
    let k = min(i32(random(state) * f32(blades)), blades - 1);
    let a0 = p.lens.w + step * f32(k);
    let v0 = vec2<f32>(cos(a0), sin(a0));
    let v1 = vec2<f32>(cos(a0 + step), sin(a0 + step));
    var u = random(state);
    var v = random(state);
    if (u + v > 1.0) {
        u = 1.0 - u;
        v = 1.0 - v;
    }
    return v0 * u + v1 * v;
}

fn is_media(index: u32) -> bool {
    return index != 0u && materials[index].kind == KIND_MEDIA;
}

// Henyey-Greenstein phase function, normalised over the sphere. `cos_theta`
// is between the direction light travels and the direction it leaves in.
fn hg_phase(cos_theta: f32, g: f32) -> f32 {
    let d = 1.0 + g * g - 2.0 * g * cos_theta;
    return (1.0 - g * g) / (4.0 * PI * pow(max(d, 1e-6), 1.5));
}

// A new direction after scattering, distributed by the phase function
// around the direction of travel.
fn sample_hg(travel: vec3<f32>, g: f32, state: ptr<function, u32>) -> vec3<f32> {
    let u1 = random(state);
    let u2 = random(state);
    var cos_t: f32;
    if (abs(g) < 1e-3) {
        cos_t = 1.0 - 2.0 * u1;
    } else {
        let s = (1.0 - g * g) / (1.0 - g + 2.0 * g * u1);
        cos_t = (1.0 + g * g - s * s) / (2.0 * g);
    }
    let sin_t = sqrt(max(0.0, 1.0 - cos_t * cos_t));
    let phi = 2.0 * PI * u2;
    var tangent: vec3<f32>;
    if (abs(travel.x) > 0.9) {
        tangent = normalize(cross(travel, vec3<f32>(0.0, 1.0, 0.0)));
    } else {
        tangent = normalize(cross(travel, vec3<f32>(1.0, 0.0, 0.0)));
    }
    let bitangent = cross(travel, tangent);
    return normalize(travel * cos_t + (tangent * cos(phi) + bitangent * sin(phi)) * sin_t);
}

struct MediumWalk {
    // Stopped by a scattering event, at `point`, in a cell of colour
    // `albedo` and phase `g`.
    scattered: bool,
    point: vec3<f32>,
    albedo: vec3<f32>,
    g: f32,
    // Light let through so far, and light the medium itself gave off.
    transmittance: vec3<f32>,
    emitted: vec3<f32>,
};

// Walks from `start` (inside a medium cell) cell by cell while the cells are
// media, until a scattering event or the first cell that is not a medium
// (`point` is then where the ray leaves). With `sample` false the walk only
// attenuates -- what a shadow ray needs.
//
// Each cell is homogeneous, so free flight inside it is sampled exactly:
// distance -ln(u)/density, accepted if it ends inside the cell.
fn walk_medium(start: vec3<f32>, dir: vec3<f32>, sample: bool, state: ptr<function, u32>) -> MediumWalk {
    var w: MediumWalk;
    w.scattered = false;
    w.point = start;
    w.albedo = vec3<f32>(1.0);
    w.g = 0.0;
    w.transmittance = vec3<f32>(1.0);
    w.emitted = vec3<f32>(0.0);

    var cell = vec3<i32>(floor(start));
    let step = vec3<i32>(sign(dir));
    let inv = 1.0 / max(abs(dir), vec3<f32>(1e-8));
    var t_max = vec3<f32>(
        select(start.x - f32(cell.x), f32(cell.x) + 1.0 - start.x, dir.x > 0.0) * inv.x,
        select(start.y - f32(cell.y), f32(cell.y) + 1.0 - start.y, dir.y > 0.0) * inv.y,
        select(start.z - f32(cell.z), f32(cell.z) + 1.0 - start.z, dir.z > 0.0) * inv.z
    );
    var t = 0.0;
    let limit = i32(p.volume_size.x + p.volume_size.y + p.volume_size.z) * 3 + 32;
    for (var i = 0; i < limit; i = i + 1) {
        let index = voxel_at(cell);
        if (!is_media(index)) {
            w.point = start + dir * t;
            return w;
        }
        let m = materials[index];
        let albedo = palette[index].rgb;
        let density = m.emission.w;
        let kind = u32(m.amount + 0.5);
        let t_exit = min(t_max.x, min(t_max.y, t_max.z));
        let len = max(t_exit - t, 0.0);

        if (kind == 0u) {
            // Absorbing: tinted like coloured glass.
            w.transmittance = w.transmittance * absorb(albedo, density, len);
        } else if (kind == 2u) {
            // Glowing: adds its colour along the way.
            w.emitted = w.emitted + w.transmittance * albedo * density * len;
        } else if (sample) {
            let s = -log(max(1.0 - random(state), 1e-7)) / max(density, 1e-6);
            if (s < len) {
                w.scattered = true;
                w.point = start + dir * (t + s);
                w.albedo = albedo;
                w.g = m.roughness;
                return w;
            }
        } else {
            w.transmittance = w.transmittance * exp(-density * len);
        }

        t = t_exit;
        if (t_max.x < t_max.y && t_max.x < t_max.z) {
            t_max.x = t_max.x + inv.x;
            cell.x = cell.x + step.x;
        } else if (t_max.y < t_max.z) {
            t_max.y = t_max.y + inv.y;
            cell.y = cell.y + step.y;
        } else {
            t_max.z = t_max.z + inv.z;
            cell.z = cell.z + step.z;
        }
    }
    w.point = start + dir * t;
    return w;
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
// Where a ray travelling inside glass ends up.
struct GlassWalk {
    // 0: out into the air, refracted; 1: onto an opaque voxel behind the
    // glass (`hit`); 2: lost (left the grid, or kept reflecting).
    outcome: u32,
    origin: vec3<f32>,
    direction: vec3<f32>,
    hit: Hit,
    // Distance travelled inside, for the absorption.
    inside: f32,
};

// Walks a ray through glass from a point inside it. At a boundary with
// empty space it refracts out (or reflects back in, past the critical
// angle). At a boundary with an opaque voxel -- water over sand, glass on a
// table -- it stops: that face is lit like any other surface, through the
// glass. Treating it as an exit to air, as this used to, pushed the ray
// into the solid, where it found the voxel at distance 0 with no normal and
// came back black.
fn walk_glass(start: vec3<f32>, dir: vec3<f32>, ior: f32, far: f32) -> GlassWalk {
    var w: GlassWalk;
    w.outcome = 2u;
    w.inside = 0.0;
    var q = start;
    var d = dir;
    for (var k = 0; k < 8; k = k + 1) {
        let out = trace(q, d, far, 1u);
        w.inside = w.inside + out.distance;
        if (!out.hit) {
            return w; // left the grid while inside
        }
        if (voxel_at(out.cell) != 0u) {
            w.outcome = 1u;
            w.hit = out;
            w.origin = q;
            w.direction = d;
            return w;
        }
        // out.normal faces back into the glass, which is the side the ray
        // arrives from -- what refract() expects.
        let bent = refract(d, out.normal, ior);
        if (dot(bent, bent) < 1e-6) {
            // Total internal reflection: bounce and keep going.
            d = reflect(d, out.normal);
            q = out.point + out.normal * 0.001;
            continue;
        }
        w.outcome = 0u;
        w.origin = out.point - out.normal * 0.001;
        w.direction = normalize(bent);
        return w;
    }
    return w;
}

fn fresnel(cos_theta: f32, ior: f32) -> f32 {
    var r0 = (1.0 - ior) / (1.0 + ior);
    r0 = r0 * r0;
    return r0 + (1.0 - r0) * pow(1.0 - clamp(cos_theta, 0.0, 1.0), 5.0);
}

// `include_sun` is false after a diffuse bounce: the sun is already sampled
// directly there, and adding the disc again when a bounce happens to escape
// toward it would count that light twice.
// One stage of the render, for finding where a GPU goes wrong: 1 a flat
// grey, 2 the primary hit's normal, 3 its palette colour, 4 the sky alone.
// A ray that hits nothing shows dark purple.
// 1 and 4 come before any tracing; the rest take the camera ray's first hit
// from the path loop, which keeps a second grid walk out of the shader.
fn debug_view(mode: u32, direction: vec3<f32>) -> vec3<f32> {
    if (mode == 1u) {
        return vec3<f32>(0.5);
    }
    return sky(direction, true, false);
}

fn debug_hit(mode: u32, hit: Hit) -> vec3<f32> {
    if (!hit.hit) {
        return vec3<f32>(0.1, 0.0, 0.1);
    }
    if (mode == 2u) {
        return hit.normal * 0.5 + vec3<f32>(0.5);
    }
    return palette[voxel_at(hit.cell)].rgb;
}

fn sky(direction: vec3<f32>, include_sun: bool, env_sampled: bool) -> vec3<f32> {
    // Uniform sky or environment map, plus a sun disc (_uni / _ibl, _inf).
    // After a diffuse bounce (`include_sun` false) an environment map is
    // left out too: it is importance-sampled at the surface, and adding it
    // again when a bounce escapes would count it twice.
    // `env_sampled`: the map was importance-sampled at the last vertex
    // (diffuse surfaces do; media and mirrors do not).
    var light = vec3<f32>(0.0);
    if (p.ibl.x < 0.5 || !env_sampled) {
        light = background(direction);
    }
    let alignment = dot(direction, normalize(p.sun_direction.xyz));
    if (include_sun && alignment > cos(p.sun_angular_radius)) {
        light = light + p.sun_color.rgb * p.sun_intensity;
    }
    return light;
}

// The energy limit (`_bounce._energy`): a contribution whose brightest
// channel passes it is scaled down to it, keeping its hue. Rare bright
// paths are what show as fireflies. `deep` is false at the first hit.
fn energy(light: vec3<f32>, deep: bool) -> vec3<f32> {
    let limit = p.sample.y;
    if (!deep || limit <= 0.0) {
        return light;
    }
    return light * (limit / max(max(light.x, max(light.y, light.z)), limit));
}

@compute @workgroup_size(8, 8, 1)
fn main(@builtin(global_invocation_id) local_id: vec3<u32>) {
    // The pixel: the dispatch covers one band of rows (`p.tile`).
    let gid = vec2<u32>(local_id.x, local_id.y + p.tile.x);
    if (gid.x >= p.resolution.x || local_id.y >= p.tile.y || gid.y >= p.resolution.y) {
        return;
    }
    let index = gid.y * p.resolution.x + gid.x;

    var rng = gid.x * 1973u + gid.y * 9277u + p.frame * 26699u;
    rng = rng | 1u;

    // Jitter inside the pixel: that is what turns accumulation into
    // antialiasing instead of just averaging the same sample repeatedly.
    var jitter = vec2<f32>(0.5);
    if (p.sample.z > 0.5) {
        jitter = vec2<f32>(random(&rng), random(&rng));
    }
    let pixel = (vec2<f32>(gid.xy) + jitter) / vec2<f32>(p.resolution);
    let ndc = vec2<f32>(pixel.x * 2.0 - 1.0, 1.0 - pixel.y * 2.0);

    var direction = normalize(
        p.forward.xyz
        + p.right.xyz * (ndc.x * p.tan_half_fov * p.aspect)
        + p.up.xyz * (ndc.y * p.tan_half_fov)
    );
    // The wide lenses, as `LensProjection::direction` in pathtrace.rs.
    let st = vec2<f32>(ndc.x * p.aspect, ndc.y) * p.projection.z;
    if (p.projection.x == 0.0 && p.projection.y > 0.5) {
        var local: vec3<f32>;
        if (p.projection.y < 1.5) {
            local = vec3<f32>(2.0 * st, 1.0 - dot(st, st));
        } else {
            local = vec3<f32>(cos(st.y) * sin(st.x), sin(st.y), cos(st.y) * cos(st.x));
        }
        direction = normalize(p.right.xyz * local.x + p.up.xyz * local.y + p.forward.xyz * local.z);
    }
    var origin = p.origin.xyz + p.volume_offset.xyz;

    // Thin lens: start the ray somewhere on the aperture and aim it at the
    // point this pixel sees in focus. In perspective that is a plane at the
    // focus depth along the view; the wide lenses focus at that distance
    // along each ray, the lens across the ray. Points in focus stay sharp;
    // the rest blurs with distance from them.
    if (p.lens.x > 0.0 && p.lens.y > 0.0 && p.projection.x == 0.0) {
        let on_lens = aperture_sample(&rng) * (p.lens.x * p.lens.y);
        if (p.projection.y < 0.5) {
            let focus = origin + direction * (p.lens.y / dot(direction, p.forward.xyz));
            origin = origin + p.right.xyz * on_lens.x + p.up.xyz * on_lens.y;
            direction = normalize(focus - origin);
        } else {
            let focus = origin + direction * p.lens.y;
            var across = cross(direction, p.up.xyz);
            if (dot(across, across) < 1e-6) {
                across = cross(direction, p.right.xyz);
            }
            across = normalize(across);
            let above = cross(across, direction);
            origin = origin + across * on_lens.x + above * on_lens.y;
            direction = normalize(focus - origin);
        }
    }
    // Distance the primary ray starts behind the eye, not to be counted
    // as distance through the haze toward the horizon.
    var pulled_back = 0.0;
    if (p.projection.x > 0.0) {
        // Parallel rays from a plane through the eye, pulled back so the
        // whole grid is in front of them: in a parallel view nothing may
        // be cut off. Only as far as that, though. Pulled back a fixed
        // 50000 voxels, a point on a surface (origin + direction * t) kept
        // ~0.004 of precision, more than the 0.001 a shadow ray steps off
        // the surface, so surfaces shadowed themselves in streaks: a lit
        // chimney rendered dark and see-through-looking in iso views.
        let centre = vec3<f32>(p.volume_size.xyz) * 0.5;
        pulled_back = length(origin - centre) + length(vec3<f32>(p.volume_size.xyz)) + 1.0;
        let h = p.projection.x;
        direction = p.forward.xyz;
        origin = origin
            + p.right.xyz * (ndc.x * h * p.aspect)
            + p.up.xyz * (ndc.y * h)
            - p.forward.xyz * pulled_back;
    }

    var radiance = vec3<f32>(0.0);
    var throughput = vec3<f32>(1.0);
    // Camera rays and mirror/glass bounces see emitters and the sun disc
    // directly; after a diffuse bounce both are already counted by direct
    // sampling, so they are skipped to avoid adding the same light twice.
    var specular = true;
    // Whether the environment map was sampled directly at the last vertex.
    var env_sampled = false;
    // Crossing a medium without scattering is not a bounce; this bounds how
    // many such crossings one path may make.
    var crossings = 0;
    // Scattering events in media and fog so far; they have their own budget.
    var scatters = 0;
    let far = f32(p.volume_size.x + p.volume_size.y + p.volume_size.z) * 2.0;
    // Whether the path has met anything yet -- a voxel, the ground, the
    // fog. A camera ray that has not is the background, and transparent
    // or a flat colour when the render asks for that.
    var met = false;
    // An opaque voxel reached through glass, to shade next iteration.
    var forced = false;
    var forced_hit: Hit;

    // Each kind of event has its own depth, as in the editor (`_bounce`):
    // diffuse bounces after the first hit (`_diffuse`; 0 is direct light
    // only), mirror and glass events (`_specular`) and scattering in media
    // and fog (`_scatter`). A path ends when one of them runs out.
    var diffuse_events = 0;
    var specular_events = 0;
    // A diagnostic view replaces the whole path.
    var steps = 128;
    let debug = p.emitter_info.y;
    if (debug == 1u || debug == 4u) {
        radiance = debug_view(debug, direction);
        steps = 0;
    }

    for (var step = 0; step < steps; step = step + 1) {
        // The editor's cap on all events together.
        if (diffuse_events + specular_events + scatters > 15) {
            break;
        }
        // Light reaching the camera after the first event is held to the
        // energy limit; direct light at the first hit never is.
        let deep = diffuse_events + specular_events + scatters > 0;
        // A ray that starts inside glass (a bounce off a voxel under water)
        // first walks out of it, or onto the next opaque voxel.
        var from_glass = forced;
        var hit = forced_hit;
        forced = false;
        let start_cell = vec3<i32>(floor(origin));
        if (!from_glass && is_glass(start_cell)) {
            let g = voxel_at(start_cell);
            let w = walk_glass(origin, direction, materials[g].ior, far);
            throughput = throughput * absorb(palette[g].rgb, materials[g].emission.w, w.inside);
            if (w.outcome == 2u) {
                break;
            }
            origin = w.origin;
            direction = w.direction;
            if (w.outcome == 1u) {
                hit = w.hit;
                from_glass = true;
            }
        }
        if (!from_glass) {
            // No distance cap here: the camera may sit far outside the
            // grid, and trace() clips to the grid box itself.
            hit = trace(origin, direction, 1e30, 0u);
        }
        if (debug != 0u) {
            radiance = debug_hit(debug, hit);
            break;
        }

        // The ground wins when it is nearer than any voxel, or when the ray
        // left the grid without hitting one. Not from inside glass: the
        // voxel reached there is already the nearest thing.
        let t_ground = select(ground_distance(origin, direction), -1.0, from_glass);

        // Where the path goes on from. Each event below that neither ends
        // nor turns the path itself only fills these in, and the lighting
        // after them is written once: one copy per event made the shader
        // too large for Windows' software GPU (WARP), which then dropped
        // parts of the result and rendered black. `scattered` is a
        // scattering in fog or a medium, lit through the phase function
        // `phase_g`; otherwise `vertex` is on a diffuse surface facing
        // `normal`, its shadow rays reaching `sun_reach`.
        var scattered = false;
        var last_scatter = false;
        var phase_g = 0.0;
        var vertex = origin;
        var normal = vec3<f32>(0.0, 0.0, 1.0);
        var sun_reach = far;

        // Fog: a homogeneous medium everywhere. Sample how far the ray goes
        // before it scatters; if that is before whatever it would reach,
        // it scatters there instead.
        if (p.fog.x > 0.0 && !from_glass && scatters >= i32(p.fog.z)) {
            // Fog scatters once a path; after that it only dims.
            var reach = far;
            if (hit.hit) {
                reach = min(reach, hit.distance);
            }
            if (t_ground > 0.0) {
                reach = min(reach, t_ground);
            }
            throughput = throughput * exp(-p.fog.x * max(reach - pulled_back, 0.0));
        } else if (p.fog.x > 0.0 && !from_glass) {
            var reach = 1e5;
            if (hit.hit) {
                reach = hit.distance;
            }
            if (t_ground > 0.0) {
                reach = min(reach, t_ground);
            }
            let s = pulled_back - log(max(1.0 - random(&rng), 1e-7)) / p.fog.x;
            if (s < reach) {
                met = true;
                // The fog does not dim the sun on its way in: an infinite
                // medium would leave no sunlight at all.
                throughput = throughput * p.fog_color.rgb;
                scattered = true;
                phase_g = p.fog.y;
                vertex = origin + direction * s;
                pulled_back = 0.0;
                // Fog scatters a path once: it spends the whole budget.
                scatters = i32(p.fog.z);
            }
        }
        if (!scattered && t_ground > 0.0 && (!hit.hit || t_ground < hit.distance)) {
            met = true;
            let point = origin + direction * t_ground;
            let up = vec3<f32>(0.0, 0.0, 1.0);

            // Distant ground dissolves into the sky *at the horizon*. Not
            // the background straight along the ray: below the horizon that
            // is the photo's ground in an HDR, or a short dark path into the
            // planet under the atmosphere, and the editor keeps its ground
            // grey in both.
            let fade = horizon_fade(max(t_ground - pulled_back, 0.0));
            pulled_back = 0.0;
            let horizon = normalize(vec3<f32>(direction.xy, 0.02 * length(direction.xy)));
            radiance = radiance + energy(throughput * fade * background(horizon), deep);
            throughput = throughput * (1.0 - fade) * lined(p.ground_color.rgb, point, up, vec3<i32>(0), true);

            // A diffuse surface like any other: sun, emitters, a bounce.
            vertex = point + up * 0.001;
            normal = up;
            sun_reach = 1e5;
        } else if (!scattered) {
            pulled_back = 0.0;
            if (!hit.hit) {
                // Only bounces light from the sky once the background is a
                // colour or nothing: the camera does not see it.
                if (met || p.look.x < 0.5) {
                    radiance = radiance + energy(throughput * sky(direction, specular, env_sampled), deep);
                }
                break;
            }
            met = true;

            let index = voxel_at(hit.cell);
            var albedo = palette[index].rgb;
            // Lines follow the cubes' faces; other shapes go without.
            if (p.shape.x < 0.5 && (p.lines.x > 0.0 || p.lines.w > 0.0)) {
                albedo = lined(albedo, hit.point, hit.normal, hit.cell, false);
            }
            let m = materials[index];
            let surface = hit.point;
            // Offset off the surface, or the next ray re-hits the voxel it left.
            let outside = surface + hit.normal * 0.001;

            if ((m.kind == KIND_EMIT || m.kind == KIND_BLEND) && specular) {
                radiance = radiance + energy(throughput * m.emission.rgb, deep);
            }

            if (m.kind == KIND_MEDIA) {
                let w = walk_medium(surface + direction * 0.001, direction, true, &rng);
                radiance = radiance + energy(throughput * w.emitted, deep);
                throughput = throughput * w.transmittance;
                if (!w.scattered) {
                    // Straight through: on from where the medium ends,
                    // without spending a bounce.
                    origin = w.point + direction * 0.001;
                    crossings = crossings + 1;
                    if (crossings >= 32) {
                        break;
                    }
                    continue;
                }
                // Scattered inside: direct sun through the phase function,
                // then a new direction drawn from it.
                throughput = throughput * w.albedo;
                scattered = true;
                phase_g = w.g;
                vertex = w.point;
                scatters = scatters + 1;
                last_scatter = scatters > i32(p.fog.z);
                // After a scattering in a cloud, one more of each surface
                // event.
                diffuse_events = max(diffuse_events, i32(p.bounces) - 1);
                specular_events = max(specular_events, i32(p.sample.x) - 1);
            } else {
                // Pick a lobe with probability equal to its weight.
                // Throughput needs no correction for that choice: each lobe
                // is sampled exactly as often as it contributes.
                let lobe = random(&rng);

                // Blend: metal for its share, then glass for its share of
                // the rest, then diffuse.
                let blend_glass = m.kind == KIND_BLEND && lobe >= m.amount
                    && (lobe - m.amount) / max(1.0 - m.amount, 1e-6) < m.extra.x;
                let mirror = (m.kind == KIND_METAL || m.kind == KIND_BLEND) && lobe < m.amount;
                if (mirror || (m.kind == KIND_GLASS && lobe < m.amount) || blend_glass) {
                    specular_events = specular_events + 1;
                    if (specular_events > i32(p.sample.x)) {
                        break;
                    }
                }
                if (mirror) {
                    // Mirror reflection, blurred by roughness. The CPU
                    // already applied the editor's 0.15 scale, so this is
                    // used as-is.
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
                    env_sampled = false;
                    continue;
                }

                if ((m.kind == KIND_GLASS && lobe < m.amount) || blend_glass) {
                    let cos_i = -dot(direction, hit.normal);
                    if (random(&rng) < fresnel(cos_i, m.ior)) {
                        // Specular reflection off the glass, untinted.
                        direction = reflect(direction, hit.normal);
                        origin = outside;
                        specular = true;
                        env_sampled = false;
                        continue;
                    }

                    // Refract in, walk through the glass, refract out -- or
                    // reach an opaque voxel behind it, shaded on the next
                    // iteration.
                    let refracted = refract(direction, hit.normal, 1.0 / m.ior);
                    let w = walk_glass(surface - hit.normal * 0.001, refracted, m.ior, far);
                    if (w.outcome == 2u) {
                        break;
                    }
                    throughput = throughput * absorb(albedo, m.emission.w, w.inside);
                    origin = w.origin;
                    direction = w.direction;
                    if (w.outcome == 1u) {
                        forced = true;
                        forced_hit = w.hit;
                    }
                    specular = true;
                    env_sampled = false;
                    continue;
                }

                // Diffuse (also the non-metal / non-glass share of mixed
                // materials, and the reflective part of emitters).
                throughput = throughput * albedo;
                vertex = outside;
                normal = hit.normal;
            }
        }

        // Direct light. The sun, weighted by the phase function in a medium
        // and by the angle on a surface; on a surface also the emissive
        // voxels and the map.
        let to_sun = normalize(p.sun_direction.xyz);
        var sun_weight = dot(normal, to_sun);
        if (scattered) {
            sun_weight = hg_phase(dot(direction, to_sun), phase_g) * PI;
        }
        var env: EnvSample;
        env.light = vec3<f32>(0.0);
        var rays = 1;
        if (!scattered) {
            radiance = radiance + energy(throughput * sample_emitters(vertex, normal, &rng), deep);
            env = pick_env(normal, &rng);
            rays = 2;
        }
        // The shadow rays toward the sun and the map's sample, from one
        // call in a loop: a second call was a second copy of the grid walk.
        for (var ray = 0; ray < rays; ray = ray + 1) {
            var toward = to_sun;
            var reach = sun_reach;
            var light = p.sun_color.rgb * p.sun_intensity * sun_weight;
            if (ray == 1) {
                toward = env.direction;
                reach = 1e5;
                light = env.light;
            }
            if ((sun_weight > 0.0 || ray == 1) && any(light > vec3<f32>(0.0))) {
                radiance = radiance + energy(throughput * shadow(vertex, toward, reach) * light, deep);
            }
        }
        if (scattered) {
            if (last_scatter) {
                break;
            }
            direction = sample_hg(direction, phase_g, &rng);
            origin = vertex;
            specular = false;
            env_sampled = false;
            continue;
        }

        diffuse_events = diffuse_events + 1;
        if (diffuse_events > i32(p.bounces)) {
            if (p.sample.w > 0.5) {
                radiance = radiance + energy(throughput * background(normal), deep);
            }
            break;
        }
        // After a diffuse bounce, at most one more mirror or glass event.
        specular_events = max(specular_events, i32(p.sample.x) - 1);
        // Cosine-weighted bounce; the lambertian term cancels.
        direction = sample_hemisphere(normal, &rng);
        origin = vertex;
        specular = false;
        env_sampled = true;
    }

    // Accumulate in linear light. Resetting on camera motion keeps a moved
    // view from averaging against frames of the old one.
    var total = vec4<f32>(radiance, 1.0);
    var covered = select(0.0, 1.0, met);
    if (p.reset == 0u) {
        total = total + accumulator[index];
        covered = covered + coverage[index];
    }
    accumulator[index] = total;
    coverage[index] = covered;

    // Exposure, bloom, vignette and the tone curve are applied by the
    // post-processing pass (post.wgsl), which can read neighbouring pixels.
}
