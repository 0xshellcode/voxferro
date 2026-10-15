// Post-processing of the accumulated image: bloom, exposure, vignette,
// tone mapping and gamma.
//
// A separate pass because bloom reads neighbouring pixels, which the tracing
// pass cannot do while those pixels are still being written.
//
// Bloom follows the uniforms the editor sets in the reference renderer:
// each frame gathers `iterations` random samples inside an ellipse of radius
// `_scale * 200 + 1` pixels, stretched by 2^(_aspect * 4) and turned by the
// lens blade angle; with lens blades the ellipse turns by pi / n every
// iteration, which draws a star. Only light above `_threshold` glows. The
// samples accumulate over frames like the image itself. How the editor's
// shader weights the samples and mixes the result is not in the binary; the
// falloff and the additive mix here are this renderer's choice.

struct Post {
    resolution: vec2<u32>,
    sample_count: u32,
    reset: u32,

    // x: exposure, y: vignette, z: gamma, w: ACES on (1) or off (0).
    film: vec4<f32>,
    // x: mix, y: threshold, z: radius in pixels, w: stretch along the axis.
    bloom: vec4<f32>,
    // x: iterations, y: base angle, z: angle step per iteration, w: seed.
    star: vec4<f32>,
};

@group(0) @binding(0) var<uniform> q: Post;
@group(0) @binding(1) var<storage, read> accumulator: array<vec4<f32>>;
@group(0) @binding(2) var<storage, read_write> glow: array<vec4<f32>>;
@group(0) @binding(3) var output: texture_storage_2d<rgba8unorm, write>;

fn pcg(state: ptr<function, u32>) -> u32 {
    let s = *state * 747796405u + 2891336453u;
    *state = s;
    let word = ((s >> ((s >> 28u) + 4u)) ^ s) * 277803737u;
    return (word >> 22u) ^ word;
}

fn random(state: ptr<function, u32>) -> f32 {
    return f32(pcg(state)) * (1.0 / 4294967296.0);
}

fn pixel(x: i32, y: i32) -> vec3<f32> {
    let w = i32(q.resolution.x);
    let h = i32(q.resolution.y);
    let px = clamp(x, 0, w - 1);
    let py = clamp(y, 0, h - 1);
    let a = accumulator[u32(py * w + px)];
    return a.rgb / max(a.a, 1.0);
}

@compute @workgroup_size(8, 8, 1)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= q.resolution.x || gid.y >= q.resolution.y) {
        return;
    }
    let index = gid.y * q.resolution.x + gid.x;
    var rng = (gid.x * 1973u + gid.y * 9277u + u32(q.star.w) * 26699u) | 1u;

    var color = pixel(i32(gid.x), i32(gid.y));

    // Bloom: this frame's estimate, folded into the running average.
    if (q.bloom.x > 0.0 && q.bloom.z > 1.0) {
        let iterations = max(i32(q.star.x), 1);
        var sum = vec3<f32>(0.0);
        for (var i = 0; i < iterations; i = i + 1) {
            let angle = q.star.y + q.star.z * f32(i);
            // A point in the unit disc, denser toward the centre so the glow
            // falls off instead of ending at a hard edge.
            let r = random(&rng);
            let theta = 6.283185307 * random(&rng);
            var d = vec2<f32>(cos(theta), sin(theta)) * r * r * q.bloom.z;
            d.x = d.x * q.bloom.w;
            let c = cos(angle);
            let s = sin(angle);
            let o = vec2<f32>(c * d.x - s * d.y, s * d.x + c * d.y);
            let sample = pixel(i32(f32(gid.x) + o.x), i32(f32(gid.y) + o.y));
            sum = sum + max(sample - vec3<f32>(q.bloom.y), vec3<f32>(0.0));
        }
        var estimate = vec4<f32>(sum / f32(iterations), 1.0);
        if (q.reset == 0u) {
            estimate = estimate + glow[index];
        }
        glow[index] = estimate;
        color = color + q.bloom.x * estimate.rgb / max(estimate.a, 1.0);
    }

    color = color * q.film.x;

    // Vignette: darken toward the corners, by the square of the distance
    // from the centre over the half diagonal.
    let uv = (vec2<f32>(gid.xy) + vec2<f32>(0.5)) / vec2<f32>(q.resolution) * 2.0 - vec2<f32>(1.0);
    let corner = dot(uv, uv) * 0.5;
    color = color * max(1.0 - q.film.y * corner, 0.0);

    if (q.film.w > 0.5) {
        // ACES filmic tone curve, the editor's default.
        let a = 2.51; let b = 0.03; let c = 2.43; let d = 0.59; let e = 0.14;
        color = (color * (a * color + b)) / (color * (c * color + d) + e);
    }
    color = clamp(color, vec3<f32>(0.0), vec3<f32>(1.0));
    // The storage texture is rgba8unorm (no automatic sRGB), so encode here.
    color = pow(color, vec3<f32>(1.0 / max(q.film.z, 0.1)));

    // Triangular dither of +-1 code before 8-bit quantisation: in bright sky
    // the tone curve's shoulder squeezes a wide range into a few codes, which
    // shows up as contour lines; grain below one code hides them.
    let dither = (random(&rng) + random(&rng) - 1.0) / 255.0;
    color = color + vec3<f32>(dither);

    textureStore(output, vec2<i32>(gid.xy), vec4<f32>(color, 1.0));
}
