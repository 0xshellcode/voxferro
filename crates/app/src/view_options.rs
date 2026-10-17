//! What the viewport draws besides the voxels: the reference editor's display
//! switches (`config/ui/editor.ui`), with its shortcuts.
//!
//! | Switch | `view` word | Shortcut |
//! |---|---|---|
//! | Display Edges | `edges` | Cmd+E |
//! | Display Frame | `frame` | Cmd+F |
//! | Display Grids | `grid` | Cmd+G |
//! | Display Background Objects | `bg` | Cmd+B |
//! | Display Wireframes | `wire` | Cmd+M |
//! | Display Shadow | `shadow` | -- |
//!
//! `view <word> [on|off]` sets one, toggling without an argument; `view`
//! lists them. With background objects off, only the selected model is
//! drawn. Shadows come from a depth map seen from the light (see the voxel
//! shader); the reference editor's Cmd+W for them is left to the system, which closes
//! the window with it on macOS.

use crate::App;
use eframe::egui;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ViewOptions {
    pub edges: bool,
    pub frame: bool,
    pub grid: bool,
    pub background: bool,
    pub wire: bool,
    pub shadow: bool,
}

impl Default for ViewOptions {
    fn default() -> Self {
        Self { edges: false, frame: true, grid: true, background: true, wire: false, shadow: false }
    }
}

/// Word, label, hint and shortcut key of each switch.
const SWITCHES: [(&str, &str, &str, egui::Key); 6] = [
    ("edges", "Edge", "Voxel edges (Cmd+E)", egui::Key::E),
    ("frame", "Frame", "The model's box (Cmd+F)", egui::Key::F),
    ("grid", "Grid", "The grid on the model's floor (Cmd+G)", egui::Key::G),
    ("bg", "BG", "The other models, besides the selected one (Cmd+B)", egui::Key::B),
    ("wire", "Wire", "Wireframe of the mesh (Cmd+M)", egui::Key::M),
    // No shortcut: Cmd+W closes the window on macOS.
    ("shadow", "Shadow", "Shadows from the light", egui::Key::F35),
];

impl ViewOptions {
    fn slot(&mut self, word: &str) -> Option<&mut bool> {
        Some(match word {
            "edges" | "edge" => &mut self.edges,
            "frame" => &mut self.frame,
            "grid" => &mut self.grid,
            "bg" | "background" => &mut self.background,
            "wire" | "wireframe" => &mut self.wire,
            "shadow" | "shadows" => &mut self.shadow,
            _ => return None,
        })
    }
}

impl App {
    pub fn view_command(&mut self, line: &str) -> String {
        let words: Vec<&str> = line.split_whitespace().skip(1).collect();
        let before = self.view;
        let message = match words.as_slice() {
            [] => {
                let v = self.view;
                let on = |b: bool| if b { "on" } else { "off" };
                return format!(
                    "edges {}, frame {}, grid {}, bg {}, wire {}, shadow {}",
                    on(v.edges),
                    on(v.frame),
                    on(v.grid),
                    on(v.background),
                    on(v.wire),
                    on(v.shadow)
                );
            }
            ["cube", rest @ ..] => {
                self.view_cube_shown = match rest {
                    [] => !self.view_cube_shown,
                    ["on"] => true,
                    ["off"] => false,
                    _ => return "view cube [on|off]".to_owned(),
                };
                format!("view cube {}", if self.view_cube_shown { "on" } else { "off" })
            }
            ["ruler", rest @ ..] => {
                self.ruler_shown = match rest {
                    [] => !self.ruler_shown,
                    ["on"] => true,
                    ["off"] => false,
                    _ => return "view ruler [on|off]".to_owned(),
                };
                format!("view ruler {}", if self.ruler_shown { "on" } else { "off" })
            }
            [word, rest @ ..] => {
                let Some(slot) = self.view.slot(word) else {
                    return "view [edges|frame|grid|bg|wire|shadow] [on|off]".to_owned();
                };
                *slot = match rest {
                    [] => !*slot,
                    ["on"] => true,
                    ["off"] => false,
                    _ => return "view <switch> [on|off]".to_owned(),
                };
                format!("{word} {}", if *slot { "on" } else { "off" })
            }
        };
        // Hiding the background and the wireframe change what is meshed.
        if self.view.background != before.background || self.view.wire != before.wire {
            self.mesh_dirty = true;
        }
        message
    }

    /// Cmd+E, F, G, B and M, as in the reference editor; in the render, Cmd+E and
    /// Cmd+G switch its edges and grid instead.
    pub fn view_input(&mut self, ctx: &egui::Context) {
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        let pressed: Vec<&str> = ctx.input(|i| {
            SWITCHES
                .iter()
                .filter(|(.., key)| i.modifiers.command && !i.modifiers.shift && i.key_pressed(*key))
                .map(|(word, ..)| *word)
                .collect()
        });
        for word in pressed {
            // In the render, Cmd+E and Cmd+G are its own lines, as there.
            let message = match word {
                "edges" if self.rendering => self.lines_command(&["edge"]),
                "grid" if self.rendering => self.lines_command(&["grid"]),
                _ => self.view_command(&format!("view {word}")),
            };
            self.console_log.push(message);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_switch_has_a_slot() {
        let mut v = ViewOptions::default();
        for (word, ..) in SWITCHES {
            assert!(v.slot(word).is_some(), "{word}");
        }
        assert!(v.slot("sun").is_none());
    }
}
