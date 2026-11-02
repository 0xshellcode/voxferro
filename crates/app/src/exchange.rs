//! Exporting to the reference editor's other formats, and importing models from
//! files (see `voxel_core::formats`).
//!
//! `export [format] <path>`, the format being one of [`FORMATS`]; without
//! it, the path's extension decides (`.obj`, `.ply`, `.xraw`, `.qb`), and a
//! bare `export` asks for an OBJ path. The mesh formats (obj, ply, cube,
//! point) take every visible model, placed in the world; the volume
//! formats (xraw, qb, slab, slice) take the selected model, since each holds
//! one grid.
//!
//! Two switches, the reference editor's (`export_sel`, `export_local`, read from its
//! a recent version executable), shape what the mesh formats take: `export
//! selected on` only the selected objects, `export local on` each model in
//! its own coordinates -- its placement and its groups' left out, centred
//! on the origin. Both are off by default and kept for the session.
//!
//! `import <path>` adds the models of another file to this scene, each a new
//! model and instance, all one undo step: a the reference editor `.vox` (each model
//! where that file puts it, unturned), XRAW, Qubicle QB or SLAB6 -- told apart by their content,
//! not their extension, since SLAB6's is `.vox` too. Their colours move to
//! the nearest colours of this palette (the reference editor's "Match Color").
//! `import <file.obj> [size] [fill]` voxelises a mesh (see
//! `voxel_core::voxelize`): its longest side `size` voxels (64 by default),
//! hollow unless `fill`.

use crate::App;
use voxel_core::formats::{self, Imported};

/// The export formats, as the reference editor's Export buttons name them.
pub const FORMATS: [(&str, &str, &str); 11] = [
    ("obj", "obj", "Merged faces, Y up, with a palette texture"),
    ("ply", "ply", "A quad per visible face, coloured, scale 0.1"),
    ("mc", "ply", "A smooth surface by marching cubes, coloured, scale 0.1"),
    ("cube", "ply", "Every voxel a separate cube, scale 0.1"),
    ("point", "ply", "A coloured point per voxel"),
    ("xraw", "xraw", "The selected model as an XRAW volume"),
    ("qb", "qb", "The selected model as Qubicle Binary"),
    ("slab", "vox", "The selected model as a SLAB6 .vox"),
    ("slice", "png", "The selected model's layers stacked in one PNG"),
    ("iso", "png", "The selected model as isometric pixel sprites, four views"),
    ("2d", "png", "The selected model as a front-view pixel sprite"),
];

/// `io_ply`, `io_cube` and `io_point` in the reference editor's `config.txt`.
const MESH_SCALE: f32 = 0.1;
const CUBE_MARGIN: f32 = 0.1;

