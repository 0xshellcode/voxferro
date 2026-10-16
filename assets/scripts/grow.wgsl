// xs_begin
// author : 'voxferro'
// xs_end

// Adds a layer of the current colour around everything, one voxel thick
// per pass: `xs -n 3 grow` grows three. Empty cells next to a solid face
// fill; solid cells stay as they are.
fn map(v: vec3<f32>) -> f32 {
    let here = voxel(v);
    if (here > 0.0) {
        return here;
    }
    let near = voxel(v + vec3<f32>(1.0, 0.0, 0.0)) + voxel(v - vec3<f32>(1.0, 0.0, 0.0))
        + voxel(v + vec3<f32>(0.0, 1.0, 0.0)) + voxel(v - vec3<f32>(0.0, 1.0, 0.0))
        + voxel(v + vec3<f32>(0.0, 0.0, 1.0)) + voxel(v - vec3<f32>(0.0, 0.0, 1.0));
    return select(0.0, i_color_index, near > 0.0);
}
