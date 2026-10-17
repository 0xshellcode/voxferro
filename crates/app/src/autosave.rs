//! Autosave and recovery: unsaved work survives a crash or a forced quit.
//!
//! While the document has unsaved changes, a copy is written every two
//! minutes (`autosave <minutes>`, `autosave off`) to the settings folder
//! (see `recent::settings_dir`), under `autosave/`: `session-<pid>.vox`
//! and a note, `session-<pid>.txt`, of the file's name and path and when
//! it was written. The copy goes away when the work is saved, undone back
//! to the saved state, or the editor closes normally -- after the user has
//! chosen to save or not.
//!
//! Each running editor holds a lock on its own `session-<pid>.lock`. At
//! start, a session whose lock nobody holds was left by an editor that did
//! not close normally; if it left a copy, the editor offers to recover it.
//! The lock tells a crashed session from one still open in another window,
//! on every platform, and goes when its process does.
//!
//! The copy is written to a temporary name and renamed into place, so a
//! crash while writing never leaves half a file where the last good copy
//! was. Scripted runs (`--run`, `--script`, ...) neither write copies nor
//! offer them.

use crate::App;
use eframe::{egui, egui_wgpu};
use std::fs::File;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// How often a copy is written while there are unsaved changes.
pub const DEFAULT_INTERVAL: Duration = Duration::from_secs(120);

/// A copy left by an editor that did not close normally.
#[derive(Debug, Clone)]
pub struct Orphan {
    /// The copy, its note and its lock.
    pub vox: PathBuf,
    note: PathBuf,
    lock: PathBuf,
    /// The document's name and path, as the note has them.
    pub name: String,
    pub path: Option<String>,
    /// When the copy was written, seconds since 1970.
    pub written: u64,
}

pub struct Autosave {
    /// Off for scripted runs, or by `autosave off`.
    pub enabled: bool,
    pub interval: Duration,
    dir: Option<PathBuf>,
    /// This session's lock, held while the editor runs.
    _lock: Option<File>,
    last_write: Instant,
    /// The history revision the copy on disk holds.
    written: Option<u64>,
    /// Copies of other sessions to offer, the first shown first.
    pub orphans: Vec<Orphan>,
}

fn session_files(dir: &Path, id: &str) -> (PathBuf, PathBuf, PathBuf) {
    (dir.join(format!("session-{id}.vox")), dir.join(format!("session-{id}.txt")), dir.join(format!("session-{id}.lock")))
}

/// The note: one `key=value` a line.
fn note_text(name: &str, path: Option<&str>, written: u64) -> String {
    format!("name={name}\npath={}\nwritten={written}\n", path.unwrap_or(""))
}

fn parse_note(text: &str) -> (String, Option<String>, u64) {
    let get = |k: &str| text.lines().find_map(|l| l.strip_prefix(k).and_then(|r| r.strip_prefix('='))).unwrap_or("");
    let path = Some(get("path").to_owned()).filter(|p| !p.is_empty());
    (get("name").to_owned(), path, get("written").parse().unwrap_or(0))
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

impl Autosave {
    /// Starts this session in `dir`: takes its lock and finds the copies
    /// sessions that did not close normally left behind.
    pub fn start(dir: Option<PathBuf>, enabled: bool) -> Self {
        let mut out = Self {
            enabled,
            interval: DEFAULT_INTERVAL,
            dir: None,
            _lock: None,
            last_write: Instant::now(),
            written: None,
            orphans: Vec::new(),
        };
        let Some(dir) = dir.filter(|_| enabled) else { return out };
        if std::fs::create_dir_all(&dir).is_err() {
            return out;
        }
        let (_, _, lock) = session_files(&dir, &std::process::id().to_string());
        out._lock = File::create(&lock).ok().filter(|f| f.try_lock().is_ok());
        out.orphans = find_orphans(&dir, &lock);
        out.dir = Some(dir);
        out
    }

    fn own(&self) -> Option<(PathBuf, PathBuf, PathBuf)> {
        Some(session_files(self.dir.as_ref()?, &std::process::id().to_string()))
    }

    /// Removes this session's copy, if there is one.
    pub fn clear(&mut self) {
        if let Some((vox, note, _)) = self.own() {
            let _ = std::fs::remove_file(vox);
            let _ = std::fs::remove_file(note);
        }
        self.written = None;
    }

    /// On a normal exit: the copy, and the lock file with it.
    pub fn finish(&mut self) {
        self.clear();
        if let Some((_, _, lock)) = self.own() {
            self._lock = None;
            let _ = std::fs::remove_file(lock);
        }
    }
}

/// The copies of sessions whose lock nobody holds. Sessions without a copy
/// are tidied away.
fn find_orphans(dir: &Path, own_lock: &Path) -> Vec<Orphan> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let lock = entry.path();
        let Some(id) = lock.file_name().and_then(|n| n.to_str()).and_then(|n| n.strip_prefix("session-")).and_then(|n| n.strip_suffix(".lock")) else {
            continue;
        };
        if lock == own_lock {
            continue;
        }
        // Held: that editor is still running.
        let free = File::options().write(true).open(&lock).is_ok_and(|f| f.try_lock().is_ok());
        if !free {
            continue;
        }
        let (vox, note, _) = session_files(dir, id);
        match std::fs::read_to_string(&note) {
            Ok(text) if vox.exists() => {
                let (name, path, written) = parse_note(&text);
                out.push(Orphan { vox, note, lock, name, path, written });
            }
            _ => {
                let _ = std::fs::remove_file(&vox);
                let _ = std::fs::remove_file(&note);
                let _ = std::fs::remove_file(&lock);
            }
        }
    }
    out.sort_by_key(|o| std::cmp::Reverse(o.written));
    out
}

