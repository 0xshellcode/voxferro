//! The palette tools:
//!
//! - **Selecting several colours**: Shift+click in the palette adds a
//!   colour or takes it out; `palette select all | none | invert | used |
//!   unused`. *Used* takes the colours of the selected voxels (or the whole
//!   model's), *unused* the colours no voxel in the scene has. With nothing
//!   selected, the tools act on the brush colour.
//! - **Sort** (`palette sort <keys>`): keys `r g b h s v`, each ascending,
//!   or descending after a `-`; one stable pass a key in the order written,
//!   so the last is the main one -- `-v-sh` orders by hue, then saturation
//!   down, then value down. Greys sort last by hue. It sorts the selected
//!   colours among their own slots, or the whole palette, and moves only
//!   the display order (`IMAP`): colours and voxels stay as they are.
//! - **Random** (`palette rand`): every voxel of the model (or of the voxel
//!   selection) takes one of the selected colours at random -- of all 255
//!   when one or none is selected.
//! - **Set** (`palette set <colour>`): every selected colour to one colour,
//!   as `#rrggbb`, `r g b`, or a grey level.
//! - **Adjust** (`palette adjust h|s|v|r|g|b <amount>`): each selected
//!   colour shifted -- hue in degrees (wrapping), saturation and value in
//!   percent, channels in 0-255 steps; greys keep no hue to shift.
//! - **Fill** (`palette fill <colour>`): every colour the same -- the New
//!   Palette button fills with 75 grey.
//! - **Copy / paste a region** (`palette copy`, `palette paste` without
//!   arguments): the selected colours by display slot, put back at the same
//!   slots, then selected.
//!
//! Each change is one undo step.

use crate::App;
use eframe::{egui, egui_wgpu};
use vox_format::Color;
use voxel_core::Command;

/// Integer HSV as the sort uses it: hue 0-359 (359 for greys),
/// saturation and value 0-255.
pub fn sort_hsv(c: Color) -> [i32; 3] {
    let (r, g, b) = (c.r as i32, c.g as i32, c.b as i32);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    if d == 0 || max == 0 {
        return [359, 0, max];
    }
    let mut h = if max == r {
        60 * (g - b) / d
    } else if max == g {
        120 + 60 * (b - r) / d
    } else {
        240 + 60 * (r - g) / d
    };
    if h < 0 {
        h += 360;
    }
    [h, 255 * d / max, max]
}

/// `slots` (display slots) reordered by `keys`:
/// one stable pass a key, in order, so the last key leads.
pub fn sorted_slots(order: &[u8; 256], slots: &[usize], colour: impl Fn(u8) -> Color, keys: &str) -> [u8; 256] {
    let mut out = *order;
    let mut values: Vec<u8> = slots.iter().map(|&s| order[s]).collect();
    let mut descending = false;
    for k in keys.to_ascii_lowercase().chars() {
        let key = match k {
            '-' => {
                descending = true;
                continue;
            }
            '+' => {
                descending = false;
                continue;
            }
            'r' => |c: Color| c.r as i32,
            'g' => |c: Color| c.g as i32,
            'b' => |c: Color| c.b as i32,
            'h' => |c: Color| sort_hsv(c)[0],
            's' => |c: Color| sort_hsv(c)[1],
            'v' => |c: Color| sort_hsv(c)[2],
            _ => continue,
        };
        // Stable, so earlier keys order the ties.
        if descending {
            values.sort_by_key(|&i| std::cmp::Reverse(key(colour(i))));
        } else {
            values.sort_by_key(|&i| key(colour(i)));
        }
        descending = false;
    }
    for (&slot, &v) in slots.iter().zip(&values) {
        out[slot] = v;
    }
    out
}

/// A colour typed: `#rrggbb` (or without `#`), `r g b`, or one grey level.
pub fn parse_colour(words: &[&str]) -> Option<Color> {
    let c = |r, g, b| Some(Color { r, g, b, a: 255 });
    match words {
        [hex] if hex.trim_start_matches('#').len() == 6 && hex.trim_start_matches('#').chars().all(|c| c.is_ascii_hexdigit()) => {
            let v = u32::from_str_radix(hex.trim_start_matches('#'), 16).ok()?;
            c((v >> 16) as u8, (v >> 8) as u8, v as u8)
        }
        [grey] => {
            let g = grey.parse::<u32>().ok()?.min(255) as u8;
            c(g, g, g)
        }
        [r, g, b] => c(r.parse::<u32>().ok()?.min(255) as u8, g.parse::<u32>().ok()?.min(255) as u8, b.parse::<u32>().ok()?.min(255) as u8),
        _ => None,
    }
}

