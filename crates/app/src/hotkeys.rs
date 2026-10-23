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
//! | , . | previous, next animation frame (wrapping in the range) |
//! | Cmd+, Cmd+. | previous, next key; with Shift, first and last |
//! | F5 F6 | add, remove an animation key |
//! | Alt+click | take the voxel's colour (Alt+Shift+click picks a group) |
//! | Tab, Shift+Tab | into and out of a group (in `objects.rs`) |
//! | Cmd+E F G B M | display switches (in `view_options.rs`); in the render, Cmd+E and G its edges and grid |
//! | Cmd+minus, Cmd+plus | interface scale, in eighths (in `view_options.rs`) |
//! | F1 | the console takes the keyboard |
//! | F2 | between the editor and the render |
//! | Cmd+P, Cmd+Shift+P | a new project; a copy of this one in its folder |
//!
//! In the Move tool, the reference editor's world editor (in `objects.rs`):
//!
//! | Keys | Does |
//! |---|---|
//! | arrows, Page Up, Page Down | move the objects a voxel along x, y, z |
//! | - +, 0 9 | a quarter turn about z, about x, either way |
//! | G | move the objects down to the ground |
//! | Cmd+R, Cmd+Shift+R | group the objects; dissolve the selected group |
//! | Cmd+C, Cmd+Shift+C, Cmd+X, Cmd+V | copy, copy as references, cut, paste |
//! | Cmd+T | a reference made a model of its own |
//! | Cmd+A, Cmd+I, Cmd+D, Delete | select all, invert, none; delete |
//!
//! In the render: Cmd+E and Cmd+G its lines, Cmd+U the ground, Cmd+B the
//! background colour, Cmd+C and Cmd+V the brush colour's material, Cmd+R
//! that material back to diffuse (in `view_options.rs`).
//!
//! Not taken: Cmd+H and Cmd+W, which hide and close the app on macOS;
//! Ctrl+5, Ctrl+6 and F12, whose actions the editor's files do not name;
//! the camera slots on the keypad, which egui cannot tell from the digits.
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
            // Global, as the reference editor's: F1 the console, F2 between the
            // editor and the render, Cmd+P a new project and Cmd+Shift+P a
            // copy of it.
            if pressed(Key::F1) {
                self.focus_console = true;
            }
            if pressed(Key::F2) {
                lines.push(if self.rendering { "mode model" } else { "mode render" }.to_owned());
            }
            // (Cmd+P alone, a new project, is with Cmd+N in `file_input`.)
            if cmd && shift && pressed(Key::P) {
                lines.push("project dup".to_owned());
            }
            // The Move tool is the reference editor's world editor, with keys of its
            // own (`object_input`): the model editor's letters stay out.
            let model_keys = !self.moving && !self.rendering;
            if plain && !shift && model_keys {
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
            }
            if plain && !shift {
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
                    lines.push("anim step -1".to_owned());
                }
                if pressed(Key::Period) {
                    lines.push("anim step 1".to_owned());
                }
                if pressed(Key::F5) {
                    lines.push("anim key".to_owned());
                }
                if pressed(Key::F6) {
                    lines.push("anim unkey".to_owned());
                }
            }
            for (k, key) in [Key::Num1, Key::Num2, Key::Num3].into_iter().enumerate() {
                if pressed(key) && !alt && !shift && model_keys {
                    if cmd {
                        self.axis_lock[k] = !self.axis_lock[k];
                    } else {
                        self.mirror[k] = !self.mirror[k];
                    }
                }
            }
            // Cmd+, and Cmd+. a key back and on; with Shift, the first and
            // the last key.
            if cmd && !alt && (pressed(Key::Comma) || pressed(Key::Period)) {
                let back = pressed(Key::Comma);
                let which = match (back, shift) {
                    (true, false) => "prev",
                    (false, false) => "next",
                    (true, true) => "first",
                    (false, true) => "last",
                };
                lines.push(format!("anim key {which}"));
            }
            if cmd && !shift && pressed(Key::I) && model_keys {
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
                Some("mode") => {
                    self.rendering = line.ends_with("render");
                    self.path_tracer.reset();
                    if self.rendering { "render" } else { "model editor" }.to_owned()
                }
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
