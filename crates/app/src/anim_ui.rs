//! Animation: choosing the frame, playing, and keying.
//!
//! The scene shows one frame at a time (`Scene::frame`); edits made there
//! land on that frame. Moving a model at a frame keys its transform there;
//! *New frame* gives the model a copy of its current shape from this frame
//! on, to edit frame by frame.

use crate::App;
use eframe::egui;
use voxel_core::Command;

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
            _ => "anim play | stop | frame <n> | fps <n> | key | unkey | newframe".to_owned(),
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
        ui.add(egui::Slider::new(&mut frame, first..=last.max(first + 1)).text("frame"));
        if frame != self.frame() {
            self.set_frame(frame);
        }
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
        for line in lines {
            let message = self.anim_command(&line);
            self.console_log.push(message);
        }
    }
}
