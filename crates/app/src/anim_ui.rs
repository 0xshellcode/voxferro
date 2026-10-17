//! Animation: choosing the frame, playing, keying, the timeline, and
//! exporting the frames as images.
//!
//! The scene shows one frame at a time (`Scene::frame`); edits made there
//! land on that frame. Moving a model at a frame keys its transform there;
//! *New frame* gives the model a copy of its current shape from this frame
//! on, to edit frame by frame.

use crate::App;
use eframe::{egui, egui_wgpu};
use voxel_core::Command;

/// A key being dragged along the timeline.
pub struct KeyDrag {
    node: i32,
    /// A shape frame (blue) rather than a transform key (yellow).
    shape: bool,
    /// The frame it is on now.
    frame: u32,
}

/// Rendering every frame of the range to numbered PNG files.
pub struct ExportJob {
    dir: std::path::PathBuf,
    stem: String,
    /// The frame being rendered.
    frame: u32,
    last: u32,
    samples: u32,
    written: usize,
}

impl App {
    /// Shows `frame`: rebuilds the placements and the meshes for it.
    pub fn set_frame(&mut self, frame: u32) {
        let Some(loaded) = self.scene.as_mut() else { return };
        if loaded.doc.scene.frame == frame {
            return;
        }
        loaded.doc.scene.frame = frame;
        loaded.doc.scene.rebuild();
        self.mesh_dirty = true;
        self.path_tracer.reset();
    }

    /// The frames stepping wraps within: the range playback uses.
    pub fn anim_bounds(&self) -> (u32, u32) {
        self.scene.as_ref().map_or((0, 0), |s| s.anim_range())
    }

    pub fn frame(&self) -> u32 {
        self.scene.as_ref().map_or(0, |s| s.doc.scene.frame)
    }

    /// Advances playback by the time since the last frame.
    pub fn animate(&mut self, ctx: &egui::Context) {
        if !self.playing {
            return;
        }
        let Some((first, last)) = self.scene.as_ref().map(|s| s.anim_range()) else { return };
        self.play_clock += ctx.input(|i| i.stable_dt) as f64;
        let step = 1.0 / self.fps.max(1.0) as f64;
        let mut frame = self.frame();
        while self.play_clock >= step {
            self.play_clock -= step;
            frame = if frame >= last || frame < first { first } else { frame + 1 };
        }
        self.set_frame(frame);
        ctx.request_repaint();
    }

