//! The editor's side of control by MCP (see `mcp`): running what agents
//! ask between frames, asking the user before an agent writes over a file,
//! and the panel that switches the server on.

use crate::mcp::{self, Op, Reply};
use crate::{App, image_out};
use eframe::{egui, egui_wgpu};
use tokio::sync::oneshot::Sender;

/// A write an agent asked for, waiting for the user.
pub struct PendingWrite {
    line: String,
    path: std::path::PathBuf,
    reply: Sender<Reply>,
}

/// A capture waiting for the frame (or the samples) it needs.
pub struct PendingCapture {
    render: bool,
    samples: u32,
    max_size: u32,
    reply: Sender<Reply>,
}

impl App {
    pub fn start_mcp(&mut self, ctx: &egui::Context) {
        match mcp::Server::start(self.mcp_port, ctx.clone()) {
            Ok(server) => {
                let message = format!("mcp: listening on {}", server.url());
                eprintln!("[voxferro] {message}");
                self.console_log.push(message);
                self.mcp = Some(server);
            }
            Err(e) => {
                let message = format!("error: mcp could not listen on 127.0.0.1:{}: {e}", self.mcp_port);
                eprintln!("[voxferro] {message}");
                self.console_log.push(message);
            }
        }
    }

    pub fn stop_mcp(&mut self) {
        if self.mcp.take().is_some() {
            self.console_log.push("mcp: stopped".to_owned());
        }
        // Whoever was waiting gets an answer rather than a timeout.
        if let Some(w) = self.mcp_write.take() {
            let _ = w.reply.send(Reply::Error("the MCP server was switched off".to_owned()));
        }
        if let Some(c) = self.mcp_capture.take() {
            let _ = c.reply.send(Reply::Error("the MCP server was switched off".to_owned()));
        }
    }

    /// Runs the requests that arrived since the last frame.
    pub fn mcp_poll(&mut self, ctx: &egui::Context, state: &egui_wgpu::RenderState) {
        let Some(server) = &self.mcp else { return };
        let requests: Vec<mcp::Request> = server.requests.try_iter().collect();
        for request in requests {
            match request.op {
                Op::Command(line) => self.mcp_command(line, request.reply, ctx, state),
                Op::Describe => {
                    let _ = request.reply.send(Reply::Text(self.describe()));
                }
                Op::Capture { render, samples, max_size } => {
                    if let Some(old) = self.mcp_capture.take() {
                        let _ = old.reply.send(Reply::Error("replaced by a newer capture".to_owned()));
                    }
                    if render != self.rendering {
                        self.rendering = render;
                        self.path_tracer.reset();
                    }
                    self.mcp_capture = Some(PendingCapture { render, samples, max_size, reply: request.reply });
                    ctx.request_repaint();
                }
            }
        }
    }

    fn mcp_command(&mut self, line: String, reply: Sender<Reply>, ctx: &egui::Context, state: &egui_wgpu::RenderState) {
        if mcp::needs_path(&line) {
            let _ = reply.send(Reply::Error("give a path: a file panel would wait for someone at the screen".to_owned()));
            return;
        }
        let current = self.scene.as_ref().and_then(|s| s.path.clone());
        match mcp::write_target(&line, current.as_deref()) {
            Some(Err(e)) => {
                let _ = reply.send(Reply::Error(e));
            }
            Some(Ok(path)) if path.exists() => {
                if self.mcp_write.is_some() {
                    let _ = reply.send(Reply::Error("another write is waiting for the user".to_owned()));
                } else {
                    self.console_log.push(format!("mcp: asking before writing {}", path.display()));
                    self.mcp_write = Some(PendingWrite { line, path, reply });
                }
            }
            _ => {
                let out = self.mcp_run(&line, ctx, state);
                let _ = reply.send(out);
            }
        }
    }

    /// Runs a console line and returns what it logged.
    fn mcp_run(&mut self, line: &str, ctx: &egui::Context, state: &egui_wgpu::RenderState) -> Reply {
        let before = self.console_log.len();
        self.run_command(line, ctx, state);
        // Skip the "> line" echo.
        let lines: Vec<&str> = self.console_log.iter().skip(before + 1).map(String::as_str).collect();
        let text = if lines.is_empty() { "done".to_owned() } else { lines.join("\n") };
        if lines.iter().any(|l| l.starts_with("error") || l.starts_with("unknown command")) {
            Reply::Error(text)
        } else {
            Reply::Text(text)
        }
    }

