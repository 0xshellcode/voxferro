//! What the viewport draws besides the voxels: the display switches, with
//! their shortcuts.
//!
//! | Switch | `view` word | Shortcut |
//! |---|---|---|
//! | Display Edges | `edges` | Cmd+E |
//! | Display Frame (lines every `spacing` voxels) | `frame` | Cmd+F |
//! | Display Grids | `grid` | Cmd+G |
//! | Display Background Objects | `bg` | Cmd+B |
//! | Display Wireframes | `wire` | Cmd+M |
//! | Display Shadow | `shadow` | -- |
//!
//! `view <word> [on|off]` sets one, toggling without an argument; `view`
//! lists them. With background objects off, only the selected model is
//! drawn. The model's box is always drawn; the frame is lines every
//! `spacing` voxels (10) on its floor and on the voxels.
//!
//! The options of the Option menu, none of them saved:
//!
//! | Option | `view` word | Default |
//! |---|---|---|
//! | Ambient occlusion | `ao` | on |
//! | Constant shading | `flat` | off |
//! | Fade background objects | `fade` | on |
//! | Colours | `colour bg\|grid\|edge\|select <c>` | 38 38 38, 17 17 17, 17 17 17, white |
//! | Frame spacing | `spacing <n>` | 10 |
//! | Interface scale | `scale <f>\|inc\|dec` | 1, steps of 1/8 |
//!
//! Ambient occlusion counts the solid cells around each face corner (see
//! `voxel_core::greedy`) and darkens the ambient light a fifth for each.
//! Constant shading shows the palette colours unlit. Faded objects -- every
//! model but the one being edited, outside the Move tool -- are drawn half
//! way to the background colour. Shadows come from a depth map seen from the light (see the voxel
//! shader); Cmd+W is left to the system, which closes the window with it on
//! macOS.

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
        Self { edges: false, frame: false, grid: true, background: true, wire: false, shadow: false }
    }
}

/// Word, label, hint and shortcut key of each switch.
const SWITCHES: [(&str, &str, &str, egui::Key); 6] = [
    ("edges", "Edge", "Voxel edges (Cmd+E)", egui::Key::E),
    ("frame", "Frame", "Frame lines every few voxels (Cmd+F; the spacing is in Settings)", egui::Key::F),
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

/// The editor options (see the module notes). For the session.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EditorView {
    pub ao: bool,
    pub flat: bool,
    pub fade: bool,
    pub background: [u8; 3],
    pub grid: [u8; 3],
    pub edge: [u8; 3],
    pub select: [u8; 3],
    pub spacing: u32,
    pub scale: f32,
}

impl Default for EditorView {
    fn default() -> Self {
        Self {
            ao: true,
            flat: false,
            fade: true,
            background: [38, 38, 38],
            grid: [17, 17, 17],
            edge: [17, 17, 17],
            select: [255, 255, 255],
            spacing: 10,
            scale: 1.0,
        }
    }
}

/// The interface scale's limits and step.
pub const SCALE_RANGE: (f32, f32) = (0.5, 4.0);
pub const SCALE_STEP: f32 = 0.125;

/// `scale` kept within the limits and snapped to the nearest eighth.
pub fn snap_scale(scale: f32) -> f32 {
    (scale.clamp(SCALE_RANGE.0, SCALE_RANGE.1) * 8.0).round() / 8.0
}

impl App {
    /// The voxel shader's options, from the editor view and the switches.
    pub fn shading(&self) -> crate::renderer::Shading {
        let e = &self.editor_view;
        let unit = |c: [u8; 3]| c.map(|v| v as f32 / 255.0);
        crate::renderer::Shading {
            constant: e.flat,
            fade: 0.5,
            frame_spacing: if self.view.frame { e.spacing.max(1) } else { 0 },
            background: unit(e.background),
            edge: unit(e.edge),
            grid: unit(e.grid),
            select: unit(e.select),
        }
    }

    /// Which instances are drawn faded: with fading on and the other models
    /// shown, every one but the model being edited -- and none in the Move
    /// tool, where every object is one being worked on.
    pub fn fade_key(&self) -> Option<usize> {
        (self.editor_view.fade && self.view.background && !self.moving).then_some(self.selected)
    }

