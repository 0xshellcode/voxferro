// xs_begin
// author : 'voxferro'
// arg : { var = 'm_from' name = 'From' value = '1' range = '1 255' step = '1' precision = '0' }
// xs_end

// Repaints every voxel of colour From with the current colour. Combined
// with a selection, only the selected voxels change.
fn map(v: vec3<f32>) -> f32 {
    let here = voxel(v);
    return select(here, i_color_index, here == m_from);
}
