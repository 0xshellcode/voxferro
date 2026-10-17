//! The Project panel: a folder of `.vox` files, opened with a click, and
//! the top bar's file actions the reference editor has (`config/ui/top.ui`): rename,
//! duplicate, and remove to the trash.
//!
//! - `project` reports the folder; `project list` names its files;
//! - `project dir <path>` changes it (remembered between sessions);
//! - `project open <name>` opens one, asking first about unsaved changes;
//! - `project rename <name>` renames the open file on disk;
//! - `project dup` saves the document as the next free `<name>-<n>.vox`
//!   beside it (the reference editor's `rename_prefix` is `-`) and goes on there;
//! - `project delete` moves the open file to the system's trash, after a
//!   confirmation, and starts a new model.
//!
//! The folder starts as the open file's, else the bundled samples'.

use crate::App;
use eframe::{egui, egui_wgpu};
use std::path::{Path, PathBuf};

/// Where the chosen folder is remembered.
fn saved_dir_file() -> Option<PathBuf> {
    Some(crate::recent::settings_dir()?.join("project_dir.txt"))
}

/// The `.vox` files in `dir`, sorted by name.
pub fn vox_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut out: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().is_some_and(|e| e.eq_ignore_ascii_case("vox")))
        .collect();
    out.sort_by_key(|p| p.file_name().map(|n| n.to_ascii_lowercase()));
    out
}

/// `stem-1.vox`, `stem-2.vox`... the first that doesn't exist.
pub fn next_free(path: &Path) -> PathBuf {
    let dir = path.parent().unwrap_or(Path::new("."));
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    // A copy of a copy counts on: castle-2 becomes castle-3, not castle-2-1.
    let base = match stem.rsplit_once('-') {
        Some((head, n)) if !head.is_empty() && n.parse::<u32>().is_ok() => head.to_owned(),
        _ => stem,
    };
    (1..)
        .map(|n| dir.join(format!("{base}-{n}.vox")))
        .find(|p| !p.exists())
        .expect("some name is free")
}

/// A new name for a file: a bare name, kept in the same folder, with
/// `.vox` added if missing. Path separators are refused.
pub fn renamed(path: &Path, name: &str) -> Result<PathBuf, String> {
    let name = name.trim();
    if name.is_empty() || name.contains(['/', '\\']) || name == "." || name == ".." {
        return Err(format!("'{name}' is not a file name"));
    }
    let name = if name.to_ascii_lowercase().ends_with(".vox") { name.to_owned() } else { format!("{name}.vox") };
    Ok(path.parent().unwrap_or(Path::new(".")).join(name))
}

/// The Project panel's tabs, as the reference editor's (`file.ui`): project files,
/// patterns, voxel scripts ("shader"), and the scene outline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    Project,
    Pattern,
    Shader,
    Outline,
}

#[derive(Default)]
pub struct ProjectState {
    pub tab: Tab,
    dir: Option<PathBuf>,
    /// The folder's files, read on first use and after each change.
    files: Option<Vec<PathBuf>>,
    rename_to: String,
    /// The file the trash confirmation is waiting on.
    pub confirm_delete: Option<PathBuf>,
}

impl App {
    pub fn project_dir(&mut self) -> Option<PathBuf> {
        if self.project.dir.is_none() {
            let saved = saved_dir_file()
                .and_then(|f| std::fs::read_to_string(f).ok())
                .map(|s| PathBuf::from(s.trim()))
                .filter(|d| d.is_dir());
            let open = self.scene.as_ref().and_then(|s| s.path.as_ref()).and_then(|p| Some(Path::new(p).parent()?.to_path_buf()));
            let samples = Some(crate::sample_dir()).filter(|d| d.is_dir());
            self.project.dir = saved.or(open).or(samples);
        }
        self.project.dir.clone()
    }

    /// The project folder's `.vox` files, for the pattern library.
    pub fn project_vox_files(&mut self) -> Vec<PathBuf> {
        self.project_files()
    }

    fn project_files(&mut self) -> Vec<PathBuf> {
        if self.project.files.is_none() {
            let files = self.project_dir().map(|d| vox_files(&d)).unwrap_or_default();
            self.project.files = Some(files);
        }
        self.project.files.clone().unwrap_or_default()
    }

