// xs_begin
// author : 'voxferro'
// arg : { var = 'm_height' name = 'Height' value = '0.5'  range = '0 1'   step = '0.05' precision = '2' }
// arg : { var = 'm_scale'  name = 'Scale'  value = '24'   range = '4 128' step = '1'    precision = '0' }
// arg : { var = 'm_seed'   name = 'Seed'   value = '1'    range = '0 999' step = '1'    precision = '0' }
// xs_end

// Rolling hills from value noise. The top layer takes the current colour;
// with more than one colour selected, height bands take the selections in
// turn, bottom to top.

fn hash(p: vec2<f32>) -> f32 {
    let q = fract(p * vec2<f32>(127.1, 311.7) + m_seed * 0.618);
    return fract(sin(dot(q, vec2<f32>(12.9898, 78.233))) * 43758.547);
}

fn noise(p: vec2<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    let a = hash(i);
    let b = hash(i + vec2<f32>(1.0, 0.0));
    let c = hash(i + vec2<f32>(0.0, 1.0));
    let d = hash(i + vec2<f32>(1.0, 1.0));
    return mix(mix(a, b, u.x), mix(c, d, u.x), u.y);
}

fn map(v: vec3<f32>) -> f32 {
    var p = v.xy / m_scale;
    var h = 0.0;
    var amp = 0.5;
    for (var octave = 0; octave < 4; octave++) {
        h += noise(p) * amp;
        p *= 2.0;
        amp *= 0.5;
    }
    let top = h * m_height * 2.0 * i_volume_size.z;
    if (v.z > top) {
        return 0.0;
    }
    if (i_num_color_sels > 1) {
        return color_sel(floor(v.z / max(top, 1.0) * f32(i_num_color_sels)));
    }
    return i_color_index;
}
