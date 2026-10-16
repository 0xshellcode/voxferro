//! Keeping the path tracer's grid up to date without rebuilding it.
//!
//! The tracer walks one grid holding every visible instance (`Scene::bake`).
//! Rebuilding and uploading it took 70-85 ms on a 256³ model, on every edit.
//! When only voxels changed -- same instances, same placements, same model
//! sizes -- the chunks whose version moved are rewritten into the kept grid
//! and only the box they cover is uploaded.

use crate::App;
use eframe::egui_wgpu;
use glam::IVec3;
use voxel_core::mesh_cache::{changed_chunks, chunk_world_box, versions_of};
use voxel_core::volume::CHUNK;
use voxel_core::{Baked, Transform, world_cell};

pub struct TraceCache {
    baked: Baked,
    /// Visible instances as (model, placement) when baked.
    placements: Vec<(usize, Transform)>,
    /// Each model's size and chunk versions when baked.
    models: Vec<([u32; 3], Vec<u64>)>,
}

impl App {
    /// Brings the tracer's grid up to date: in place when it can, rebuilt
    /// when it must.
    pub fn sync_trace_volume(&mut self, state: &egui_wgpu::RenderState) {
        if self.update_trace_in_place(state) {
            return;
        }
        self.upload_trace_volume(state);
    }

    fn placements(&self) -> Vec<(usize, Transform)> {
        let visible = self.visibility();
        let Some(loaded) = &self.scene else { return Vec::new() };
        loaded
            .doc
            .scene
            .instances
            .iter()
            .filter(|i| visible.shows(i))
            .map(|i| (i.model_index, i.transform))
            .collect()
    }

    /// Records what the grid was baked from.
    pub fn remember_bake(&mut self, baked: Baked) {
        let placements = self.placements();
        let Some(loaded) = &self.scene else { return };
        let models = loaded.doc.scene.models.iter().map(|v| (v.size(), versions_of(v))).collect();
        self.trace_cache = Some(TraceCache { baked, placements, models });
    }

    /// Rewrites only the changed chunks. False when the scene changed in a
    /// way that needs a full bake.
    fn update_trace_in_place(&mut self, state: &egui_wgpu::RenderState) -> bool {
        let placements = self.placements();
        let Some(loaded) = &self.scene else { return false };
        let Some(cache) = &mut self.trace_cache else { return false };
        let models = &loaded.doc.scene.models;
        if cache.placements != placements
            || cache.models.len() != models.len()
            || cache.models.iter().zip(models).any(|(m, v)| m.0 != v.size())
        {
            return false;
        }

        let grid_size = cache.baked.volume.size();
        let inside = |w: IVec3| {
            w.x >= 0 && w.y >= 0 && w.z >= 0 && (w.x as u32) < grid_size[0] && (w.y as u32) < grid_size[1] && (w.z as u32) < grid_size[2]
        };
        let mut lo = IVec3::splat(i32::MAX);
        let mut hi = IVec3::splat(i32::MIN);
        let mut any = false;
        for (m, volume) in models.iter().enumerate() {
            let changed = changed_chunks(&cache.models[m].1, volume);
            if changed.is_empty() {
                continue;
            }
            for &(_, transform) in placements.iter().filter(|p| p.0 == m) {
                for &c in &changed {
                    let size = volume.size();
                    for z in c[2] * CHUNK..((c[2] + 1) * CHUNK).min(size[2]) {
                        for y in c[1] * CHUNK..((c[1] + 1) * CHUNK).min(size[1]) {
                            for x in c[0] * CHUNK..((c[0] + 1) * CHUNK).min(size[0]) {
                                let value = volume.get(x, y, z);
                                let g = world_cell(volume, &transform, IVec3::new(x as i32, y as i32, z as i32)) - cache.baked.origin;
                                if !inside(g) {
                                    if value != 0 {
                                        // Grew past the baked box.
                                        return false;
                                    }
                                    continue;
                                }
                                cache.baked.volume.set(g.x as u32, g.y as u32, g.z as u32, value);
                            }
                        }
                    }
                    let (a, b) = chunk_world_box(volume, &transform, c);
                    lo = lo.min(a - cache.baked.origin);
                    hi = hi.max(b - cache.baked.origin);
                    any = true;
                }
            }
            cache.models[m].1 = versions_of(volume);
        }
        if any {
            let max = IVec3::new(grid_size[0] as i32 - 1, grid_size[1] as i32 - 1, grid_size[2] as i32 - 1);
            let (lo, hi) = (lo.clamp(IVec3::ZERO, max), hi.clamp(IVec3::ZERO, max));
            self.path_tracer.update_volume_region(
                &state.queue,
                &cache.baked.volume,
                lo.as_uvec3().to_array(),
                hi.as_uvec3().to_array(),
            );
            self.path_tracer.set_emitters(&state.device, &cache.baked.volume, &loaded.doc.materials);
        }
        true
    }
}