impl Orphan {
    pub fn remove(&self) {
        let _ = std::fs::remove_file(&self.vox);
        let _ = std::fs::remove_file(&self.note);
        let _ = std::fs::remove_file(&self.lock);
    }
}

/// "5 minutes ago", for the recovery dialog.
fn ago(written: u64) -> String {
    let s = now().saturating_sub(written);
    match s {
        0..60 => "less than a minute ago".to_owned(),
        60..3600 => format!("{} minutes ago", s / 60),
        3600..86400 => format!("{} hours ago", s / 3600),
        _ => format!("{} days ago", s / 86400),
    }
}

impl App {
    /// Once a frame: writes a copy when it is due, and drops it once
    /// nothing is unsaved.
    pub fn autosave_tick(&mut self) {
        if !self.autosave.enabled {
            return;
        }
        if !self.is_dirty() {
            if self.autosave.written.is_some() {
                self.autosave.clear();
            }
            return;
        }
        let revision = self.history.revision();
        if self.autosave.written == Some(revision) || self.autosave.last_write.elapsed() < self.autosave.interval {
            return;
        }
        self.autosave.last_write = Instant::now();
        let (Some((vox, note, _)), Some(loaded)) = (self.autosave.own(), self.scene.as_ref()) else { return };
        let temp = vox.with_extension("vox.tmp");
        let written = loaded.save(&temp.to_string_lossy()).is_ok() && std::fs::rename(&temp, &vox).is_ok();
        if written && std::fs::write(&note, note_text(&loaded.name, loaded.path.as_deref(), now())).is_ok() {
            self.autosave.written = Some(revision);
        }
    }

    /// `autosave`, `autosave on|off`, `autosave <minutes>`, and `autosave
    /// recover|discard` for the copy a crashed session left.
    pub fn autosave_command(&mut self, line: &str, state: &egui_wgpu::RenderState) -> String {
        match line.split_whitespace().nth(1) {
            Some("recover") => return self.resolve_orphan(true, state),
            Some("discard") => return self.resolve_orphan(false, state),
            _ => {}
        }
        let a = &mut self.autosave;
        match line.split_whitespace().nth(1) {
            None => {}
            Some("off") => {
                a.enabled = false;
                a.clear();
            }
            Some("on") => a.enabled = a.dir.is_some(),
            Some(n) => match n.parse::<f32>() {
                Ok(m) if m > 0.0 && m <= 600.0 => {
                    a.interval = Duration::from_secs_f32(m * 60.0);
                    a.enabled = a.dir.is_some();
                }
                _ => return "autosave [on|off|<minutes>|recover|discard]".to_owned(),
            },
        }
        if !a.enabled {
            return "autosave off".to_owned();
        }
        let minutes = a.interval.as_secs_f32() / 60.0;
        match &a.dir {
            Some(d) => format!("autosave every {minutes} minutes while there are unsaved changes, in {}", d.display()),
            None => "autosave off".to_owned(),
        }
    }

