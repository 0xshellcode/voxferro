// xs_begin
// author : 'voxferro'
// arg : { var = 'm_level' name = 'Level' value = '3' range = '1 5' step = '1' precision = '0' }
// xs_end

// A Menger sponge on the largest cube that fits: a cell is removed when, at
// any level, at least two of its three digits in base 3 are the middle one.
fn map(v: vec3<f32>) -> f32 {
    let side = min(i_volume_size.x, min(i_volume_size.y, i_volume_size.z));
    var p = floor(v) / side;
    if (any(p >= vec3<f32>(1.0))) {
        return 0.0;
    }
    for (var level = 0; level < i32(m_level); level++) {
        p *= 3.0;
        let digit = floor(p);
        p -= digit;
        let middle = vec3<f32>(select(vec3<f32>(0.0), vec3<f32>(1.0), digit == vec3<f32>(1.0)));
        if (middle.x + middle.y + middle.z >= 2.0) {
            return 0.0;
        }
    }
    return i_color_index;
}
