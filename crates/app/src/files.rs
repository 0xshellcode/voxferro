//! Opening, creating and saving files, and not losing unsaved work.
//!
//! Anything that would replace the open document (opening another file,
//! starting a new one, quitting) goes through [`App::request`]. With unsaved
//! changes it asks first, the way every document editor does; without them it
//! just happens.

use crate::{App, scene};
use eframe::{egui, egui_wgpu};

/// An action that discards the open document.
#[derive(Debug, Clone, PartialEq)]
pub enum Discarding {
    /// Open a file; `None` asks for one with the open panel.
    Open(Option<String>),
    /// Start a new empty model of this size.
    New([u32; 3]),
    Quit,
}

/// What the unsaved-changes prompt is waiting on.
pub struct Confirm {
    pub action: Discarding,
}

/// The editor's default size for a new model.
pub const NEW_MODEL_SIZE: [u32; 3] = [32, 32, 32];

impl App {
    /// Whether the document differs from the last save or load.
    pub fn is_dirty(&self) -> bool {
        self.scene.is_some() && self.history.revision() != self.saved_revision
    }

    /// Runs `action` now, or asks first if that would lose unsaved changes.
    pub fn request(&mut self, action: Discarding, ctx: &egui::Context, state: &egui_wgpu::RenderState) {
        if self.is_dirty() {
            self.confirm = Some(Confirm { action });
        } else {
            self.perform(action, ctx, state);
        }
    }

    fn perform(&mut self, action: Discarding, ctx: &egui::Context, state: &egui_wgpu::RenderState) {
        match action {
            Discarding::Open(path) => {
                let path = path.or_else(|| self.open_panel());
                if let Some(path) = path {
                    self.load(state, &path);
                }
            }
            Discarding::New(size) => {
                self.install(state, scene::LoadedScene::new_empty(size));
                self.console_log.push(format!("new model {}x{}x{}", size[0], size[1], size[2]));
            }
            Discarding::Quit => {
                self.allow_close = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
    }

    /// Saves to the file's own path, or asks for one if it has none.
    /// Returns a message for the log, or `None` if the user cancelled.
    pub fn save(&mut self) -> Option<String> {
        let path = match self.scene.as_ref()?.path.clone() {
            Some(p) => p,
            None => self.save_panel()?,
        };
        Some(self.save_to(&path))
    }

    /// Always asks where to save.
    pub fn save_as(&mut self) -> Option<String> {
        let path = self.save_panel()?;
        Some(self.save_to(&path))
    }

    pub fn save_to(&mut self, path: &str) -> String {
        let Some(loaded) = self.scene.as_mut() else {
            return "nothing to save".to_owned();
        };
        match loaded.save(path) {
            Ok(bytes) => {
                loaded.set_path(path);
                self.saved_revision = self.history.revision();
                format!("saved {path} ({bytes} bytes)")
            }
            Err(e) => format!("error: could not save {path}: {e}"),
        }
    }

    /// Writes `stem.obj`, `stem.mtl` and `stem.png` next to each other.
    /// With no path, asks for one.
    pub fn export_obj(&mut self, path: Option<&str>) -> Option<String> {
        let loaded = self.scene.as_ref()?;
        let path = match path {
            Some(p) => std::path::PathBuf::from(p),
            None => {
                let stem = std::path::Path::new(&loaded.name)
                    .file_stem()
                    .map_or("model".to_owned(), |s| s.to_string_lossy().into_owned());
                let mut dialog = rfd::FileDialog::new()
                    .add_filter("Wavefront OBJ", &["obj"])
                    .set_file_name(format!("{stem}.obj"));
                if let Some(dir) = self.current_dir() {
                    dialog = dialog.set_directory(dir);
                }
                dialog.save_file()?
            }
        };
        let path = path.with_extension("obj");
        let stem = path.file_stem()?.to_string_lossy().into_owned();
        let hidden = self.hidden_layers;
        let export = voxel_core::export::export_obj(
            &loaded.doc.scene,
            &loaded.doc.palette,
            |i| !crate::is_hidden(&hidden, i.layer),
            &stem,
        );
        let written = std::fs::write(&path, &export.obj)
            .and_then(|()| std::fs::write(path.with_extension("mtl"), &export.mtl))
            .and_then(|()| std::fs::write(path.with_extension("png"), &export.png));
        Some(match written {
            Ok(()) => format!("exported {} ({} quads, with .mtl and .png)", path.display(), export.quads),
            Err(e) => format!("error: could not export {}: {e}", path.display()),
        })
    }

    fn open_panel(&self) -> Option<String> {
        let mut dialog = rfd::FileDialog::new().add_filter("the reference editor model", &["vox"]);
        if let Some(dir) = self.current_dir() {
            dialog = dialog.set_directory(dir);
        }
        dialog.pick_file().map(|p| p.to_string_lossy().into_owned())
    }

    fn save_panel(&self) -> Option<String> {
        let name = self.scene.as_ref().map_or("untitled.vox".to_owned(), |s| s.name.clone());
        let mut dialog = rfd::FileDialog::new()
            .add_filter("the reference editor model", &["vox"])
            .set_file_name(name);
        if let Some(dir) = self.current_dir() {
            dialog = dialog.set_directory(dir);
        }
        let mut path = dialog.save_file()?;
        if path.extension().is_none() {
            path.set_extension("vox");
        }
        Some(path.to_string_lossy().into_owned())
    }

    fn current_dir(&self) -> Option<std::path::PathBuf> {
        let path = self.scene.as_ref()?.path.as_ref()?;
        Some(std::path::Path::new(path).parent()?.to_path_buf())
    }

    /// The window title: file name, with a dot while there are unsaved
    /// changes, as macOS shows it.
    pub fn title(&self) -> String {
        match &self.scene {
            Some(s) if self.is_dirty() => format!("● {} — Voxferro", s.name),
            Some(s) => format!("{} — Voxferro", s.name),
            None => "Voxferro".to_owned(),
        }
    }

    /// File-level keyboard shortcuts, the window's close button and files
    /// dropped onto the window.
    pub fn file_input(&mut self, ctx: &egui::Context, state: &egui_wgpu::RenderState) {
        let (save, save_as, open, new) = ctx.input(|i| {
            let cmd = i.modifiers.command;
            (
                cmd && !i.modifiers.shift && i.key_pressed(egui::Key::S),
                cmd && i.modifiers.shift && i.key_pressed(egui::Key::S),
                cmd && i.key_pressed(egui::Key::O),
                cmd && i.key_pressed(egui::Key::N),
            )
        });
        if save {
            if let Some(m) = self.save() {
                self.console_log.push(m);
            }
        }
        if save_as {
            if let Some(m) = self.save_as() {
                self.console_log.push(m);
            }
        }
        if open {
            self.request(Discarding::Open(None), ctx, state);
        }
        if new {
            self.request(Discarding::New(NEW_MODEL_SIZE), ctx, state);
        }

        let dropped = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .find(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("vox")))
        });
        if let Some(path) = dropped {
            self.request(Discarding::Open(Some(path.to_string_lossy().into_owned())), ctx, state);
        }

