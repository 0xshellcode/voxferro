//! The `xs` command and the Scripts panel (see `xs` for the scripts
//! themselves).
//!
//! `xs [-n passes] [-c colours] <script> [arg...]` runs a script over the
//! selected model, like the reference editor's command of the same name:
//!
//! - `<script>` is a name from the script folders, or a path to a file;
//! - the args fill the script's header arguments in order, the rest keeping
//!   their defaults;
//! - `-n` runs that many passes, each reading the last;
//! - `-c 12,40,41` sets the colour selection `color_sel` reads; without it,
//!   the selection is the current colour alone.
//!
//! `xs list` names the scripts found. The result is one undoable step, and
//! with voxels selected on that model only they change.

use crate::xs::{self, Script, XsRunner};
use crate::App;
use eframe::{egui, egui_wgpu};

pub struct XsState {
    runner: XsRunner,
    /// Scripts found, by name; filled on first use and by Refresh.
    found: Option<Vec<(String, std::path::PathBuf)>>,
    /// The panel's script and its argument values.
    chosen: Option<Script>,
    values: Vec<f32>,
    passes: u32,
}

impl XsState {
    pub fn new(device: &wgpu::Device) -> Self {
        Self {
            runner: XsRunner::new(device),
            found: None,
            chosen: None,
            values: Vec::new(),
            passes: 1,
        }
    }

    fn found(&mut self) -> &[(String, std::path::PathBuf)] {
        self.found.get_or_insert_with(|| xs::list(&crate::script_dirs()))
    }

    fn choose(&mut self, script: Script) {
        self.values = script.args.iter().map(|a| a.value).collect();
        self.chosen = Some(script);
    }
}

/// What a command line asks for.
#[derive(Debug, PartialEq)]
struct Request {
    script: String,
    passes: u32,
    colours: Option<Vec<u8>>,
    args: Vec<f32>,
}

fn parse(line: &str) -> Result<Request, String> {
    let usage = "xs [-n passes] [-c c1,c2,...] <script> [arg...], or xs list";
    let mut words = line.split_whitespace().skip(1);
    let mut passes = 1;
    let mut colours = None;
    let script = loop {
        match words.next() {
            Some("-n") => {
                passes = words
                    .next()
                    .and_then(|n| n.parse::<u32>().ok())
                    .filter(|n| (1..=256).contains(n))
                    .ok_or("-n takes a number of passes, 1 to 256")?;
            }
            Some("-c") => {
                let list = words.next().ok_or("-c takes colours, such as 12,40,41")?;
                let parsed: Result<Vec<u8>, _> = list.split(',').map(|c| c.trim().parse::<u8>()).collect();
                colours = Some(parsed.ok().filter(|c| !c.is_empty()).ok_or("-c takes colours 0 to 255, such as 12,40,41")?);
            }
            Some(word) => break word.to_owned(),
            None => return Err(usage.to_owned()),
        }
    };
    let args = words
        .map(|w| w.parse::<f32>().map_err(|_| format!("'{w}' is not a number")))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Request {
        script,
        passes,
        colours,
        args,
    })
}

/// A seed that differs between runs, for `i_rand`. Nothing here needs more
/// than "not the same as last time".
fn random4() -> [f32; 4] {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
    let bits = h.finish();
    [0, 16, 32, 48].map(|shift| ((bits >> shift) & 0xffff) as f32 / 65536.0)
}

impl App {
    pub fn xs_command(&mut self, line: &str, state: &egui_wgpu::RenderState) -> String {
        if line.split_whitespace().nth(1) == Some("list") {
            self.xs.found = None;
            let names: Vec<String> = self.xs.found().iter().map(|(n, _)| n.clone()).collect();
            if names.is_empty() {
                return "no scripts found".to_owned();
            }
            return format!("scripts: {}", names.join(", "));
        }
        let request = match parse(line) {
            Ok(r) => r,
            Err(e) => return format!("error: {e}"),
        };
        let Some(path) = xs::find(&request.script, &crate::script_dirs()) else {
            return format!("error: no script '{}' (xs list names them)", request.script);
        };
        let script = match Script::load(&path) {
            Ok(s) => s,
            Err(e) => return format!("error: {e}"),
        };
        if request.args.len() > script.args.len() {
            return format!(
                "error: {} takes {} argument(s){}",
                script.name,
                script.args.len(),
                if script.args.is_empty() {
                    String::new()
                } else {
                    format!(": {}", script.args.iter().map(|a| a.name.as_str()).collect::<Vec<_>>().join(", "))
                }
            );
        }
        let mut values: Vec<f32> = script.args.iter().map(|a| a.value).collect();
        values[..request.args.len()].copy_from_slice(&request.args);
        self.run_script(&script, &values, request.passes, request.colours.as_deref(), state)
    }

