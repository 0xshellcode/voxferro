//! A text command console.
//!
//! Taken from the editor's own design: there, every button emits a command
//! string that the same interpreter executes, so the UI, the hotkeys and the
//! console (F2) all travel one path. Doing the same here means scripting comes
//! free rather than being bolted on later.

use glam::IVec3;
use voxel_core::{Command, Document, History, Volume};

/// Outcome of running a command line.
pub enum Outcome {
    /// Message for the log; `changed` says whether the mesh needs rebuilding.
    Ok { message: String, changed: bool },
    Error(String),
}

/// Parses and runs one command line against one model of the document.
pub fn execute(line: &str, doc: &mut Document, model: usize, history: &mut History) -> Outcome {
    let Some(volume) = doc.scene.models.get(model) else {
        return Outcome::Error("no model selected".to_owned());
    };
    let mut parts = line.split_whitespace();
    let Some(verb) = parts.next() else {
        return Outcome::Ok {
            message: String::new(),
            changed: false,
        };
    };
    let args: Vec<&str> = parts.collect();

    // Keep verbs aligned with the editor's own vocabulary where they overlap.
    match verb {
        "help" => Outcome::Ok {
            message: "model: fill <c> | clear | box <x0..z1 c> | voxels <x y z c>... | sphere <c> | gpu <shape> | \
                      rotate|mirror x|y|z | crop | resize x y z; \
                      brush [attach|erase|paint|select|move] [voxel|box|line|face|fill] | brush mirror <xyz|off> | brush pick | color <n>; \
                      select all|none|invert|color [n] | delete | paint | copy | cut | paste | \
                      move dx dy dz | transform scale axes f | transform rotate a deg; brush size n | brush tip cube|sphere|2d|3d | brush centre fill|ring|even|odd | brush project on|off | brush face color|geo|4|8|palette|voxel; palette <i> r g b | material <i> <kind> [k=v]; \
                      instance list|select i|move|place|turn|dup|delete|new; \
                      group [i ...] | ungroup | node name|hide|show|move|turn|into; layer list|new|hide|show|name|assign; \
                      anim play|stop|frame n|fps n|key|unkey|newframe|movekey a b|moveframe a b|range a b|export dir [n]; \
                      camera front|back|left|right|top|bottom|iso|ortho|persp|free|frame [all|selection|instance]|slots|save n|load n; \
                      full | zero | rev | x2 | xh | loop a n | scale axes f | shear a s t | rot a deg | repeat axes n | mir a | dia a | prim elli|cyli|pyra|cone | flood [c] | hull | dil|ero [6|18|26]; \
                      render save <file.png>; snap [file.png] | snap window [file.png]; view cube [on|off]; view [edges|frame|grid|bg|wire|shadow] [on|off]; shrink; xs [-n passes] [-c c1,c2] <script> [args] | xs list | xs use <script> [args]; \
                      pattern [selection|open f|match on|off|turn a|colours own|brush|clear]; project [list|dir p|open n|rename n|dup|delete]; palette open|save <file.png> | palette reset | palette default 0-3 | palette swap|gradient|copy a b | palette paste i; brush axis <axes> | brush region same|color|geo [4|8]; \
                      open | new | save | recover f | export [obj|ply|mc|cube|point|xraw|qb|slab|slice|iso|2d] <file> | import <file> | import <file.obj> [size] [fill] | import materials|render|cameras|palette <file.vox|file.txt> | export materials|render|cameras <file.txt>; env …; undo | redo"
                .to_owned(),
            changed: false,
        },

        "fill" => {
            let color = match parse_color(args.first()) {
                Ok(c) => c,
                Err(e) => return Outcome::Error(e),
            };
            // Only cells that are already solid, so "fill" recolours the model
            // rather than inflating it into a cube.
            let writes = all_cells(volume)
                .filter(|p| volume.get_i(p.x, p.y, p.z) != 0)
                .map(|p| (p, color))
                .collect::<Vec<_>>();
            apply(doc, model, history, writes, "filled")
        }

        "clear" => {
            let writes = all_cells(volume).map(|p| (p, 0)).collect::<Vec<_>>();
            apply(doc, model, history, writes, "cleared")
        }

        "box" => {
            if args.len() != 7 {
                return Outcome::Error("box needs x0 y0 z0 x1 y1 z1 colour".to_owned());
            }
            let nums: Result<Vec<i32>, _> =
                args[..6].iter().map(|a| a.parse::<i32>()).collect();
            let Ok(n) = nums else {
                return Outcome::Error("box coordinates must be integers".to_owned());
            };
            let color = match parse_color(args.get(6)) {
                Ok(c) => c,
                Err(e) => return Outcome::Error(e),
            };

            let (lo, hi) = (
                IVec3::new(n[0].min(n[3]), n[1].min(n[4]), n[2].min(n[5])),
                IVec3::new(n[0].max(n[3]), n[1].max(n[4]), n[2].max(n[5])),
            );
            let mut writes = Vec::new();
            for z in lo.z..=hi.z {
                for y in lo.y..=hi.y {
                    for x in lo.x..=hi.x {
                        writes.push((IVec3::new(x, y, z), color));
                    }
                }
            }
            apply(doc, model, history, writes, "box")
        }

        // Single voxels, four numbers each: x y z colour (0 erases). One
        // undo step for the whole list; what agents use to build by hand.
        "voxels" => {
            if args.is_empty() || !args.len().is_multiple_of(4) {
                return Outcome::Error("voxels needs x y z colour, repeated".to_owned());
            }
            let [sx, sy, sz] = volume.size();
            let mut writes = Vec::with_capacity(args.len() / 4);
            for v in args.chunks(4) {
                let p: Result<Vec<i32>, _> = v[..3].iter().map(|a| a.parse::<i32>()).collect();
                let Ok(p) = p else {
                    return Outcome::Error(format!("'{}' is not a voxel position", v[..3].join(" ")));
                };
                if p[0] < 0 || p[1] < 0 || p[2] < 0 || p[0] as u32 >= sx || p[1] as u32 >= sy || p[2] as u32 >= sz {
                    return Outcome::Error(format!(
                        "voxel {} {} {} is outside the {sx}x{sy}x{sz} model",
                        p[0], p[1], p[2]
                    ));
                }
                let color = match parse_color(v.get(3)) {
                    Ok(c) => c,
                    Err(e) => return Outcome::Error(e),
                };
                writes.push((IVec3::new(p[0], p[1], p[2]), color));
            }
            apply(doc, model, history, writes, "voxels")
        }

        "sphere" => {
            let color = match parse_color(args.first()) {
                Ok(c) => c,
                Err(e) => return Outcome::Error(e),
            };
            let [sx, sy, sz] = volume.size();
            let centre = IVec3::new(sx as i32 / 2, sy as i32 / 2, sz as i32 / 2);
            let radius = (sx.min(sy).min(sz) as f32) * 0.4;
            let writes = all_cells(volume)
                .filter(|p| (*p - centre).as_vec3().length() <= radius)
                .map(|p| (p, color))
                .collect::<Vec<_>>();
            apply(doc, model, history, writes, "sphere")
        }

        "undo" => {
            let changed = history.undo(doc);
            Outcome::Ok {
                message: if changed { "undone" } else { "nothing to undo" }.to_owned(),
                changed,
            }
        }

        "redo" => {
            let changed = history.redo(doc);
            Outcome::Ok {
                message: if changed { "redone" } else { "nothing to redo" }.to_owned(),
                changed,
            }
        }

        other => Outcome::Error(format!("unknown command '{other}' (try: help)")),
    }
}