    pub fn project_command(&mut self, line: &str, ctx: &egui::Context, state: &egui_wgpu::RenderState) -> String {
        let words: Vec<&str> = line.split_whitespace().collect();
        let rest = |n: usize| words.get(n..).map(|w| w.join(" ")).unwrap_or_default();
        match words.get(1).copied() {
            None => match self.project_dir() {
                Some(d) => format!("project folder {} ({} files)", d.display(), self.project_files().len()),
                None => "no project folder: project dir <path>".to_owned(),
            },
            Some("list") => {
                let names: Vec<String> = self
                    .project_files()
                    .iter()
                    .filter_map(|p| Some(p.file_stem()?.to_string_lossy().into_owned()))
                    .collect();
                if names.is_empty() { "no .vox files in the project folder".to_owned() } else { names.join(", ") }
            }
            Some("dir") => {
                let dir = PathBuf::from(rest(2));
                if !dir.is_dir() {
                    return format!("{} is not a folder", dir.display());
                }
                if let Some(f) = saved_dir_file() {
                    if let Some(parent) = f.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    let _ = std::fs::write(f, dir.to_string_lossy().as_bytes());
                }
                self.project.dir = Some(dir.clone());
                self.project.files = None;
                format!("project folder {} ({} files)", dir.display(), self.project_files().len())
            }
            Some("open") => {
                let name = rest(2);
                let found = self.project_files().into_iter().find(|p| {
                    p.file_stem().is_some_and(|s| s.to_string_lossy() == name)
                        || p.file_name().is_some_and(|s| s.to_string_lossy() == name)
                });
                match found {
                    Some(path) => self.file_command(&format!("open {}", path.display()), ctx, state),
                    None => format!("no {name} in the project folder"),
                }
            }
            Some("rename") => self.rename_project(&rest(2)),
            Some("dup") => {
                let Some(loaded) = &self.scene else { return "nothing to duplicate".to_owned() };
                let base = match &loaded.path {
                    Some(p) => PathBuf::from(p),
                    None => match self.project_dir() {
                        Some(d) => d.join("untitled.vox"),
                        None => return "save the model first, or choose a project folder".to_owned(),
                    },
                };
                let target = next_free(&base);
                let message = self.save_to(&target.to_string_lossy());
                self.project.files = None;
                message
            }
            Some("delete") => match self.scene.as_ref().and_then(|s| s.path.clone()) {
                Some(path) => {
                    self.project.confirm_delete = Some(PathBuf::from(path));
                    "confirm moving the file to the trash".to_owned()
                }
                None => "the model has no file to remove".to_owned(),
            },
            _ => "project [list | dir <path> | open <name> | rename <name> | dup | delete]".to_owned(),
        }
    }

    fn rename_project(&mut self, name: &str) -> String {
        let Some(path) = self.scene.as_ref().and_then(|s| s.path.clone()) else {
            return "the model has no file yet: save it first".to_owned();
        };
        let from = PathBuf::from(&path);
        let to = match renamed(&from, name) {
            Ok(t) => t,
            Err(e) => return e,
        };
        if to.exists() {
            return format!("{} already exists", to.display());
        }
        if let Err(e) = std::fs::rename(&from, &to) {
            return format!("error: could not rename: {e}");
        }
        let to_str = to.to_string_lossy().into_owned();
        if let Some(loaded) = self.scene.as_mut() {
            loaded.set_path(&to_str);
        }
        self.remember(&to_str);
        self.project.files = None;
        format!("renamed to {to_str}")
    }