    /// Runs `script` over the selected model as one undoable step.
    fn run_script(
        &mut self,
        script: &Script,
        values: &[f32],
        passes: u32,
        colours: Option<&[u8]>,
        state: &egui_wgpu::RenderState,
    ) -> String {
        let model = self.selected_model();
        let only: Option<std::collections::BTreeSet<[i32; 3]>> = self
            .active_selection()
            .filter(|s| Some(s.model) == model)
            .map(|s| s.cells().clone());
        let (Some(loaded), Some(model)) = (self.scene.as_mut(), model) else {
            return "error: no model selected".to_owned();
        };
        let volume = &loaded.doc.scene.models[model];
        let [sx, sy, sz] = volume.size();

        let colours = colours.map(<[u8]>::to_vec).unwrap_or_else(|| vec![self.brush_color]);
        let (first, last) = loaded.anim_range();
        let mut params = xs::Params {
            volume_size: [sx as f32, sy as f32, sz as f32],
            color_index: self.brush_color as f32,
            mirror: self.mirror.map(|m| m as u8 as f32),
            num_color_sels: colours.len() as i32,
            // the reference editor's axis buttons; this editor has none yet.
            axis: [0.0; 3],
            frame: loaded.doc.scene.frame.saturating_sub(first) as f32,
            rand: random4(),
            num_frames: (last.saturating_sub(first) + 1) as f32,
            iter: 0.0,
            _pad: [0.0; 2],
            args: [0.0; 16],
            color_sels: [0.0; 256],
            palette: [[0.0; 4]; 256],
        };
        params.args[..values.len()].copy_from_slice(values);
        for (slot, c) in params.color_sels.iter_mut().zip(&colours) {
            *slot = *c as f32;
        }
        for (i, slot) in params.palette.iter_mut().enumerate() {
            let c = loaded.doc.palette.color(i as u8);
            *slot = [c.r, c.g, c.b, c.a].map(|v| v as f32 / 255.0);
        }

        let started = std::time::Instant::now();
        let result = match self.xs.runner.run(&state.device, &state.queue, volume, script, &params, passes) {
            Ok(r) => r,
            Err(e) => return format!("error: {e}"),
        };
        let elapsed = started.elapsed();

        let mut writes = Vec::new();
        let mut i = 0usize;
        for z in 0..sz {
            for y in 0..sy {
                for x in 0..sx {
                    let new = result[i];
                    i += 1;
                    let at = [x as i32, y as i32, z as i32];
                    if volume.get(x, y, z) != new && only.as_ref().is_none_or(|cells| cells.contains(&at)) {
                        writes.push((glam::IVec3::from_array(at), new));
                    }
                }
            }
        }
        let count = writes.len();
        self.history.end_gesture();
        if count > 0 {
            self.history.set_voxels(&mut loaded.doc, model, writes);
            self.history.end_gesture();
            self.mesh_dirty = true;
        }
        let passes = if passes > 1 { format!(", {passes} passes") } else { String::new() };
        format!("xs {}: {count} voxels changed in {elapsed:.1?}{passes}", script.name)
    }

    pub fn scripts_ui(&mut self, ui: &mut egui::Ui, state: &egui_wgpu::RenderState) {
        let names: Vec<(String, std::path::PathBuf)> = self.xs.found().to_vec();
        let current = self.xs.chosen.as_ref().map(|s| s.name.clone());
        let mut pick = None;
        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt("xs script")
                .selected_text(current.as_deref().unwrap_or("Choose…"))
                .show_ui(ui, |ui| {
                    for (name, path) in &names {
                        let label = ui
                            .selectable_label(current.as_deref() == Some(name.as_str()), name)
                            .on_hover_text(path.display().to_string());
                        if label.clicked() {
                            pick = Some(path.clone());
                        }
                    }
                });
            if ui.small_button("Refresh").on_hover_text("Look for scripts again, and reload this one").clicked() {
                self.xs.found = None;
                pick = self.xs.chosen.as_ref().map(|s| s.path.clone());
            }
        });
        if let Some(path) = pick {
            match Script::load(&path) {
                Ok(script) => {
                    // Reloading the same script keeps the values set.
                    let keep = self.xs.chosen.as_ref().is_some_and(|s| s.path == script.path && s.args == script.args);
                    if keep {
                        self.xs.chosen = Some(script);
                    } else {
                        self.xs.choose(script);
                    }
                }
                Err(e) => self.console_log.push(format!("error: {e}")),
            }
        }

        let Some(script) = self.xs.chosen.clone() else {
            if names.is_empty() {
                ui.label("No scripts found.");
            }
            return;
        };
        for (arg, value) in script.args.iter().zip(self.xs.values.iter_mut()) {
            let (lo, hi) = arg.range;
            let slider = egui::Slider::new(value, lo.min(hi)..=hi.max(lo))
                .text(&arg.name)
                .fixed_decimals(arg.precision);
            let slider = if arg.step > 0.0 { slider.step_by(arg.step as f64) } else { slider };
            ui.add(slider);
        }
        let mut run = false;
        ui.horizontal(|ui| {
            ui.add(egui::DragValue::new(&mut self.xs.passes).range(1..=256).prefix("passes "));
            run = ui
                .button("Run")
                .on_hover_text("On the selected model; with voxels selected, only those change")
                .clicked();
        });
        if run {
            let values = self.xs.values.clone();
            let message = self.run_script(&script, &values, self.xs.passes, None, state);
            self.console_log.push(message);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_flags_script_and_arguments() {
        assert_eq!(
            parse("xs -n 3 -c 1,2,40 gear 12 0.5").unwrap(),
            Request {
                script: "gear".to_owned(),
                passes: 3,
                colours: Some(vec![1, 2, 40]),
                args: vec![12.0, 0.5],
            }
        );
        assert_eq!(parse("xs sphere").unwrap().passes, 1);
        assert!(parse("xs").is_err());
        assert!(parse("xs -n 0 grow").is_err());
        assert!(parse("xs -c 300 grow").is_err());
        assert!(parse("xs gear twelve").is_err());
    }
}
