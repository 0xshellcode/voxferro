//! A text command console.
//!
//! Taken from the editor's own design: there, every button emits a command
//! string that the same interpreter executes, so the UI, the hotkeys and the
//! console (F2) all travel one path. Doing the same here means scripting comes
//! free rather than being bolted on later.

use glam::IVec3;
use voxel_core::{Command, History, Volume};

/// Outcome of running a command line.
pub enum Outcome {
    /// Message for the log; `changed` says whether the mesh needs rebuilding.
    Ok { message: String, changed: bool },
    Error(String),
}

/// Parses and runs one command line against a volume.
pub fn execute(line: &str, volume: &mut Volume, history: &mut History) -> Outcome {
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
            message: "fill <c> | clear | box <x0..z1 c> | sphere <c> | undo | redo | \
                      save [path] | gpu <sphere|torus|box|shell>"
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
            apply(volume, history, writes, "filled")
        }

        "clear" => {
            let writes = all_cells(volume).map(|p| (p, 0)).collect::<Vec<_>>();
            apply(volume, history, writes, "cleared")
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
            apply(volume, history, writes, "box")
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
            apply(volume, history, writes, "sphere")
        }

        "undo" => {
            let changed = history.undo(volume);
            Outcome::Ok {
                message: if changed { "undone" } else { "nothing to undo" }.to_owned(),
                changed,
            }
        }

        "redo" => {
            let changed = history.redo(volume);
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
    volume: &mut Volume,
    history: &mut History,
    writes: Vec<(IVec3, u8)>,
    what: &str,
) -> Outcome {
    let command = Command::set_voxels(volume, writes);
    if command.is_empty() {
        return Outcome::Ok {
            message: format!("{what}: nothing to change"),
            changed: false,
        };
    }
    let count = match &command {
        Command::SetVoxels { changes } => changes.len(),
    };
    // A console command is one gesture, never merged into a previous stroke.
    history.end_gesture();
    history.exec(volume, command);
    history.end_gesture();
    Outcome::Ok {
        message: format!("{what}: {count} voxels"),
        changed: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (Volume, History) {
        let mut v = Volume::new([8, 8, 8]);
        v.set(1, 1, 1, 3);
        (v, History::default())
    }

    #[test]
    fn fill_recolours_only_solid_cells() {
        let (mut v, mut h) = setup();
        execute("fill 9", &mut v, &mut h);
        assert_eq!(v.get(1, 1, 1), 9);
        assert_eq!(v.get(0, 0, 0), 0, "fill must not inflate the model");
    }

    #[test]
    fn box_writes_a_region_and_undoes_as_one_step() {
        let (mut v, mut h) = setup();
        execute("box 0 0 0 2 2 2 7", &mut v, &mut h);
        assert_eq!(v.get(0, 0, 0), 7);
        assert_eq!(v.get(2, 2, 2), 7);

        h.undo(&mut v);
        assert_eq!(v.get(0, 0, 0), 0);
        assert_eq!(v.get(1, 1, 1), 3, "the original voxel should return");
    }

    #[test]
    fn unknown_and_malformed_commands_report_errors() {
        let (mut v, mut h) = setup();
        assert!(matches!(execute("nope", &mut v, &mut h), Outcome::Error(_)));
        assert!(matches!(execute("fill", &mut v, &mut h), Outcome::Error(_)));
        assert!(matches!(
            execute("fill red", &mut v, &mut h),
            Outcome::Error(_)
        ));
        assert!(matches!(execute("box 1 2", &mut v, &mut h), Outcome::Error(_)));
    }

    #[test]
    fn an_empty_line_is_not_an_error() {
        let (mut v, mut h) = setup();
        assert!(matches!(
            execute("   ", &mut v, &mut h),
            Outcome::Ok { changed: false, .. }
        ));
    }

    #[test]
    fn clear_then_undo_restores_everything() {
        let (mut v, mut h) = setup();
        execute("clear", &mut v, &mut h);
        assert_eq!(v.filled_count(), 0);
        execute("undo", &mut v, &mut h);
        assert_eq!(v.get(1, 1, 1), 3);
    }
}