/// RGB to HSV: hue 0-360, saturation and value 0-1.
pub fn to_hsv(c: Color) -> [f32; 3] {
    let [r, g, b] = [c.r, c.g, c.b].map(|v| v as f32 / 255.0);
    let max = r.max(g).max(b);
    let d = max - r.min(g).min(b);
    let h = if d == 0.0 {
        0.0
    } else if max == r {
        60.0 * ((g - b) / d).rem_euclid(6.0)
    } else if max == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    [h, if max == 0.0 { 0.0 } else { d / max }, max]
}

pub fn from_hsv([h, s, v]: [f32; 3]) -> Color {
    let c = v * s;
    let x = c * (1.0 - ((h / 60.0).rem_euclid(2.0) - 1.0).abs());
    let (r, g, b) = match (h.rem_euclid(360.0) / 60.0) as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    let f = |u: f32| ((u + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    Color { r: f(r), g: f(g), b: f(b), a: 255 }
}

/// `c` shifted on one channel.
pub fn adjusted(c: Color, channel: char, amount: f32) -> Color {
    let clamp = |v: f32| v.round().clamp(0.0, 255.0) as u8;
    match channel {
        'r' => Color { r: clamp(c.r as f32 + amount), ..c },
        'g' => Color { g: clamp(c.g as f32 + amount), ..c },
        'b' => Color { b: clamp(c.b as f32 + amount), ..c },
        _ => {
            let [h, s, v] = to_hsv(c);
            let grey = s == 0.0;
            let hsv = match channel {
                // A grey has no hue to turn, nor saturation to change.
                'h' if !grey => [(h + amount).rem_euclid(360.0), s, v],
                's' if !grey => [h, (s + amount / 100.0).clamp(0.0, 1.0), v],
                'v' => [h, s, (v + amount / 100.0).clamp(0.0, 1.0)],
                _ => return c,
            };
            Color { a: c.a, ..from_hsv(hsv) }
        }
    }
}

impl App {
    /// The colours the palette tools act on: the selection, or the brush
    /// colour.
    pub fn palette_targets(&self) -> Vec<u8> {
        if self.palette_selection.is_empty() { vec![self.brush_color] } else { self.palette_selection.clone() }
    }

    /// Shift+click: a colour in or out of the selection.
    pub fn toggle_palette_colour(&mut self, index: u8) {
        if index == 0 {
            return;
        }
        if self.palette_selection.is_empty() && self.brush_color != index {
            self.palette_selection.push(self.brush_color);
        }
        match self.palette_selection.iter().position(|&i| i == index) {
            Some(at) => {
                self.palette_selection.remove(at);
            }
            None => self.palette_selection.push(index),
        }
        self.palette_selection.sort_unstable();
    }

    /// Sets colours, one undo step.
    fn set_colours(&mut self, changes: Vec<(u8, Color)>, what: &str, state: &egui_wgpu::RenderState) -> String {
        let Some(loaded) = &self.scene else { return "nothing open".to_owned() };
        let commands: Vec<Command> = changes
            .into_iter()
            .filter(|&(i, c)| i != 0 && loaded.doc.palette.color(i) != c)
            .map(|(index, after)| Command::SetColor { index, before: loaded.doc.palette.color(index), after })
            .collect();
        let n = commands.len();
        let message = self.palette_batch(commands, what, state);
        if message.ends_with("nothing changed") { message } else { format!("{what}: {n} colours") }
    }

    /// The palette tools' commands; `None`
    /// for the ones `palette_command` keeps.
    pub fn palette_tool_command(&mut self, words: &[&str], state: &egui_wgpu::RenderState) -> Option<String> {
        let Some(loaded) = &self.scene else { return Some("nothing open".to_owned()) };
        let doc = &loaded.doc;
        Some(match words {
            ["select", what] => {
                let used: Vec<bool> = {
                    let mut used = vec![false; 256];
                    match (*what, self.active_selection()) {
                        ("used", Some(sel)) => {
                            if let Some(v) = doc.scene.models.get(sel.model) {
                                for p in sel.iter() {
                                    used[v.get_i(p.x, p.y, p.z) as usize] = true;
                                }
                            }
                        }
                        _ => {
                            let models: Vec<usize> = if *what == "used" {
                                self.selected_model().into_iter().collect()
                            } else {
                                (0..doc.scene.models.len()).collect()
                            };
                            for m in models {
                                for &v in doc.scene.models[m].cells() {
                                    used[v as usize] = true;
                                }
                            }
                        }
                    }
                    used
                };
                let all: Vec<u8> = (1..=255).collect();
                let current = self.palette_targets();
                self.palette_selection = match *what {
                    "all" => all,
                    "none" => Vec::new(),
                    "invert" => all.into_iter().filter(|i| !current.contains(i)).collect(),
                    "used" => all.into_iter().filter(|&i| used[i as usize]).collect(),
                    "unused" => all.into_iter().filter(|&i| !used[i as usize]).collect(),
                    _ => return Some("palette select all|none|invert|used|unused".to_owned()),
                };
                format!("{} colours selected", self.palette_targets().len())
            }
            ["sort", keys] => {
                let order = doc.palette_order.clone();
                let targets = self.palette_targets();
                let slots: Vec<usize> = if targets.len() > 1 {
                    let mut s: Vec<usize> = targets.iter().filter_map(|&i| order.iter().position(|&o| o == i)).collect();
                    s.sort_unstable();
                    s
                } else {
                    (1..256).collect()
                };
                let after = Box::new(sorted_slots(&order, &slots, |i| doc.palette.color(i), keys));
                let n = slots.len();
                let message = self.palette_batch(vec![Command::SetPaletteOrder { before: order, after }], "sort", state);
                if message == "sort" { format!("sorted {n} colours by {keys}") } else { message }
            }
            ["rand"] => {
                let targets = self.palette_targets();
                let list: Vec<u8> = if targets.len() > 1 { targets } else { (1..=255).collect() };
                let mut rand = voxel_core::kernels::Rand::from_clock();
                let Some(model) = self.selected_model() else { return Some("no model".to_owned()) };
                let mut after = doc.scene.models[model].clone();
                let [sx, sy, sz] = after.size();
                for z in 0..sz {
                    for y in 0..sy {
                        for x in 0..sx {
                            if after.get(x, y, z) != 0 {
                                after.set(x, y, z, list[rand.next_u32() as usize % list.len()]);
                            }
                        }
                    }
                }
                self.recolour_model(model, after, "rand")
            }
            ["set", rest @ ..] => match parse_colour(rest) {
                Some(c) => {
                    let changes = self.palette_targets().into_iter().map(|i| (i, c)).collect();
                    self.set_colours(changes, "set", state)
                }
                None => "palette set #rrggbb | r g b | grey".to_owned(),
            },
            ["adjust", channel, amount] => match (channel.chars().next().filter(|c| "rgbhsv".contains(*c)), amount.parse::<f32>()) {
                (Some(ch), Ok(a)) if channel.len() == 1 => {
                    let changes = self.palette_targets().into_iter().map(|i| (i, adjusted(doc.palette.color(i), ch, a))).collect();
                    self.set_colours(changes, "adjust", state)
                }
                _ => "palette adjust h|s|v|r|g|b <amount>".to_owned(),
            },
            ["fill", rest @ ..] => match parse_colour(rest) {
                Some(c) => {
                    let changes = (1..=255u8).map(|i| (i, c)).collect();
                    self.set_colours(changes, "fill", state)
                }
                None => "palette fill #rrggbb | r g b | grey".to_owned(),
            },
            ["copy"] => {
                let order = &doc.palette_order;
                self.palette_region = self
                    .palette_targets()
                    .into_iter()
                    .filter_map(|i| Some((order.iter().position(|&o| o == i)?, doc.palette.color(i))))
                    .collect();
                format!("copied {} colours", self.palette_region.len())
            }
            ["paste"] => {
                if self.palette_region.is_empty() {
                    return Some("nothing copied: palette copy first".to_owned());
                }
                let order = doc.palette_order.clone();
                let changes: Vec<(u8, Color)> = self.palette_region.iter().map(|&(slot, c)| (order[slot], c)).collect();
                self.palette_selection = changes.iter().map(|&(i, _)| i).filter(|&i| i != 0).collect();
                self.palette_selection.sort_unstable();
                self.set_colours(changes, "paste", state)
            }
            _ => return None,
        })
    }

    /// Recolours a model to `after` (the same size), within the voxel
    /// selection if there is one, as one undo step.
    fn recolour_model(&mut self, model: usize, after: voxel_core::Volume, what: &str) -> String {
        let only = self.active_selection().filter(|s| s.model == model).map(|s| s.cells().clone());
        let Some(loaded) = self.scene.as_mut() else { return "nothing open".to_owned() };
        let command = Command::diff(&loaded.doc, model, &after, |p| only.as_ref().is_none_or(|c| c.contains(&p.to_array())));
        let n = match &command {
            Command::SetVoxels { changes, .. } => changes.len(),
            _ => 0,
        };
        self.history.exec_alone(&mut loaded.doc, command);
        self.mesh_dirty = true;
        format!("{what}: {n} voxels recoloured")
    }

    /// Hex and HSV fields under the colour editor: they change every
    /// selected colour, by the amount the brush colour's field moves.
    pub fn palette_tools_ui(&mut self, ui: &mut egui::Ui, state: &egui_wgpu::RenderState) {
        let Some(loaded) = &self.scene else { return };
        let colour = loaded.doc.palette.color(self.brush_color);
        let n = self.palette_targets().len();
        let mut lines: Vec<String> = Vec::new();
        ui.horizontal(|ui| {
            ui.label("Hex");
            let id = egui::Id::new("palette hex");
            let mut text = ui.data(|d| d.get_temp::<String>(id)).unwrap_or_else(|| format!("{:02x}{:02x}{:02x}", colour.r, colour.g, colour.b));
            let r = ui.add(egui::TextEdit::singleline(&mut text).desired_width(64.0).font(egui::TextStyle::Monospace));
            if r.has_focus() {
                ui.data_mut(|d| d.insert_temp(id, text.clone()));
            } else {
                ui.data_mut(|d| d.remove::<String>(id));
            }
            if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                lines.push(format!("palette set {text}"));
            }
            ui.label(egui::RichText::new(format!("{n} selected")).weak().size(10.5));
        });
        let [h, s, v] = to_hsv(colour);
        ui.horizontal(|ui| {
            for (label, value, unit, channel) in [("H", h, 1.0, 'h'), ("S", s * 100.0, 1.0, 's'), ("V", v * 100.0, 1.0, 'v')] {
                ui.label(label);
                let mut x = value.round();
                let r = ui.add(egui::DragValue::new(&mut x).speed(unit).range(0.0..=if channel == 'h' { 359.0 } else { 100.0 }));
                if r.drag_started() {
                    self.history.begin_group();
                }
                if r.changed() && x != value.round() {
                    lines.push(format!("palette adjust {channel} {}", x - value.round()));
                }
                if r.drag_stopped() {
                    self.history.end_group();
                }
            }
        });
        ui.horizontal(|ui| {
            for (label, line, hint) in [
                ("All", "palette select all", "Select every colour"),
                ("Inv", "palette select invert", "Select the other colours"),
                ("Used", "palette select used", "The colours of the selected voxels, or the model's"),
                ("Unused", "palette select unused", "The colours no voxel has"),
            ] {
                if ui.small_button(label).on_hover_text(hint).clicked() {
                    lines.push(line.to_owned());
                }
            }
        });
        ui.horizontal(|ui| {
            for (label, line, hint) in [
                ("Sort", "palette sort -v-sh", "By hue, then saturation and value down (only the order shown changes)"),
                ("Rand", "palette rand", "Paint the model's voxels with the selected colours at random"),
                ("Copy", "palette copy", "Copy the selected colours"),
                ("Paste", "palette paste", "Paste them back at the same places"),
            ] {
                if ui.small_button(label).on_hover_text(hint).clicked() {
                    lines.push(line.to_owned());
                }
            }
        });
        for line in lines {
            let message = self.palette_command(&line, state);
            self.console_log.push(message);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgb(r: u8, g: u8, b: u8) -> Color {
        Color { r, g, b, a: 255 }
    }

    #[test]
    fn sorting_keeps_colours_and_moves_slots_with_the_last_key_leading() {
        let colours = [rgb(0, 0, 0), rgb(10, 10, 10), rgb(255, 0, 0), rgb(0, 0, 255), rgb(0, 255, 0), rgb(128, 0, 0)];
        let colour = |i: u8| colours.get(i as usize).copied().unwrap_or(rgb(0, 0, 0));
        let order: [u8; 256] = std::array::from_fn(|k| k as u8);
        let out = sorted_slots(&order, &[1, 2, 3, 4, 5], colour, "-v-sh");
        // Hue first: red 0 (bright before dark), green 120, blue 240, grey last.
        assert_eq!(&out[1..6], &[2, 5, 4, 3, 1]);
        assert_eq!(out[0], 0, "slots outside the sort stay");
        assert_eq!(sort_hsv(rgb(10, 10, 10))[0], 359, "greys sort last by hue");
    }

    #[test]
    fn colours_parse_and_adjust() {
        assert_eq!(parse_colour(&["#ff8000"]), Some(rgb(255, 128, 0)));
        assert_eq!(parse_colour(&["75"]), Some(rgb(75, 75, 75)));
        assert_eq!(parse_colour(&["1", "2", "300"]), Some(rgb(1, 2, 255)));
        assert_eq!(parse_colour(&["#zz0000"]), None);
        let red = rgb(255, 0, 0);
        assert_eq!(adjusted(red, 'h', 120.0), rgb(0, 255, 0));
        assert_eq!(adjusted(red, 'v', -50.0), rgb(128, 0, 0));
        assert_eq!(adjusted(rgb(100, 100, 100), 'h', 90.0), rgb(100, 100, 100), "greys have no hue");
        assert_eq!(adjusted(red, 'g', 300.0), rgb(255, 255, 0));
        for c in [rgb(12, 200, 99), rgb(255, 255, 255), rgb(3, 4, 250)] {
            assert_eq!(from_hsv(to_hsv(c)), c);
        }
    }
}