    /// `anim play | stop | frame <n> | fps <n> | key | unkey | newframe`
    pub fn anim_command(&mut self, line: &str) -> String {
        let words: Vec<&str> = line.split_whitespace().skip(1).collect();
        match words.as_slice() {
            ["play"] => {
                self.playing = true;
                "playing".to_owned()
            }
            ["stop"] => {
                self.playing = false;
                "stopped".to_owned()
            }
            ["frame", n] => match n.parse::<u32>() {
                Ok(n) => {
                    self.set_frame(n);
                    format!("frame {n}")
                }
                Err(_) => "anim frame <n>".to_owned(),
            },
            ["fps", n] => match n.parse::<f32>() {
                Ok(n) if n > 0.0 => {
                    self.fps = n;
                    format!("{n} frames per second")
                }
                _ => "anim fps <n>".to_owned(),
            },
            ["key"] => {
                // Writing the current placement back at this frame keys it.
                let Some(loaded) = &self.scene else { return "nothing open".to_owned() };
                let scene = &loaded.doc.scene;
                let nodes = match self.selected_group {
                    Some(group) => scene.with_node_moved(group, glam::IVec3::ZERO),
                    None => match scene.instances.get(self.selected) {
                        Some(i) => scene.with_instance_placed(self.selected, i.transform),
                        None => return "nothing selected".to_owned(),
                    },
                };
                match self.exec_graph(nodes, false) {
                    Ok(()) => format!("keyed at frame {}", self.frame()),
                    Err(e) => format!("error: {e}"),
                }
            }
            ["unkey"] => {
                let target = self.selected_group.or_else(|| {
                    let loaded = self.scene.as_ref()?;
                    loaded.doc.scene.instances.get(self.selected)?.node
                });
                let Some(node) = target else { return "nothing selected".to_owned() };
                let Some(loaded) = &self.scene else { return "nothing open".to_owned() };
                let nodes = loaded.doc.scene.with_key_removed(node);
                match self.exec_graph(nodes, false) {
                    Ok(()) => format!("key at frame {} removed", self.frame()),
                    Err(e) => format!("error: {e}"),
                }
            }
            ["newframe"] => self.new_shape_frame(),
            // the reference editor a recent version's stepping, loop and reverse (`astep`,
            // `aset loop`, `arev`), on the selected objects' keys.
            ["step", n] => match n.trim_start_matches('+').parse::<i64>() {
                Ok(n) => {
                    // Wraps within the range, as `astep time` does.
                    let (first, last) = self.anim_bounds();
                    let len = (last - first + 1) as i64;
                    let f = first as i64 + (self.frame() as i64 - first as i64 + n).rem_euclid(len);
                    self.set_frame(f as u32);
                    format!("frame {f}")
                }
                Err(_) => "anim step <frames>".to_owned(),
            },
            ["key", which @ ("next" | "prev" | "first" | "last")] => {
                let Some(node) = self.target_node() else { return "nothing selected".to_owned() };
                let Some(loaded) = &self.scene else { return "nothing open".to_owned() };
                let keys = loaded.doc.scene.keys_of(node);
                let (first, last) = self.anim_bounds();
                let now = self.frame();
                let to = match *which {
                    "next" => keys.iter().copied().find(|&k| k > now).unwrap_or(last),
                    "prev" => keys.iter().rev().copied().find(|&k| k < now).unwrap_or(first),
                    "first" => keys.first().copied().unwrap_or(first),
                    _ => keys.last().copied().unwrap_or(last),
                };
                self.set_frame(to);
                format!("frame {to}")
            }
            ["loop", on @ ("on" | "off")] => {
                let nodes = if self.objects.is_empty() { self.target_node().into_iter().collect() } else { self.object_nodes() };
                self.history.begin_group();
                let mut done = 0;
                for node in nodes {
                    let Some(loaded) = &self.scene else { break };
                    let graph = loaded.doc.scene.with_loop(node, *on == "on");
                    if self.exec_graph(graph, false).is_ok() {
                        done += 1;
                    }
                }
                self.history.end_group();
                format!("loop {on} on {done} objects")
            }
            ["reverse"] => {
                let Some(node) = self.target_node() else { return "nothing selected".to_owned() };
                let Some(loaded) = &self.scene else { return "nothing open".to_owned() };
                let graph = loaded.doc.scene.with_keys_reversed(node);
                match self.exec_graph(graph, false) {
                    Ok(()) => "keys reversed".to_owned(),
                    Err(_) => "reverse: the selection has no keys".to_owned(),
                }
            }
            [verb @ ("movekey" | "moveframe"), from, to] => {
                let (Ok(from), Ok(to)) = (from.parse::<u32>(), to.parse::<u32>()) else {
                    return format!("anim {verb} <from> <to>");
                };
                let Some(node) = self.target_node() else { return "nothing selected".to_owned() };
                let Some(loaded) = &self.scene else { return "nothing open".to_owned() };
                let nodes = loaded.doc.scene.with_key_moved(node, from, to, *verb == "moveframe");
                match self.exec_graph(nodes, false) {
                    Ok(()) => format!("moved from frame {from} to {to}"),
                    Err(e) => format!("error: {e}"),
                }
            }
            ["range", a, b] => match (a.parse::<u32>(), b.parse::<u32>()) {
                (Ok(a), Ok(b)) if b >= a => {
                    self.set_anim_range(a, b, false);
                    format!("range {a} to {b}")
                }
                _ => "anim range <first> <last>, last not before first".to_owned(),
            },
            ["export", dir, rest @ ..] => {
                let samples = rest.first().and_then(|s| s.parse::<u32>().ok()).unwrap_or(self.export_samples);
                self.start_export(std::path::PathBuf::from(dir), samples)
            }
            _ => "anim play | stop | frame <n> | fps <n> | key | unkey | newframe | movekey <a> <b> | moveframe <a> <b> | range <a> <b> | export <dir> [samples]"
                .to_owned(),
        }
    }

