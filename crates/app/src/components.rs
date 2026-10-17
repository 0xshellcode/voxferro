//! Component files: the reference editor's File tab exports and imports a project's
//! materials, render settings and cameras on their own (`o_comp`,
//! `i_comp`). They are text, written by `the reference renderer::
//! SaveDictToSLM` in the syntax of its `config.txt`:
//!
//! ```text
//! name : {
//!     key = 'value'
//! }
//! ```
//!
//! `export materials|render|cameras <file.txt>` writes one block a dict --
//! `mtl` blocks keyed by `_id` for materials, a block named by its `_type`
//! for each render object, `camera` blocks keyed by `_id` -- and `import
//! ... <file.txt>` reads them back, one undo step. The block names are this
//! editor's choice; the reader takes any name, and comments (`//`) and blank
//! lines are skipped, so the reference editor's own files read too.

use vox_format::Dict;

/// Blocks of a component file: name and dict, in order.
pub fn parse(text: &str) -> Result<Vec<(String, Dict)>, String> {
    let mut out = Vec::new();
    let mut open: Option<(String, Dict)> = None;
    for (n, raw) in text.lines().enumerate() {
        let line = raw.split("//").next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if let Some(name) = line.strip_suffix('{').map(str::trim).and_then(|l| l.strip_suffix(':')) {
            if open.is_some() {
                return Err(format!("line {}: a block inside a block", n + 1));
            }
            open = Some((name.trim().to_owned(), Dict::default()));
        } else if line == "}" {
            out.push(open.take().ok_or_else(|| format!("line {}: '}}' without a block", n + 1))?);
        } else if let Some((key, value)) = line.split_once('=') {
            let (_, dict) = open.as_mut().ok_or_else(|| format!("line {}: a value outside a block", n + 1))?;
            let value = value.trim().trim_matches('\'');
            dict.set(key.trim(), value);
        } else {
            return Err(format!("line {}: expected `key = 'value'`", n + 1));
        }
    }
    if open.is_some() {
        return Err("the last block is not closed".to_owned());
    }
    Ok(out)
}

/// Writes blocks as `SaveDictToSLM` does.
pub fn write(blocks: &[(String, Dict)]) -> String {
    let mut out = String::new();
    for (name, dict) in blocks {
        out.push_str(&format!("{name} : {{\n"));
        for (k, v) in &dict.0 {
            out.push_str(&format!("    {k} = '{v}'\n"));
        }
        out.push_str("}\n");
    }
    out
}

/// A dict with `_id` first, then `rest`.
fn with_id(id: i32, rest: &Dict) -> Dict {
    let mut d = Dict::default();
    d.set("_id", id.to_string());
    for (k, v) in &rest.0 {
        if k != "_id" {
            d.set(k, v.clone());
        }
    }
    d
}

impl crate::App {
    pub fn export_component(&self, what: &str, path: &str) -> String {
        let Some(loaded) = &self.scene else { return "nothing open".to_owned() };
        let doc = &loaded.doc;
        let blocks: Vec<(String, Dict)> = match what {
            "materials" => doc.materials.iter().map(|m| ("mtl".to_owned(), with_id(m.id, &m.properties))).collect(),
            "render" => doc
                .render
                .iter()
                .map(|d| (d.get("_type").unwrap_or("render").trim_start_matches('_').to_owned(), d.clone()))
                .collect(),
            "cameras" => doc.cameras.iter().map(|c| ("camera".to_owned(), with_id(c.id, &c.attributes))).collect(),
            _ => return "export materials|render|cameras <file.txt>".to_owned(),
        };
        match std::fs::write(path, write(&blocks)) {
            Ok(()) => format!("exported {} {what} to {path}", blocks.len()),
            Err(e) => format!("error: could not write {path}: {e}"),
        }
    }

    /// The command that sets `what` from a component file.
    pub fn component_command(&self, what: &str, text: &str) -> Result<voxel_core::Command, String> {
        let Some(loaded) = &self.scene else { return Err("nothing open".to_owned()) };
        let doc = &loaded.doc;
        let blocks = parse(text)?;
        let id_of = |d: &Dict| d.get("_id").and_then(|i| i.parse::<i32>().ok());
        Ok(match what {
            "materials" => voxel_core::Command::Batch(
                blocks
                    .iter()
                    .filter_map(|(_, d)| {
                        let id = id_of(d).filter(|i| (1..=255).contains(i))?;
                        let mut properties = d.clone();
                        properties.0.retain(|(k, _)| k != "_id");
                        let index = id as u8;
                        let after = Some(vox_format::Material { id, properties });
                        let before = doc.material(index).cloned();
                        (before != after).then_some(voxel_core::Command::SetMaterial { index, before, after })
                    })
                    .collect(),
            ),
            "render" => voxel_core::Command::SetRender { before: doc.render.clone(), after: blocks.into_iter().map(|(_, d)| d).collect() },
            "cameras" => voxel_core::Command::SetCameras {
                before: doc.cameras.clone(),
                after: blocks
                    .iter()
                    .filter_map(|(_, d)| {
                        let id = id_of(d)?;
                        let mut attributes = d.clone();
                        attributes.0.retain(|(k, _)| k != "_id");
                        Some(vox_format::Camera { id, attributes })
                    })
                    .collect(),
            },
            _ => return Err("import materials|render|cameras <file.txt>".to_owned()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_round_trip_and_read_reference_editor_syntax() {
        let mut d = Dict::default();
        d.set("_type", "_metal");
        d.set("_rough", "0.1");
        let text = write(&[("mtl".to_owned(), d.clone())]);
        assert_eq!(text, "mtl : {\n    _type = '_metal'\n    _rough = '0.1'\n}\n");
        assert_eq!(parse(&text).unwrap(), vec![("mtl".to_owned(), d)]);
        // the reference editor's config.txt style, comments and all.
        let mv = "io_obj :\n{\n    scale   : '0.1'\n}\n";
        assert!(parse(mv).is_err(), "':' pairs are not dict values");
        let ok = "render : {\n    _type = '_env' // the sky\n\n    _i = '1'\n}\n";
        let blocks = parse(ok).unwrap();
        assert_eq!(blocks[0].1.get("_i"), Some("1"));
        assert!(parse("x : {\n a = '1'\n").is_err());
    }
}
