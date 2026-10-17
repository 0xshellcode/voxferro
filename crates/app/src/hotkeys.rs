//! the reference editor's model-editor shortcuts, from its `config/hotkey.txt`
//! (a recent version), so hands that know it work here too. Ctrl there is Cmd on
//! macOS, as for every shortcut here.
//!
//! | Keys | Does |
//! |---|---|
//! | V F B L C P | brush shape: voxel, face, box, line, shader, pattern |
//! | T R G | brush mode: attach, erase, paint |
//! | M N | select: marquee (box), region (fill) |
//! | 1 2 3 | mirror x, y, z on or off |
//! | Cmd+1 2 3 | axis x, y, z on or off |
//! | U | full: the box filled with the colour |
//! | I | fill: every voxel repainted with the colour |
//! | Cmd+I | invert the selection |
//! | Delete, Backspace | the selection deleted; with none, nothing (a hint) |
//! | 5 | frame the selection or the model (Shift+F everything) |
//! | 6 | snapshot of the viewport |
//! | 7 8 | save the camera to the slot, load it |
//! | , . | previous, next animation frame |
//! | F5 F6 | add, remove an animation key |
//! | Alt+click | take the voxel's colour (Alt+Shift+click picks a group) |
//!
//! Where these took a key this editor had used, the editor's moved: the
//! fixed views are Alt+1 to Alt+7, the eyedropper Alt+click, and framing
//! 5 as in the reference editor. Keys do nothing while a text field has focus, and
//! the letters only without Cmd, so Cmd+B and the like stay the display
//! switches.

use crate::App;
use eframe::{egui, egui_wgpu};

impl App {
    pub fn hotkeys(&mut self, ctx: &egui::Context, state: &egui_wgpu::RenderState) {
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        use egui::Key;
        let mut lines: Vec<String> = Vec::new();
        let mut edits: Vec<&str> = Vec::new();
        let mut snap = false;
        let mut hint = false;
        ctx.input(|i| {
            let (cmd, shift, alt) = (i.modifiers.command, i.modifiers.shift, i.modifiers.alt);
            let plain = !cmd && !alt;
            let pressed = |k: Key| i.key_pressed(k);
            if plain && !shift {
                // In the free camera, W A S D Q E fly; none of these clash.
                for (key, line) in [
                    (Key::V, "brush voxel"),
                    (Key::F, "brush face"),
                    (Key::B, "brush box"),
                    (Key::L, "brush line"),
                    (Key::C, "brush shader"),
                    (Key::P, "brush pattern"),
                    (Key::T, "brush attach"),
                    (Key::R, "brush erase"),
                    (Key::G, "brush paint"),
                    (Key::M, "brush select box"),
                    (Key::N, "brush select fill"),
                ] {
                    if pressed(key) {
                        lines.push(line.to_owned());
                    }
                }
                if pressed(Key::U) {
                    edits.push("full");
                }
                if pressed(Key::I) {
                    edits.push("fill");
                }
                if pressed(Key::Num5) {
                    lines.push("camera frame".to_owned());
                }
                if pressed(Key::Num6) {
                    snap = true;
                }
                if pressed(Key::Num7) {
                    lines.push(format!("camera save {}", self.camera_slot));
                }
                if pressed(Key::Num8) {
                    lines.push(format!("camera load {}", self.camera_slot));
                }
                if pressed(Key::Comma) {
                    lines.push(format!("anim frame {}", self.frame().saturating_sub(1)));
                }
                if pressed(Key::Period) {
                    lines.push(format!("anim frame {}", self.frame() + 1));
                }
                if pressed(Key::F5) {
                    lines.push("anim key".to_owned());
                }
                if pressed(Key::F6) {
                    lines.push("anim unkey".to_owned());
                }
            }
            for (k, key) in [Key::Num1, Key::Num2, Key::Num3].into_iter().enumerate() {
                if pressed(key) && !alt && !shift {
                    if cmd {
                        self.axis_lock[k] = !self.axis_lock[k];
                    } else {
                        self.mirror[k] = !self.mirror[k];
                    }
                }
            }
            if cmd && !shift && pressed(Key::I) {
                lines.push("select invert".to_owned());
            }
            // the reference editor clears the whole model here. Not this editor: a
            // selection dropped without noticing, then Delete, would empty
            // the model when a piece was meant. Clear in the Edit panel
            // (`zero`) does it on purpose.
            // (In the Move tool, Delete removes the selected objects.)
            if !cmd && !self.moving && (pressed(Key::Delete) || pressed(Key::Backspace)) && self.active_selection().is_none() {
                hint = true;
            }
        });
        for line in lines {
            let first = line.split_whitespace().next();
            let message = match first {
                Some("brush") => self.brush_command(&line),
                Some("camera") => self.camera_command(&line),
                Some("select") => self.select_command(&line, ctx),
                _ => {
                    self.run_command(&line, ctx, state);
                    continue;
                }
            };
            self.console_log.push(message);
        }
        for verb in edits {
            let message = self.edit_command(verb);
            self.console_log.push(message);
        }
        if hint {
            self.console_log.push("nothing selected; Clear in the Edit panel empties the model".to_owned());
        }
        if snap {
            let message = self.snap(None, state);
            self.console_log.push(message);
        }
    }
}