    /// Saves the range in the file's `META`, as one undo step (`merge` for
    /// a dragged field).
    fn set_anim_range(&mut self, first: u32, last: u32, merge: bool) {
        let Some(loaded) = self.scene.as_mut() else { return };
        let command = Command::SetMeta { before: loaded.doc.meta.clone(), after: loaded.doc.with_anim_range(first, last) };
        if merge {
            self.history.exec(&mut loaded.doc, command);
        } else {
            self.history.exec_alone(&mut loaded.doc, command);
        }
    }

    /// Starts rendering the range, `samples` path-traced samples a frame.
    pub fn start_export(&mut self, dir: std::path::PathBuf, samples: u32) -> String {
        let Some(loaded) = &self.scene else { return "nothing open".to_owned() };
        if let Err(e) = std::fs::create_dir_all(&dir) {
            return format!("error: cannot use {}: {e}", dir.display());
        }
        let (first, last) = loaded.anim_range();
        let stem = std::path::Path::new(&loaded.name)
            .file_stem()
            .map_or("frame".to_owned(), |s| s.to_string_lossy().into_owned());
        self.playing = false;
        self.rendering = true;
        self.set_frame(first);
        self.path_tracer.reset();
        let count = last - first + 1;
        self.export = Some(ExportJob { dir: dir.clone(), stem, frame: first, last, samples: samples.max(1), written: 0 });
        format!("exporting {count} frames to {} at {samples} samples", dir.display())
    }

    /// Writes the current frame once it has its samples, and moves on.
    pub fn export_step(&mut self, ctx: &egui::Context, state: &egui_wgpu::RenderState) {
        let Some(job) = &self.export else { return };
        ctx.request_repaint();
        if self.path_tracer.samples < job.samples {
            return;
        }
        let path = job.dir.join(format!("{}_{:04}.png", job.stem, job.frame));
        let written = match self.path_tracer.read_frame(&state.device, &state.queue) {
            Some((w, h, rgba)) => std::fs::write(&path, voxel_core::export::png_rgba(w, h, &rgba)).map_err(|e| e.to_string()),
            None => Err("nothing rendered".to_owned()),
        };
        let Some(job) = &mut self.export else { return };
        if let Err(e) = written {
            let message = format!("export stopped at frame {}: {e}", job.frame);
            self.export = None;
            self.console_log.push(message);
            return;
        }
        job.written += 1;
        if job.frame >= job.last {
            let message = format!("exported {} frames to {}", job.written, job.dir.display());
            eprintln!("[voxferro] {message}");
            self.export = None;
            self.console_log.push(message);
        } else {
            job.frame += 1;
            let next = job.frame;
            self.set_frame(next);
            self.path_tracer.reset();
        }
    }

    /// The strip of frames: the range, where the selected model or group is
    /// keyed (yellow) and where its shape changes model (blue), and the
    /// current frame. Click or drag on it to scrub.
    fn timeline(&mut self, ui: &mut egui::Ui, first: u32, last: u32) {
        let width = ui.available_width().max(60.0);
        let (rect, response) = ui.allocate_exact_size(egui::vec2(width, 26.0), egui::Sense::click_and_drag());
        let span = (last - first).max(1) as f32;
        let x_of = |f: u32| rect.left() + 4.0 + (f.saturating_sub(first) as f32 / span) * (rect.width() - 8.0);
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 3.0, egui::Color32::from_gray(32));