    /// Offers the copy a session that did not close normally left.
    pub fn recovery_ui(&mut self, ctx: &egui::Context, state: &egui_wgpu::RenderState) {
        let Some(orphan) = self.autosave.orphans.first().cloned() else { return };
        let mut choice = None;
        egui::Modal::new(egui::Id::new("recover autosave")).show(ctx, |ui| {
            ui.heading("Recover unsaved work?");
            ui.label(format!(
                "Voxferro did not close normally. Unsaved changes to {} were saved {}.",
                orphan.name,
                ago(orphan.written)
            ));
            if let Some(p) = &orphan.path {
                ui.label(egui::RichText::new(p).small().weak());
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Recover").clicked() {
                    choice = Some(true);
                }
                if ui.button("Discard").clicked() {
                    choice = Some(false);
                }
            });
        });
        if let Some(recover) = choice {
            let message = self.resolve_orphan(recover, state);
            self.console_log.push(message);
        }
    }

    /// Recovers the first copy on offer, or discards it.
    pub fn resolve_orphan(&mut self, recover: bool, state: &egui_wgpu::RenderState) -> String {
        if self.autosave.orphans.is_empty() {
            return "no unsaved work to recover".to_owned();
        }
        let orphan = self.autosave.orphans.remove(0);
        let message = if recover {
            match crate::scene::LoadedScene::load(&orphan.vox.to_string_lossy()) {
                Ok(mut loaded) => {
                    loaded.name = orphan.name.clone();
                    loaded.path = orphan.path.clone();
                    self.install(state, loaded);
                    // Recovered work is unsaved work: saving goes to the
                    // file it came from, and closing asks first.
                    self.saved_revision = u64::MAX;
                    format!("recovered unsaved work on {}: save to keep it", orphan.name)
                }
                Err(e) => return format!("error: could not recover {}: {e}", orphan.vox.display()),
            }
        } else {
            format!("discarded the unsaved work on {}", orphan.name)
        };
        orphan.remove();
        message
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("voxferro-autosave-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn a_note_round_trips() {
        let text = note_text("castle.vox", Some("/tmp/castle.vox"), 1234);
        assert_eq!(parse_note(&text), ("castle.vox".to_owned(), Some("/tmp/castle.vox".to_owned()), 1234));
        assert_eq!(parse_note(&note_text("new.vox", None, 5)).1, None);
    }

    #[test]
    fn a_session_whose_lock_is_free_is_an_orphan_and_a_held_one_is_not() {
        let dir = scratch("orphans");
        std::fs::create_dir_all(&dir).unwrap();
        // A crashed session: copy, note, a lock nobody holds.
        let (vox, note, lock) = session_files(&dir, "1");
        std::fs::write(&vox, b"VOX ").unwrap();
        std::fs::write(&note, note_text("a.vox", None, 10)).unwrap();
        std::fs::write(&lock, b"").unwrap();
        // A running one: its lock held.
        let (vox2, note2, lock2) = session_files(&dir, "2");
        std::fs::write(&vox2, b"VOX ").unwrap();
        std::fs::write(&note2, note_text("b.vox", None, 20)).unwrap();
        let held = File::create(&lock2).unwrap();
        held.try_lock().unwrap();
        // A crashed session with no copy is tidied away.
        let (_, _, lock3) = session_files(&dir, "3");
        std::fs::write(&lock3, b"").unwrap();

        let me = Autosave::start(Some(dir.clone()), true);
        let names: Vec<&str> = me.orphans.iter().map(|o| o.name.as_str()).collect();
        assert_eq!(names, ["a.vox"]);
        assert!(!lock3.exists());
        me.orphans[0].remove();
        assert!(!vox.exists() && !note.exists());
        drop(held);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn off_for_scripted_runs() {
        let dir = scratch("off");
        let a = Autosave::start(Some(dir.clone()), false);
        assert!(!a.enabled && a.orphans.is_empty());
        assert!(!dir.exists(), "nothing written");
    }
}