    /// The confirmation before moving a file to the trash.
    pub fn project_confirm_ui(&mut self, ctx: &egui::Context, state: &egui_wgpu::RenderState) {
        let Some(path) = self.project.confirm_delete.clone() else { return };
        let mut choice = None;
        egui::Modal::new(egui::Id::new("trash project")).show(ctx, |ui| {
            ui.heading("Move to the trash?");
            ui.label(format!("{} will be moved to the trash, and a new model started.", path.display()));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Move to Trash").clicked() {
                    choice = Some(true);
                }
                if ui.button("Cancel").clicked() {
                    choice = Some(false);
                }
            });
        });
        match choice {
            Some(true) => {
                self.project.confirm_delete = None;
                let message = match trash::delete(&path) {
                    Ok(()) => {
                        // The file is gone: nothing to save, so no prompt.
                        self.saved_revision = self.history.revision();
                        self.request(crate::files::Discarding::New(crate::files::NEW_MODEL_SIZE), ctx, state);
                        format!("moved {} to the trash", path.display())
                    }
                    Err(e) => format!("error: could not move {} to the trash: {e}", path.display()),
                };
                self.project.files = None;
                self.console_log.push(message);
            }
            Some(false) => self.project.confirm_delete = None,
            None => {}
        }
    }

    /// The right-hand Project panel: tabs above, Export folded below.
    pub fn project_panel(&mut self, ui: &mut egui::Ui, state: &egui_wgpu::RenderState) {
        crate::layout::title(ui, "Project");
        let tabs = [
            (Tab::Project, "Files", "The project folder's models"),
            (Tab::Pattern, "Pattern", "The pattern brush's model and library"),
            (Tab::Shader, "Script", "Voxel scripts (the reference editor's shaders)"),
            (Tab::Outline, "Scene", "The outline: models, groups, layers, animation"),
        ];
        let items: Vec<(bool, &str, &str)> = tabs.iter().map(|(t, l, h)| (self.project.tab == *t, *l, *h)).collect();
        if let Some(i) = crate::layout::toggle_row(ui, &items) {
            self.project.tab = tabs[i].0;
        }
        ui.separator();
        // Export at the bottom, as there; the tab's content fills the rest.
        egui::Panel::bottom("project export").show(ui, |ui| {
            egui::CollapsingHeader::new("EXPORT").id_salt("project export").show(ui, |ui| {
                ui.add_enabled_ui(self.scene.is_some(), |ui| {
                    ui.horizontal_wrapped(|ui| {
                        for (name, _, hint) in crate::exchange::FORMATS {
                            if ui.button(name).on_hover_text(hint).clicked() {
                                let m = self.export_command(&format!("export {name}"));
                                self.console_log.push(m);
                            }
                        }
                    });
                });
            });
        });
        egui::ScrollArea::vertical().id_salt("project tab").show(ui, |ui| match self.project.tab {
            Tab::Project => {
                self.project_ui(ui, state);
                ui.separator();
                self.recent_ui(ui, state);
            }
            Tab::Pattern => self.pattern_ui(ui),
            Tab::Shader => self.scripts_ui(ui, state),
            Tab::Outline => {
                self.scene_ui(ui);
                ui.separator();
                ui.label("Layers");
                self.layers_ui(ui);
                ui.separator();
                ui.label("Animation");
                self.animation_ui(ui);
            }
        });
    }

    pub fn project_ui(&mut self, ui: &mut egui::Ui, state: &egui_wgpu::RenderState) {
        let ctx = ui.ctx().clone();
        let mut line: Option<String> = None;
        ui.horizontal(|ui| {
            let dir = self.project_dir();
            let label = dir
                .as_ref()
                .and_then(|d| d.file_name())
                .map_or("No folder".to_owned(), |n| n.to_string_lossy().into_owned());
            ui.label(label).on_hover_text(dir.map(|d| d.display().to_string()).unwrap_or_default());
            if ui.small_button("Folder…").clicked()
                && let Some(d) = rfd::FileDialog::new().pick_folder()
            {
                line = Some(format!("project dir {}", d.display()));
            }
            if ui.small_button("↻").on_hover_text("Read the folder again").clicked() {
                self.project.files = None;
            }
        });
        let open = self.scene.as_ref().and_then(|s| s.path.clone()).map(PathBuf::from);
        egui::ScrollArea::vertical().id_salt("project files").max_height(420.0).show(ui, |ui| {
            for path in self.project_files() {
                let name = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                let current = open.as_ref().is_some_and(|o| same_file(o, &path));
                if ui.selectable_label(current, name).clicked() && !current {
                    line = Some(format!("open {}", path.display()));
                }
            }
        });
        let has_file = open.is_some();
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut self.project.rename_to).hint_text("new name").desired_width(90.0));
            if ui.add_enabled(has_file, egui::Button::new("Rename").small()).clicked() {
                line = Some(format!("project rename {}", self.project.rename_to));
            }
        });
        ui.horizontal(|ui| {
            if ui.small_button("Duplicate").on_hover_text("Save a copy as name-1.vox and go on there").clicked() {
                line = Some("project dup".to_owned());
            }
            if ui.add_enabled(has_file, egui::Button::new("Trash…").small()).on_hover_text("Move the file to the trash").clicked() {
                line = Some("project delete".to_owned());
            }
        });
        if let Some(line) = line {
            let message = if line.starts_with("open ") {
                self.file_command(&line, &ctx, state)
            } else {
                self.project_command(&line, &ctx, state)
            };
            if !message.is_empty() {
                self.console_log.push(message);
            }
        }
    }
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copies_count_on() {
        let dir = std::env::temp_dir().join(format!("voxferro-project-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let castle = dir.join("castle.vox");
        assert_eq!(next_free(&castle), dir.join("castle-1.vox"));
        std::fs::write(dir.join("castle-1.vox"), b"").unwrap();
        assert_eq!(next_free(&castle), dir.join("castle-2.vox"));
        assert_eq!(next_free(&dir.join("castle-1.vox")), dir.join("castle-2.vox"));
        // A name that merely ends in a word keeps it.
        assert_eq!(next_free(&dir.join("sky-high.vox")), dir.join("sky-high-1.vox"));
        std::fs::write(dir.join("b.VOX"), b"").unwrap();
        std::fs::write(dir.join("a.txt"), b"").unwrap();
        let names: Vec<_> = vox_files(&dir).iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(names, ["b.VOX", "castle-1.vox"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn rename_stays_in_the_folder() {
        let p = Path::new("/a/b/castle.vox");
        assert_eq!(renamed(p, "fort").unwrap(), Path::new("/a/b/fort.vox"));
        assert_eq!(renamed(p, "fort.vox").unwrap(), Path::new("/a/b/fort.vox"));
        assert!(renamed(p, "../x").is_err());
        assert!(renamed(p, " ").is_err());
    }
}