        // Closing the window with unsaved work: cancel the close and ask.
        if ctx.input(|i| i.viewport().close_requested()) && !self.allow_close && self.is_dirty() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.confirm = Some(Confirm { action: Discarding::Quit });
        }
    }

    /// The unsaved-changes prompt, while one is pending.
    pub fn confirm_ui(&mut self, ctx: &egui::Context, state: &egui_wgpu::RenderState) {
        let Some(confirm) = &self.confirm else { return };
        let action = confirm.action.clone();
        let name = self.scene.as_ref().map_or(String::new(), |s| s.name.clone());

        let mut choice = None;
        egui::Modal::new(egui::Id::new("unsaved changes")).show(ctx, |ui| {
            ui.heading("Unsaved changes");
            ui.label(format!("Save the changes to {name} first?"));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Save").clicked() {
                    choice = Some(true);
                }
                if ui.button("Don't save").clicked() {
                    choice = Some(false);
                }
                if ui.button("Cancel").clicked() {
                    self.confirm = None;
                }
            });
        });

        match choice {
            Some(true) => {
                self.confirm = None;
                // Cancelling the save panel cancels the whole action.
                if let Some(message) = self.save() {
                    let saved = !message.starts_with("error");
                    self.console_log.push(message);
                    if saved {
                        self.perform(action, ctx, state);
                    }
                }
            }
            Some(false) => {
                self.confirm = None;
                self.perform(action, ctx, state);
            }
            None => {}
        }
    }

    /// `open [path]`, `new [x y z]`, `save [path]`.
    pub fn file_command(&mut self, line: &str, ctx: &egui::Context, state: &egui_wgpu::RenderState) -> String {
        let mut words = line.split_whitespace();
        let verb = words.next().unwrap_or_default();
        let args: Vec<&str> = words.collect();
        match verb {
            "open" => {
                let path = args.first().map(|p| (*p).to_owned());
                self.request(Discarding::Open(path), ctx, state);
                if self.confirm.is_some() {
                    "unsaved changes: save or discard them first".to_owned()
                } else {
                    self.status.clone()
                }
            }
            "new" => {
                let size = match args.as_slice() {
                    [] => NEW_MODEL_SIZE,
                    [n] => match n.parse::<u32>() {
                        Ok(n) => [n; 3],
                        Err(_) => return "new [size] or new <x> <y> <z>".to_owned(),
                    },
                    [x, y, z] => match (x.parse(), y.parse(), z.parse()) {
                        (Ok(x), Ok(y), Ok(z)) => [x, y, z],
                        _ => return "new [size] or new <x> <y> <z>".to_owned(),
                    },
                    _ => return "new [size] or new <x> <y> <z>".to_owned(),
                };
                if size.iter().any(|&s| s == 0 || s > 256) {
                    return "a model is 1 to 256 voxels on each side".to_owned();
                }
                self.request(Discarding::New(size), ctx, state);
                if self.confirm.is_some() {
                    "unsaved changes: save or discard them first".to_owned()
                } else {
                    String::new()
                }
            }
            _ => match args.first() {
                Some(path) => self.save_to(path),
                None => self.save().unwrap_or_else(|| "save cancelled".to_owned()),
            },
        }
    }
}
