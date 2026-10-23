// Which of each voxel and its 26 neighbours are solid, one mask a cell, for
// the path tracer's marching cubes.
//
// The tracer used to read all 27 voxels around every cell its rays met near
// a surface -- and the 6 beside every empty one, to know whether to look --
// which made the shape ten times slower than cubes. Computed here once
// whenever the volume changes, it is one read. Bit (x + 1) + 3 (y + 1) +
// 9 (z + 1) is the voxel at that offset; outside the grid is empty.

@group(0) @binding(0) var volume: texture_3d<u32>;
@group(0) @binding(1) var masks: texture_storage_3d<r32uint, write>;

@compute @workgroup_size(4, 4, 4)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let size = vec3<i32>(textureDimensions(volume));
    let cell = vec3<i32>(gid);
    if (any(cell >= size)) {
        return;
    }
    var around = 0u;
    for (var k = 0; k < 27; k = k + 1) {
        let at = cell + vec3<i32>(k % 3, (k / 3) % 3, k / 9) - vec3<i32>(1);
        if (all(at >= vec3<i32>(0)) && all(at < size) && textureLoad(volume, at, 0).x != 0u) {
            around = around | (1u << u32(k));
        }
    }
    textureStore(masks, cell, vec4<u32>(around, 0u, 0u, 0u));
}