    /// Asks the user whether an agent may write over a file.
    pub fn mcp_confirm_ui(&mut self, ctx: &egui::Context, state: &egui_wgpu::RenderState) {
        let Some(pending) = &self.mcp_write else { return };
        let mut choice = None;
        egui::Modal::new(egui::Id::new("mcp write")).show(ctx, |ui| {
            ui.heading("An agent wants to write a file");
            ui.label(format!("{} already exists and would be replaced.", pending.path.display()));
            ui.monospace(&pending.line);
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Allow").clicked() {
                    choice = Some(true);
                }
                if ui.button("Deny").clicked() {
                    choice = Some(false);
                }
            });
        });
        let Some(allow) = choice else { return };
        let Some(pending) = self.mcp_write.take() else { return };
        let reply = if allow {
            self.mcp_run(&pending.line, ctx, state)
        } else {
            self.console_log.push(format!("mcp: write to {} denied", pending.path.display()));
            Reply::Error(format!("the user did not allow writing {}", pending.path.display()))
        };
        let _ = pending.reply.send(reply);
    }

    /// Answers a pending capture once its image is ready: after this
    /// frame's viewport for the editor view, after enough samples for a
    /// render.
    pub fn mcp_finish_capture(&mut self, ctx: &egui::Context, state: &egui_wgpu::RenderState) {
        let Some(capture) = &self.mcp_capture else { return };
        if capture.render && self.path_tracer.samples < capture.samples {
            ctx.request_repaint();
            return;
        }
        let Some(capture) = self.mcp_capture.take() else { return };
        let frame = if capture.render {
            self.path_tracer.read_frame(&state.device, &state.queue)
        } else {
            self.scene_renderer.read_frame(&state.device, &state.queue)
        };
        let reply = match frame {
            Some((w, h, rgba)) => {
                let (w, h, rgba) = image_out::fit(w, h, &rgba, capture.max_size);
                self.console_log.push(format!("mcp: captured {w}x{h}"));
                Reply::Png(voxel_core::export::png_rgba(w, h, &rgba))
            }
            None => Reply::Error("nothing rendered yet".to_owned()),
        };
        let _ = capture.reply.send(reply);
    }

    /// The document as JSON, for `describe_scene`.
    pub fn describe(&self) -> String {
        let Some(loaded) = &self.scene else {
            return serde_json::json!({ "document": null }).to_string();
        };
        let doc = &loaded.doc;
        let mut colours = [0u64; 256];
        let models: Vec<_> = doc
            .scene
            .models
            .iter()
            .enumerate()
            .map(|(i, v)| {
                let h = v.histogram();
                for (c, n) in h.iter().enumerate().skip(1) {
                    colours[c] += n;
                }
                let filled: u64 = h[1..].iter().sum();
                serde_json::json!({ "index": i, "size": v.size(), "voxels": filled })
            })
            .collect();
        let instances: Vec<_> = doc
            .scene
            .instances
            .iter()
            .enumerate()
            .map(|(i, inst)| {
                serde_json::json!({
                    "index": i,
                    "model": inst.model_index,
                    "position": inst.transform.translation.to_array(),
                    "layer": inst.layer,
                    "hidden": inst.hidden,
                })
            })
            .collect();
        let layers: Vec<_> = doc
            .layers
            .iter()
            .map(|l| serde_json::json!({ "id": l.id, "name": l.name(), "hidden": l.is_hidden() }))
            .collect();
        let used: Vec<_> = colours
            .iter()
            .enumerate()
            .filter(|(c, n)| *c > 0 && **n > 0)
            .map(|(c, n)| {
                let k = doc.palette.color(c as u8);
                serde_json::json!({ "index": c, "hex": format!("#{:02x}{:02x}{:02x}", k.r, k.g, k.b), "voxels": n })
            })
            .collect();
        serde_json::json!({
            "file": loaded.name,
            "path": loaded.path,
            "unsaved_changes": self.is_dirty(),
            "models": models,
            "instances": instances,
            "selected_instance": self.selected,
            "selected_model": self.selected_model(),
            "layers": layers,
            "colours_in_use": used,
            "brush_colour": self.brush_color,
            "animation_frame": doc.scene.frame,
            "rendering": self.rendering,
            "undo_steps": self.history.undo_depth(),
            "axes": "z is up; voxel coordinates are each model's own cells",
        })
        .to_string()
    }

    /// The panel section that switches the server on and off.
    pub fn mcp_panel(&mut self, ui: &mut egui::Ui) {
        egui::CollapsingHeader::new("Agents (MCP)").show(ui, |ui| {
            let mut on = self.mcp.is_some();
            if ui
                .checkbox(&mut on, "Let agents use the editor")
                .on_hover_text("An MCP server on this computer only (127.0.0.1)")
                .changed()
            {
                if on {
                    self.start_mcp(ui.ctx());
                } else {
                    self.stop_mcp();
                }
            }
            match &self.mcp {
                Some(server) => {
                    ui.label("Add it to Claude Code with:");
                    let line = format!("claude mcp add --transport http voxferro {}", server.url());
                    ui.add(egui::Label::new(egui::RichText::new(&line).monospace().small()).wrap());
                    if ui.small_button("Copy").clicked() {
                        ui.ctx().copy_text(line);
                    }
                }
                None => {
                    ui.horizontal(|ui| {
                        ui.label("port");
                        ui.add(egui::DragValue::new(&mut self.mcp_port).range(1024..=65535));
                    });
                }
            }
        });
    }
}