fn all_cells(volume: &Volume) -> impl Iterator<Item = IVec3> + use<> {
    let [sx, sy, sz] = volume.size();
    (0..sz).flat_map(move |z| {
        (0..sy).flat_map(move |y| (0..sx).map(move |x| IVec3::new(x as i32, y as i32, z as i32)))
    })
}

fn parse_color(arg: Option<&&str>) -> Result<u8, String> {
    match arg {
        Some(a) => a
            .parse::<u8>()
            .map_err(|_| format!("'{a}' is not a palette index (0-255)")),
        None => Err("missing colour index".to_owned()),
    }
}

fn apply(
    doc: &mut Document,
    model: usize,
    history: &mut History,
    writes: Vec<(IVec3, u8)>,
    what: &str,
) -> Outcome {
    let command = Command::set_voxels(doc, model, writes);
    let count = match &command {
        Command::SetVoxels { changes, .. } => changes.len(),
        _ => 0,
    };
    // A console command is one gesture, never merged into a previous stroke.
    if !history.exec_alone(doc, command) {
        return Outcome::Ok {
            message: format!("{what}: nothing to change"),
            changed: false,
        };
    }
    Outcome::Ok {
        message: format!("{what}: {count} voxels"),
        changed: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (Document, History) {
        let mut v = Volume::new([8, 8, 8]);
        v.set(1, 1, 1, 3);
        let doc = Document {
            scene: voxel_core::Scene { models: vec![v], ..Default::default() },
            ..Default::default()
        };
        (doc, History::default())
    }

    fn run(line: &str, d: &mut Document, h: &mut History) -> Outcome {
        execute(line, d, 0, h)
    }

    fn get(d: &Document, x: u32, y: u32, z: u32) -> u8 {
        d.scene.models[0].get(x, y, z)
    }

    #[test]
    fn fill_recolours_only_solid_cells() {
        let (mut d, mut h) = setup();
        run("fill 9", &mut d, &mut h);
        assert_eq!(get(&d, 1, 1, 1), 9);
        assert_eq!(get(&d, 0, 0, 0), 0, "fill must not inflate the model");
    }

    #[test]
    fn box_writes_a_region_and_undoes_as_one_step() {
        let (mut d, mut h) = setup();
        run("box 0 0 0 2 2 2 7", &mut d, &mut h);
        assert_eq!(get(&d, 0, 0, 0), 7);
        assert_eq!(get(&d, 2, 2, 2), 7);

        h.undo(&mut d);
        assert_eq!(get(&d, 0, 0, 0), 0);
        assert_eq!(get(&d, 1, 1, 1), 3, "the original voxel should return");
    }

    #[test]
    fn unknown_and_malformed_commands_report_errors() {
        let (mut d, mut h) = setup();
        assert!(matches!(run("nope", &mut d, &mut h), Outcome::Error(_)));
        assert!(matches!(run("fill", &mut d, &mut h), Outcome::Error(_)));
        assert!(matches!(
            run("fill red", &mut d, &mut h),
            Outcome::Error(_)
        ));
        assert!(matches!(run("box 1 2", &mut d, &mut h), Outcome::Error(_)));
    }

    #[test]
    fn voxels_sets_each_cell_and_rejects_cells_outside() {
        let (mut d, mut h) = setup();
        run("voxels 0 0 0 5 7 7 7 6 1 1 1 0", &mut d, &mut h);
        assert_eq!(get(&d, 0, 0, 0), 5);
        assert_eq!(get(&d, 7, 7, 7), 6);
        assert_eq!(get(&d, 1, 1, 1), 0, "colour 0 erases");
        assert!(matches!(run("voxels 8 0 0 1", &mut d, &mut h), Outcome::Error(_)));
        assert!(matches!(run("voxels 1 2 3", &mut d, &mut h), Outcome::Error(_)));
        h.undo(&mut d);
        assert_eq!(get(&d, 1, 1, 1), 3, "one undo step for the list");
    }

    #[test]
    fn an_empty_line_is_not_an_error() {
        let (mut d, mut h) = setup();
        assert!(matches!(
            run("   ", &mut d, &mut h),
            Outcome::Ok { changed: false, .. }
        ));
    }

    #[test]
    fn clear_then_undo_restores_everything() {
        let (mut d, mut h) = setup();
        run("clear", &mut d, &mut h);
        assert_eq!(d.scene.models[0].filled_count(), 0);
        run("undo", &mut d, &mut h);
        assert_eq!(get(&d, 1, 1, 1), 3);
    }
}