impl App {
    /// The scene the mesh exports write: the document's, or a copy with
    /// only the selected objects and each model at the origin, as the
    /// export switches ask.
    pub(crate) fn export_scene(&self) -> Option<std::borrow::Cow<'_, voxel_core::Scene>> {
        let loaded = self.scene.as_ref()?;
        let scene = &loaded.doc.scene;
        if !self.export_selected && !self.export_local {
            return Some(std::borrow::Cow::Borrowed(scene));
        }
        let mut copy = scene.clone();
        if self.export_selected {
            let chosen = self.object_indices();
            let chosen = if chosen.is_empty() { vec![self.selected] } else { chosen };
            copy.instances = scene.instances.iter().enumerate().filter(|(i, _)| chosen.contains(i)).map(|(_, inst)| inst.clone()).collect();
        }
        if self.export_local {
            for instance in &mut copy.instances {
                instance.transform = voxel_core::Transform::default();
                instance.parent = voxel_core::Transform::default();
            }
        }
        Some(std::borrow::Cow::Owned(copy))
    }

    pub fn export_command(&mut self, line: &str) -> String {
        let words: Vec<&str> = line.split_whitespace().skip(1).collect();
        if let [which @ ("selected" | "local"), rest @ ..] = words.as_slice() {
            let slot = if *which == "selected" { &mut self.export_selected } else { &mut self.export_local };
            *slot = match rest {
                [] => !*slot,
                ["on"] => true,
                ["off"] => false,
                _ => return format!("export {which} [on|off]"),
            };
            return format!("export {which} {}", if *slot { "on" } else { "off" });
        }
        if let [what @ ("materials" | "render" | "cameras" | "notes"), path @ ..] = words.as_slice() {
            if path.is_empty() {
                return format!("export {what} <file.txt>");
            }
            return self.export_component(what, &path.join(" "));
        }
        let (format, path) = match words.as_slice() {
            [] => return self.export_obj(None).unwrap_or_else(|| "export cancelled".to_owned()),
            [f, rest @ ..] if FORMATS.iter().any(|(n, ..)| n == f) => (f.to_string(), (!rest.is_empty()).then(|| rest.join(" "))),
            path => {
                let path = path.join(" ");
                let ext = std::path::Path::new(&path).extension().map(|e| e.to_string_lossy().to_ascii_lowercase());
                let format = match ext.as_deref() {
                    Some("ply") => "ply",
                    Some("xraw") => "xraw",
                    Some("qb") => "qb",
                    _ => "obj",
                };
                (format.to_owned(), Some(path))
            }
        };
        if format == "obj" {
            return self.export_obj(path.as_deref()).unwrap_or_else(|| "export cancelled".to_owned());
        }
        let ext = FORMATS.iter().find(|(n, ..)| *n == format).map_or("dat", |(_, e, _)| *e);
        let path = match path {
            Some(p) => p,
            None => match self.export_panel(&format, ext) {
                Some(p) => p,
                None => return "export cancelled".to_owned(),
            },
        };
        let bytes = match self.export_bytes(&format) {
            Ok(b) => b,
            Err(e) => return format!("error: {e}"),
        };
        match std::fs::write(&path, &bytes) {
            Ok(()) => format!("exported {format} to {path} ({} bytes)", bytes.len()),
            Err(e) => format!("error: could not write {path}: {e}"),
        }
    }

    fn export_bytes(&self, format: &str) -> Result<Vec<u8>, String> {
        let loaded = self.scene.as_ref().ok_or("nothing open")?;
        let exported = self.export_scene().ok_or("nothing open")?;
        let (scene, palette) = (exported.as_ref(), &loaded.doc.palette);
        let visible = self.visibility();
        let keep = |i: &voxel_core::Instance| visible.shows(i);
        let selected = || {
            let model = self.selected_model().ok_or("no model selected")?;
            scene.models.get(model).ok_or("no model selected")
        };
        Ok(match format {
            "ply" => formats::ply_mesh(scene, palette, keep, MESH_SCALE).into_bytes(),
            "mc" => voxel_core::marching::ply(scene, palette, keep, MESH_SCALE).into_bytes(),
            "cube" => formats::ply_cubes(scene, palette, keep, MESH_SCALE, CUBE_MARGIN).into_bytes(),
            "point" => formats::ply_points(scene, palette, keep, 1.0, glam::Vec3::ZERO).into_bytes(),
            "xraw" => formats::xraw(selected()?, palette),
            "qb" => {
                let name = std::path::Path::new(&loaded.name).file_stem().map_or("model".to_owned(), |s| s.to_string_lossy().into_owned());
                formats::qb(selected()?, palette, &name)
            }
            "slab" => formats::slab(selected()?, palette),
            "slice" => {
                let (w, h, rgba) = formats::slices(selected()?, palette);
                voxel_core::export::png_rgba(w, h, &rgba)
            }
            "iso" | "2d" => {
                let img = if format == "iso" {
                    voxel_core::sprites::iso(selected()?, palette, &Default::default())
                } else {
                    voxel_core::sprites::flat(selected()?, palette, &Default::default())
                };
                voxel_core::export::png_rgba(img.width, img.height, &img.rgba)
            }
            other => return Err(format!("unknown format {other}")),
        })
    }

    fn export_panel(&self, format: &str, ext: &str) -> Option<String> {
        let stem = self
            .scene
            .as_ref()
            .and_then(|s| std::path::Path::new(&s.name).file_stem().map(|n| n.to_string_lossy().into_owned()))
            .unwrap_or_else(|| "model".to_owned());
        let suffix = match format {
            "cube" => "-cubes",
            "mc" => "-mc",
            "point" => "-points",
            "slab" => "-slab6",
            "slice" => "-slices",
            "iso" => "-iso",
            "2d" => "-2d",
            _ => "",
        };
        rfd::FileDialog::new()
            .add_filter(format, &[ext])
            .set_file_name(format!("{stem}{suffix}.{ext}"))
            .save_file()
            .map(|p| p.to_string_lossy().into_owned())
    }

    /// `import materials|render|cameras|palette <file.vox>`: one part of
    /// another project, as one undo step (the reference editor's File tab imports
    /// these from its own component files; here from a `.vox`, which holds
    /// them all).
    fn import_component(&mut self, what: &str, path: &str) -> String {
        // A component file (the reference editor's text format) rather than a project.
        if path.to_ascii_lowercase().ends_with(".txt") && what != "palette" {
            let text = crate::read_text(std::path::Path::new(path)).map_err(|e| e.to_string());
            let command = match text.and_then(|t| self.component_command(what, &t)) {
                Ok(c) => c,
                Err(e) => return format!("error: {path}: {e}"),
            };
            let Some(loaded) = self.scene.as_mut() else { return "nothing open".to_owned() };
            self.history.end_gesture();
            let changed = self.history.exec_alone(&mut loaded.doc, command);
            self.mesh_dirty = true;
            return if changed { format!("imported the {what} of {path}") } else { format!("{path}: nothing new in its {what}") };
        }
        let other = match crate::scene::LoadedScene::load(path) {
            Ok(s) => s.doc,
            Err(e) => return format!("error: {path}: {e}"),
        };
        let Some(loaded) = self.scene.as_mut() else { return "nothing open".to_owned() };
        let doc = &loaded.doc;
        let command = match what {
            "materials" => voxel_core::Command::Batch(
                (1..=255u8)
                    .filter_map(|i| {
                        let (before, after) = (doc.material(i).cloned(), other.material(i).cloned());
                        (before != after).then_some(voxel_core::Command::SetMaterial { index: i, before, after })
                    })
                    .collect(),
            ),
            "palette" => voxel_core::Command::Batch(
                (1..=255u8)
                    .filter_map(|i| {
                        let (before, after) = (doc.palette.color(i), other.palette.color(i));
                        (before != after).then_some(voxel_core::Command::SetColor { index: i, before, after })
                    })
                    .collect(),
            ),
            "render" => voxel_core::Command::SetRender { before: doc.render.clone(), after: other.render.clone() },
            "cameras" => voxel_core::Command::SetCameras { before: doc.cameras.clone(), after: other.cameras.clone() },
            "notes" => voxel_core::Command::SetNotes { before: doc.notes.clone(), after: other.notes.clone() },
            _ => return "import materials|render|cameras|palette|notes <file.vox>".to_owned(),
        };
        self.history.end_gesture();
        let changed = self.history.exec_alone(&mut loaded.doc, command);
        // Through `refresh_document`, which resyncs colours and render settings.
        self.mesh_dirty = true;
        if changed { format!("imported the {what} of {path}") } else { format!("{path}: nothing new in its {what}") }
    }

    pub fn import_command(&mut self, line: &str) -> String {
        let words: Vec<&str> = line.split_whitespace().collect();
        // the reference editor's Import Sprites: the file's models imported, then
        // packed into one looping animation, a frame each.
        if words.get(1) == Some(&"sprites") {
            let path = words[2..].join(" ");
            if path.is_empty() {
                return "import sprites <file>".to_owned();
            }
            let before = self.scene.as_ref().map_or(0, |s| s.doc.scene.instances.len());
            let imported = self.import_command(&format!("import {path}"));
            let after = self.scene.as_ref().map_or(0, |s| s.doc.scene.instances.len());
            let indices: Vec<usize> = (before..after).collect();
            if indices.len() < 2 {
                return imported;
            }
            let packed = self.scene.as_ref().map(|s| s.doc.scene.with_packed(&indices, false, true));
            return match packed.map(|nodes| self.exec_graph(nodes, false)) {
                Some(Ok(())) => {
                    self.forget_picks();
                    format!("{imported}; packed as a looping animation of {} frames", indices.len())
                }
                Some(Err(e)) => format!("{imported}; error: {e}"),
                None => imported,
            };
        }
        if let Some(what) = words.get(1).filter(|w| matches!(**w, "materials" | "render" | "cameras" | "palette" | "notes")) {
            let path = words[2..].join(" ");
            if path.is_empty() {
                return format!("import {what} <file.vox>");
            }
            return self.import_component(what, &path);
        }
        // `import <file.obj> [size] [fill]`: a mesh, voxelised.
        let mut rest: Vec<&str> = line.split_whitespace().skip(1).collect();
        let mut obj_fill = false;
        let mut obj_size = 64;
        while let Some(last) = rest.last() {
            if *last == "fill" {
                obj_fill = true;
            } else if let Ok(n) = last.parse::<u32>() {
                obj_size = n.clamp(1, 256);
            } else {
                break;
            }
            rest.pop();
        }
        let path = rest.join(" ");
        let path = if path.is_empty() {
            match rfd::FileDialog::new()
                .add_filter("Models", &["vox", "xraw", "qb", "obj"])
                .pick_file()
            {
                Some(p) => p.to_string_lossy().into_owned(),
                None => return "import cancelled".to_owned(),
            }
        } else {
            path
        };
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) => return format!("error: {path}: {e}"),
        };
        let Some(palette) = self.scene.as_ref().map(|s| s.doc.palette.clone()) else {
            return "nothing open to import into".to_owned();
        };
        if path.to_ascii_lowercase().ends_with(".obj") {
            let text = String::from_utf8_lossy(&bytes);
            // The material library sits beside the OBJ.
            let beside = |name: &str| std::path::Path::new(&path).with_file_name(name);
            let mut materials = voxel_core::voxelize::mtllib(&text)
                .and_then(|m| std::fs::read_to_string(beside(&m)).ok())
                .map(|m| voxel_core::voxelize::parse_mtl(&m))
                .unwrap_or_default();
            // Textures, decoded here: the core has no image reader.
            for m in materials.values_mut() {
                m.texture = m.texture_name.as_ref().and_then(|t| std::fs::read(beside(t)).ok()).and_then(|b| crate::palette_files::read_rgba(&b));
            }
            let volume = match voxel_core::voxelize::parse_obj(&text, &materials) {
                Ok(mesh) => voxel_core::voxelize::voxelize(&mesh, &palette, obj_size, obj_fill),
                Err(e) => return format!("error: {path}: {e}"),
            };
            let [x, y, z] = volume.size();
            let voxels = volume.filled_count();
            return match self.add_model(volume, None) {
                Ok(_) => {
                    self.mesh_dirty = true;
                    format!("imported {path} as {x}x{y}x{z}, {voxels} voxels")
                }
                Err(e) => e,
            };
        }
        // Each model and where its corner goes (None: beside the scene).
        let models: Vec<(voxel_core::Volume, Option<glam::IVec3>)> = match read_any(&bytes, &path) {
            Ok(Read::Vox(scene)) => {
                // Placed as in that file: its instances' corners, unturned.
                scene
                    .doc
                    .scene
                    .instances
                    .iter()
                    .filter_map(|i| {
                        let v = scene.doc.scene.models.get(i.model_index)?;
                        let corner = i.transform.translation - v.pivot().as_ivec3();
                        let colours = formats::Imported {
                            name: String::new(),
                            size: v.size(),
                            position: corner,
                            cells: formats::Cells::Indexed(indices(v), Box::new(*scene.doc.palette.as_slice())),
                        };
                        Some((colours.to_volume(&palette), Some(corner)))
                    })
                    .collect()
            }
            // Qubicle's matrices keep their places relative to each other; a
            // single model goes beside the scene.
            Ok(Read::Models(list)) => {
                let several = list.len() > 1;
                list.iter().map(|m| (m.to_volume(&palette), several.then_some(m.position))).collect()
            }
            Err(e) => return format!("error: {path}: {e}"),
        };
        if models.is_empty() {
            return format!("{path}: no models");
        }
        // Models that keep their places relative to each other move together
        // to beside the scene, so the import doesn't land inside it.
        let models = beside_scene(models, self.scene.as_ref().and_then(|s| s.doc.scene.declared_bounds()));
        self.history.begin_group();
        let mut added = 0;
        for (volume, corner) in models {
            if self.add_model(volume, corner).is_ok() {
                added += 1;
            }
        }
        self.history.end_group();
        self.mesh_dirty = true;
        format!("imported {added} model(s) from {path}")
    }
}

