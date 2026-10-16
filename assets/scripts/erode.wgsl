// xs_begin
// author : 'voxferro'
// arg : { var = 'm_amount' name = 'Amount' value = '0.5' range = '0 1' step = '0.05' precision = '2' }
// xs_end

// Wears surfaces away: a solid voxel with an empty face neighbour is
// removed with probability Amount. Repeat with `-n` for deeper wear;
// `i_rand` changes every run, `i_iter` every pass.

fn hash(p: vec3<f32>) -> f32 {
    let q = p + i_rand.xyz * 97.0 + i_iter * 13.0;
    return fract(sin(dot(q, vec3<f32>(12.9898, 78.233, 37.719))) * 43758.547);
}

fn map(v: vec3<f32>) -> f32 {
    let here = voxel(v);
    if (here == 0.0) {
        return 0.0;
    }
    let exposed = voxel(v + vec3<f32>(1.0, 0.0, 0.0)) == 0.0 || voxel(v - vec3<f32>(1.0, 0.0, 0.0)) == 0.0
        || voxel(v + vec3<f32>(0.0, 1.0, 0.0)) == 0.0 || voxel(v - vec3<f32>(0.0, 1.0, 0.0)) == 0.0
        || voxel(v + vec3<f32>(0.0, 0.0, 1.0)) == 0.0 || voxel(v - vec3<f32>(0.0, 0.0, 1.0)) == 0.0;
    if (exposed && hash(v) < m_amount) {
        return 0.0;
    }
    return here;
}