        // Frame ticks; every fifth one taller.
        let step = if last - first > 60 { 5 } else { 1 };
        for f in (first..=last).step_by(step as usize) {
            let tall = f % 5 == 0;
            let x = x_of(f);
            let top = if tall { rect.top() + 4.0 } else { rect.top() + 10.0 };
            painter.line_segment([egui::pos2(x, top), egui::pos2(x, rect.bottom() - 4.0)], egui::Stroke::new(1.0, egui::Color32::from_gray(70)));
        }

        // Keys of the selected node, and the frames where its shape (if it
        // has one) changes model.
        let node = self.target_node();
        let target = self.scene.as_ref().and_then(|loaded| {
            let nodes = &loaded.doc.scene.nodes;
            match nodes.iter().find(|n| Some(n.id()) == node)? {
                vox_format::SceneNode::Transform(t) => {
                    let keys: Vec<u32> = voxel_core::anim::keyframes(t).iter().map(|k| k.0).collect();
                    let shape_frames: Vec<u32> = match nodes.iter().find(|n| n.id() == t.child) {
                        Some(vox_format::SceneNode::Shape(s)) => s
                            .models
                            .iter()
                            .map(|m| m.attributes.get("_f").and_then(|v| v.parse().ok()).unwrap_or(0))
                            .collect(),
                        _ => Vec::new(),
                    };
                    Some((keys, shape_frames))
                }
                _ => None,
            }
        });
        let markers = target.clone();
        if let Some((keys, shape_frames)) = target {
            if keys.len() > 1 {
                for f in keys {
                    let c = egui::pos2(x_of(f), rect.top() + 9.0);
                    painter.rect_filled(egui::Rect::from_center_size(c, egui::vec2(6.0, 6.0)), 1.0, egui::Color32::from_rgb(255, 210, 90));
                }
            }
            if shape_frames.len() > 1 {
                for f in shape_frames {
                    painter.circle_filled(egui::pos2(x_of(f), rect.bottom() - 7.0), 3.0, egui::Color32::from_rgb(110, 170, 255));
                }
            }
        }

        let now = x_of(self.frame());
        painter.line_segment([egui::pos2(now, rect.top()), egui::pos2(now, rect.bottom())], egui::Stroke::new(2.0, egui::Color32::WHITE));