enum Read {
    Vox(Box<crate::scene::LoadedScene>),
    Models(Vec<Imported>),
}

/// Tells the formats apart by content.
fn read_any(bytes: &[u8], path: &str) -> Result<Read, String> {
    if bytes.starts_with(b"VOX ") {
        return crate::scene::LoadedScene::load(path).map(|s| Read::Vox(Box::new(s)));
    }
    if bytes.starts_with(b"XRAW") {
        return formats::read_xraw(bytes).map(|m| Read::Models(vec![m]));
    }
    if formats::looks_like_slab(bytes) {
        return formats::read_slab(bytes).map(|m| Read::Models(vec![m]));
    }
    if bytes.len() > 24 && bytes[..4] == [1, 1, 0, 0] {
        return formats::read_qb(bytes).map(Read::Models);
    }
    Err("not a format this editor reads (vox, xraw, qb, SLAB6)".to_owned())
}

fn beside_scene(
    models: Vec<(voxel_core::Volume, Option<glam::IVec3>)>,
    bounds: Option<(glam::Vec3, glam::Vec3)>,
) -> Vec<(voxel_core::Volume, Option<glam::IVec3>)> {
    let Some(min_x) = models.iter().filter_map(|(_, c)| c.map(|c| c.x)).min() else { return models };
    let right = bounds.map_or(0, |(_, max)| max.x.ceil() as i32);
    let shift = glam::IVec3::new(right + 2 - min_x, 0, 0);
    models.into_iter().map(|(v, c)| (v, c.map(|c| c + shift))).collect()
}

/// A model's indices, x fastest, then y, then z.
fn indices(v: &voxel_core::Volume) -> Vec<u8> {
    let [sx, sy, sz] = v.size();
    let mut out = Vec::with_capacity((sx * sy * sz) as usize);
    for z in 0..sz {
        for y in 0..sy {
            for x in 0..sx {
                out.push(v.get(x, y, z));
            }
        }
    }
    out
}
