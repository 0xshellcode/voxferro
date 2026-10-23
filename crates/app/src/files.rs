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
    /// the reference editor's *Recover*: the models and palette of a damaged file.
    Recover(String),
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
                    if self.scene.as_ref().and_then(|s| s.path.as_deref()) == Some(path.as_str()) {
                        self.remember(&path);
                    }
                }
            }
            Discarding::New(size) => {
                self.install(state, scene::LoadedScene::new_empty(size));
                self.console_log.push(format!("new model {}x{}x{}", size[0], size[1], size[2]));
            }
            Discarding::Recover(path) => {
                let message = match std::fs::read(&path) {
                    Ok(bytes) => match vox_format::VoxFile::recover(&bytes) {
                        Some(file) => {
                            let (models, voxels) = (file.models.len(), file.voxel_count());
                            let mut recovered = scene::LoadedScene::from_file(file, None);
                            let stem = std::path::Path::new(&path).file_stem().map_or("model".into(), |s| s.to_string_lossy());
                            recovered.name = format!("{stem}-recovered.vox");
                            self.install(state, recovered);
                            format!("recovered {models} model(s), {voxels} voxels, from {path}: save to keep them")
                        }
                        None => format!("{path}: no models found to recover"),
                    },
                    Err(e) => format!("error: {path}: {e}"),
                };
                self.console_log.push(message);
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
                self.remember(path);
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
        let visible = self.visibility();
        let scene = self.export_scene()?;
        let export = voxel_core::export::export_obj(
            scene.as_ref(),
            &loaded.doc.palette,
            |i| visible.shows(i),
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

    /// Adds a file to the recent list, unless this is a scripted run.
    pub fn remember(&mut self, path: &str) {
        if self.remember_files {
            crate::recent::push(&mut self.recent, path);
        }
    }

    /// The recent files, in the panel under the file buttons.
    pub fn recent_ui(&mut self, ui: &mut egui::Ui, state: &egui_wgpu::RenderState) {
        if self.recent.is_empty() {
            return;
        }
        let mut open = None;
        egui::CollapsingHeader::new("Recent").show(ui, |ui| {
            for path in &self.recent {
                let name = std::path::Path::new(path)
                    .file_name()
                    .map_or(path.clone(), |n| n.to_string_lossy().into_owned());
                if ui.small_button(name).on_hover_text(path).clicked() {
                    open = Some(path.clone());
                }
            }
        });
        if let Some(path) = open {
            let ctx = ui.ctx().clone();
            self.request(Discarding::Open(Some(path)), &ctx, state);
        }
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
                // Cmd+P too: the reference editor's new project.
                cmd && !i.modifiers.shift && (i.key_pressed(egui::Key::N) || i.key_pressed(egui::Key::P)),
            )
        });
        if save
            && let Some(m) = self.save()
        {
            self.console_log.push(m);
        }
        if save_as
            && let Some(m) = self.save_as()
        {
            self.console_log.push(m);
        }
        if open {
            self.request(Discarding::Open(None), ctx, state);
        }
        if new {
            self.request(Discarding::New(NEW_MODEL_SIZE), ctx, state);
        }

        // A file dropped on the window asks what to do with it, where it
        // landed, as the reference editor's drop menu does.
        let dropped = ctx.input(|i| {
            let at = i.pointer.latest_pos().unwrap_or_default();
            i.raw.dropped_files.first().map(|f| (f.path().to_string_lossy().into_owned(), at))
        });
        if let Some(drop) = dropped {
            self.dropped = Some(drop);
        }
        self.drop_menu_ui(ctx, state);

        // Closing the window with unsaved work: cancel the close and ask.
        if ctx.input(|i| i.viewport().close_requested()) && !self.allow_close && self.is_dirty() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.confirm = Some(Confirm { action: Discarding::Quit });
        }
    }

    /// The menu for a dropped file: open it as the project, add its models
    /// to this one, or take it as the palette -- what its kind allows.
    fn drop_menu_ui(&mut self, ctx: &egui::Context, state: &egui_wgpu::RenderState) {
        let Some((path, at)) = self.dropped.clone() else { return };
        let ext = std::path::Path::new(&path).extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
        let name = std::path::Path::new(&path).file_name().map_or(path.clone(), |n| n.to_string_lossy().into_owned());
        let can_import = matches!(ext.as_str(), "vox" | "xraw" | "qb" | "obj");
        let mut choice: Option<&str> = None;
        let shown = egui::Area::new(egui::Id::new("drop menu")).order(egui::Order::Foreground).fixed_pos(at).show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.label(egui::RichText::new(&name).strong());
                if ext == "vox" && ui.button("Open project").on_hover_text("Open it in place of this one").clicked() {
                    choice = Some("open");
                }
                if can_import && ui.button("Import models").on_hover_text("Add its models to this scene").clicked() {
                    choice = Some("import");
                }
                if can_import && ui.button("Import sprites").on_hover_text("Its models as the frames of one looping animation").clicked() {
                    choice = Some("sprites");
                }
                if ext == "png" && ui.button("Use as palette").on_hover_text("A 256x1 palette image").clicked() {
                    choice = Some("palette");
                }
                if !can_import && ext != "png" {
                    ui.label("Voxferro cannot open this kind of file.");
                }
                if ui.button("Cancel").clicked() {
                    choice = Some("cancel");
                }
            });
        });
        // A click anywhere else, or Escape, dismisses it.
        let away = ctx.input(|i| i.pointer.any_pressed() && !i.pointer.latest_pos().is_some_and(|p| shown.response.rect.contains(p)))
            || ctx.input(|i| i.key_pressed(egui::Key::Escape));
        if choice.is_none() && !away {
            return;
        }
        self.dropped = None;
        let message = match choice {
            Some("open") => {
                self.request(Discarding::Open(Some(path)), ctx, state);
                return;
            }
            Some("import") => self.import_command(&format!("import {path}")),
            // the reference editor's Import Sprites: import, then pack what came in.
            Some("sprites") => self.import_command(&format!("import sprites {path}")),
            Some("palette") => self.palette_file_command(&format!("palette open {path}"), state).unwrap_or_default(),
            _ => return,
        };
        self.console_log.push(message);
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
            "recover" => match args.is_empty() {
                true => "recover <file.vox>".to_owned(),
                false => {
                    self.request(Discarding::Recover(args.join(" ")), ctx, state);
                    if self.confirm.is_some() { "unsaved changes: save or discard them first".to_owned() } else { String::new() }
                }
            },
            _ => match args.first() {
                Some(path) => self.save_to(path),
                None => self.save().unwrap_or_else(|| "save cancelled".to_owned()),
            },
        }
    }
}
