// xs_begin
// author : 'voxferro'
// arg : { var = 'm_scale' name = 'Scale' value = '1' range = '0.1 1' step = '0.05' precision = '2' }
// xs_end

// A sphere (an ellipsoid on a box that is not a cube) filling the model.
fn map(v: vec3<f32>) -> f32 {
    let half = i_volume_size * 0.5;
    let u = (v - half) / (half * m_scale);
    return i_color_index * step(length(u), 1.0);
}