    /// `view ao|flat|fade [on|off]`, `view colour bg|grid|edge|select
    /// <colour>`, `view spacing <n>`, `view scale <f>|inc|dec`.
    fn editor_view_command(&mut self, words: &[&str]) -> Option<String> {
        let e = &mut self.editor_view;
        let toggle = |slot: &mut bool, rest: &[&str]| -> Option<bool> {
            *slot = match rest {
                [] => !*slot,
                ["on"] => true,
                ["off"] => false,
                _ => return None,
            };
            Some(*slot)
        };
        let on = |b: bool| if b { "on" } else { "off" };
        Some(match words {
            ["ao", rest @ ..] => match toggle(&mut e.ao, rest) {
                Some(b) => format!("ambient occlusion {}", on(b)),
                None => "view ao [on|off]".to_owned(),
            },
            ["flat", rest @ ..] => match toggle(&mut e.flat, rest) {
                Some(b) => format!("constant shading {}", on(b)),
                None => "view flat [on|off]".to_owned(),
            },
            ["fade", rest @ ..] => match toggle(&mut e.fade, rest) {
                Some(b) => format!("fade background objects {}", on(b)),
                None => "view fade [on|off]".to_owned(),
            },
            ["colour" | "color", which, rest @ ..] => {
                let usage = "view colour bg|grid|edge|select <colour>";
                let Some(c) = crate::palette_ops::parse_colour(rest) else { return Some(usage.to_owned()) };
                let slot = match *which {
                    "bg" | "background" => &mut e.background,
                    "grid" => &mut e.grid,
                    "edge" | "edges" => &mut e.edge,
                    "select" | "selection" => &mut e.select,
                    _ => return Some(usage.to_owned()),
                };
                *slot = [c.r, c.g, c.b];
                format!("{which} colour {} {} {}", c.r, c.g, c.b)
            }
            ["spacing", n] => match n.parse::<u32>() {
                Ok(n) => {
                    e.spacing = n.clamp(1, 128);
                    format!("frame lines every {} voxels", e.spacing)
                }
                Err(_) => "view spacing <1-128>".to_owned(),
            },
            ["scale", what] => {
                let scale = match *what {
                    "inc" | "+" => e.scale + SCALE_STEP,
                    "dec" | "-" => e.scale - SCALE_STEP,
                    v => match v.parse::<f32>() {
                        Ok(v) => v,
                        Err(_) => return Some("view scale <0.5-4>|inc|dec".to_owned()),
                    },
                };
                e.scale = snap_scale(scale);
                format!("interface scale {}", e.scale)
            }
            _ => return None,
        })
    }

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
            [word, ..] if matches!(*word, "ao" | "flat" | "fade" | "colour" | "color" | "spacing" | "scale") => {
                let before = self.editor_view;
                let message = self.editor_view_command(&words).unwrap_or_default();
                if self.editor_view.ao != before.ao {
                    self.mesh_dirty = true;
                }
                message
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
                    return "view [edges|frame|grid|bg|wire|shadow] [on|off] | view ao|flat|fade [on|off] | view colour bg|grid|edge|select <colour> | view spacing <n> | view scale <f>|inc|dec".to_owned();
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

    /// Cmd+E, F, G, B and M; in the render, Cmd+E and
    /// Cmd+G switch its edges and grid instead. Cmd+minus and Cmd+plus
    /// scale the interface by eighths.
    pub fn view_input(&mut self, ctx: &egui::Context) {
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        let step = ctx.input(|i| {
            if !i.modifiers.command {
                0
            } else if i.key_pressed(egui::Key::Minus) {
                -1
            } else if i.key_pressed(egui::Key::Plus) || i.key_pressed(egui::Key::Equals) {
                1
            } else {
                0
            }
        });
        if step != 0 {
            let message = self.view_command(if step > 0 { "view scale inc" } else { "view scale dec" });
            self.console_log.push(message);
        }
        let pressed: Vec<&str> = ctx.input(|i| {
            SWITCHES
                .iter()
                .filter(|(.., key)| i.modifiers.command && !i.modifiers.shift && i.key_pressed(*key))
                .map(|(word, ..)| *word)
                .collect()
        });
        for word in pressed {
            // In the render, Cmd+E and Cmd+G are its own lines.
            let message = match word {
                "edges" if self.rendering => self.lines_command(&["edge"]),
                "grid" if self.rendering => self.lines_command(&["grid"]),
                // Its Display Background Color in the render.
                "bg" if self.rendering => self.render_background_toggle(),
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

    #[test]
    fn the_interface_scale_moves_in_eighths_within_its_limits() {
        assert_eq!(snap_scale(1.06), 1.0);
        assert_eq!(snap_scale(1.07), 1.125);
        assert_eq!(snap_scale(0.1), 0.5);
        assert_eq!(snap_scale(9.0), 4.0);
    }
}