        let frame_at = |x: f32| {
            let t = ((x - rect.left() - 4.0) / (rect.width() - 8.0)).clamp(0.0, 1.0);
            first + (t * span).round() as u32
        };
        // Pressing on a marker grabs it: keys along the top half, shape
        // frames along the bottom. Anywhere else scrubs.
        if response.drag_started() {
            // Where the button went down, not where the drag was recognised:
            // by then the pointer has moved past the marker's few pixels.
            let pressed_at = ui.input(|i| i.pointer.press_origin());
            if let (Some(pos), Some((keys, shape_frames)), Some(node)) = (pressed_at, &markers, node) {
                let shape = pos.y > rect.center().y;
                let candidates = if shape { shape_frames } else { keys };
                let grabbed = candidates
                    .iter()
                    .copied()
                    .filter(|_| candidates.len() > 1)
                    .min_by_key(|f| (x_of(*f) - pos.x).abs() as i32)
                    .filter(|f| (x_of(*f) - pos.x).abs() <= 6.0);
                if let Some(frame) = grabbed {
                    self.history.end_gesture();
                    self.key_drag = Some(KeyDrag { node, shape, frame });
                }
            }
        }
        if let Some(pos) = response.interact_pointer_pos() {
            if let Some(drag) = &self.key_drag {
                if response.dragged() {
                    let (node, shape, from) = (drag.node, drag.shape, drag.frame);
                    let to = frame_at(pos.x);
                    let moved = self.scene.as_ref().map(|s| s.doc.scene.with_key_moved(node, from, to, shape));
                    // Onto a taken frame it simply waits; past it, it moves.
                    if let Some(Ok(nodes)) = moved
                        && self.exec_graph(Ok(nodes), true).is_ok()
                    {
                        if let Some(drag) = &mut self.key_drag {
                            drag.frame = to;
                        }
                        self.set_frame(to);
                    }
                }
            } else if response.clicked() || response.dragged() {
                self.set_frame(frame_at(pos.x));
            }
        }
        if response.drag_stopped() {
            self.key_drag = None;
        }
    }

    /// Gives the selected model a copy of its current shape, shown from the
    /// current frame on: one undo step.
    fn new_shape_frame(&mut self) -> String {
        let frame = self.frame();
        let Some(loaded) = self.scene.as_mut() else { return "nothing open".to_owned() };
        let scene = &loaded.doc.scene;
        let Some(instance) = scene.instances.get(self.selected) else { return "nothing selected".to_owned() };
        let copy = scene.models[instance.model_index].clone();
        let new_model = scene.models.len();
        let nodes = match scene.with_shape_frame(self.selected, new_model, frame) {
            Ok(n) => n,
            Err(e) => return format!("error: {e}"),
        };
        let command = Command::Batch(vec![
            Command::PushModel { volume: copy },
            Command::SetGraph { before: scene.nodes.clone(), after: nodes },
        ]);
        self.history.exec_alone(&mut loaded.doc, command);
        self.mesh_dirty = true;
        format!("model {new_model} shown from frame {frame}")
    }

    pub fn animation_ui(&mut self, ui: &mut egui::Ui) {
        let Some((first, last)) = self.scene.as_ref().map(|s| s.anim_range()) else { return };
        let mut lines: Vec<String> = Vec::new();
        let mut frame = self.frame();
        ui.horizontal(|ui| {
            let play = if self.playing { "Pause" } else { "Play" };
            if ui.small_button(play).clicked() {
                lines.push(if self.playing { "anim stop" } else { "anim play" }.to_owned());
            }
            if ui.small_button("<").clicked() && frame > first {
                frame -= 1;
            }
            if ui.small_button(">").clicked() {
                frame += 1;
            }
            ui.add(egui::DragValue::new(&mut self.fps).range(1.0..=60.0).suffix(" fps").speed(0.2));
        });
        if frame != self.frame() {
            self.set_frame(frame);
        }
        self.timeline(ui, first, last);
        ui.horizontal(|ui| {
            ui.label(format!("frame {}", self.frame()));
            ui.label("range");
            let (mut a, mut b) = (first, last);
            let ca = ui.add(egui::DragValue::new(&mut a).range(0..=b)).changed();
            let cb = ui.add(egui::DragValue::new(&mut b).range(a..=9999)).changed();
            if ca || cb {
                // Merged until the drag ends.
                self.set_anim_range(a, b, true);
            }
        });
        ui.horizontal(|ui| {
            for (label, line, tip) in [
                ("Key", "anim key", "Key the selected model or group's placement at this frame"),
                ("Unkey", "anim unkey", "Remove its key at this frame"),
                ("New frame", "anim newframe", "A copy of the model, shown from this frame on"),
            ] {
                if ui.small_button(label).on_hover_text(tip).clicked() {
                    lines.push(line.to_owned());
                }
            }
        });
        ui.horizontal(|ui| {
            match &self.export {
                Some(job) => {
                    ui.label(format!("exporting frame {} of {}…", job.frame, job.last));
                    if ui.small_button("Stop").clicked() {
                        self.export = None;
                    }
                }
                None => {
                    if ui.small_button("Export frames…").on_hover_text("Path trace every frame of the range to PNG files").clicked()
                        && let Some(dir) = rfd::FileDialog::new().pick_folder()
                    {
                        lines.push(format!("anim export {} {}", dir.display(), self.export_samples));
                    }
                    ui.add(egui::DragValue::new(&mut self.export_samples).range(1..=4096).suffix(" samples"));
                }
            }
        });
        for line in lines {
            let message = self.anim_command(&line);
            self.console_log.push(message);
        }
    }
}
