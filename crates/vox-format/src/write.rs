//! Serialising a [`VoxFile`] back to bytes.
//!
//! The chunk tree is written depth-first, and every chunk header needs the
//! byte length of its content *and* of its children before either is written.
//! Rather than computing sizes up front, each chunk is built into its own
//! buffer and then spliced in with the lengths known — the same trick the
//! editor's own XIFF writer uses with its "chunk unfinished" bookkeeping.

use crate::{Color, Dict, SceneNode, VoxFile};

/// Grows a byte buffer with the format's primitives.
struct Writer {
    out: Vec<u8>,
}

impl Writer {
    fn new() -> Self {
        Self { out: Vec::new() }
    }

    fn u8(&mut self, v: u8) {
        self.out.push(v);
    }

    fn u32(&mut self, v: u32) {
        self.out.extend_from_slice(&v.to_le_bytes());
    }

    fn i32(&mut self, v: i32) {
        self.out.extend_from_slice(&v.to_le_bytes());
    }

    fn bytes(&mut self, v: &[u8]) {
        self.out.extend_from_slice(v);
    }

    fn string(&mut self, v: &str) {
        self.u32(v.len() as u32);
        self.bytes(v.as_bytes());
    }

    fn dict(&mut self, d: &Dict) {
        self.u32(d.0.len() as u32);
        for (k, v) in &d.0 {
            self.string(k);
            self.string(v);
        }
    }

    /// Writes a complete chunk with no children.
    fn chunk(&mut self, id: &[u8; 4], content: &[u8]) {
        self.bytes(id);
        self.u32(content.len() as u32);
        self.u32(0); // children length
        self.bytes(content);
    }
}

impl VoxFile {
    /// Serialises this file to `.vox` bytes.
    ///
    /// The result round-trips: parsing it yields an equivalent `VoxFile`.
    /// Chunks this crate does not model are not reproduced — see
    /// [`VoxFile::unknown_chunks`] to check whether any were dropped.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut children = Writer::new();

        // Model geometry: every XYZI must be preceded by its SIZE.
        for model in &self.models {
            let mut size = Writer::new();
            size.u32(model.size[0]);
            size.u32(model.size[1]);
            size.u32(model.size[2]);
            children.chunk(b"SIZE", &size.out);

            let mut xyzi = Writer::new();
            xyzi.u32(model.voxels.len() as u32);
            for v in &model.voxels {
                xyzi.u8(v.x);
                xyzi.u8(v.y);
                xyzi.u8(v.z);
                xyzi.u8(v.index);
            }
            children.chunk(b"XYZI", &xyzi.out);
        }

        // The scene graph. A file with none still needs one, or the editor
        // opens it with nothing placed.
        let minimal;
        let nodes = if self.nodes.is_empty() {
            minimal = crate::minimal_graph(&self.models);
            &minimal
        } else {
            &self.nodes
        };
        for node in nodes {
            write_node(&mut children, node);
        }

        for layer in &self.layers {
            let mut w = Writer::new();
            w.i32(layer.id);
            w.dict(&layer.attributes);
            w.i32(-1); // reserved
            children.chunk(b"LAYR", &w.out);
        }

        // Undo the palette shift: chunk entry i holds the colour of voxel
        // index i+1, which is how Palette::from_chunk read it.
        let mut rgba = Writer::new();
        for i in 0..256usize {
            let c: Color = self.palette.color(((i + 1) & 0xff) as u8);
            rgba.u8(c.r);
            rgba.u8(c.g);
            rgba.u8(c.b);
            rgba.u8(c.a);
        }
        children.chunk(b"RGBA", &rgba.out);

        if let Some(map) = &self.index_map {
            children.chunk(b"IMAP", map.as_slice());
        }

        for material in &self.materials {
            let mut w = Writer::new();
            w.i32(material.id);
            w.dict(&material.properties);
            children.chunk(b"MATL", &w.out);
        }

        for obj in &self.render_objects {
            let mut w = Writer::new();
            w.dict(obj);
            children.chunk(b"rOBJ", &w.out);
        }

        for camera in &self.cameras {
            let mut w = Writer::new();
            w.i32(camera.id);
            w.dict(&camera.attributes);
            children.chunk(b"rCAM", &w.out);
        }

        if !self.color_names.is_empty() {
            let mut w = Writer::new();
            w.u32(self.color_names.len() as u32);
            for name in &self.color_names {
                w.string(name);
            }
            children.chunk(b"NOTE", &w.out);
        }

        for meta in &self.meta {
            let mut w = Writer::new();
            w.dict(meta);
            children.chunk(b"META", &w.out);
        }

        // Header, then MAIN wrapping everything as its children.
        let mut out = Writer::new();
        out.bytes(crate::MAGIC);
        out.u32(self.version);
        out.bytes(b"MAIN");
        out.u32(0); // MAIN carries no content of its own
        out.u32(children.out.len() as u32);
        out.bytes(&children.out);
        out.out
    }
}

fn write_node(children: &mut Writer, node: &SceneNode) {
    match node {
        SceneNode::Transform(t) => {
            let mut w = Writer::new();
            w.i32(t.id);
            w.dict(&t.attributes);
            w.i32(t.child);
            w.i32(-1); // reserved
            w.i32(t.layer);
            w.u32(t.frames.len() as u32);
            for frame in &t.frames {
                w.dict(frame);
            }
            children.chunk(b"nTRN", &w.out);
        }
        SceneNode::Group(g) => {
            let mut w = Writer::new();
            w.i32(g.id);
            w.dict(&g.attributes);
            w.u32(g.children.len() as u32);
            for child in &g.children {
                w.i32(*child);
            }
            children.chunk(b"nGRP", &w.out);
        }
        SceneNode::Shape(s) => {
            let mut w = Writer::new();
            w.i32(s.id);
            w.dict(&s.attributes);
            w.u32(s.models.len() as u32);
            for m in &s.models {
                w.i32(m.model_id);
                w.dict(&m.attributes);
            }
            children.chunk(b"nSHP", &w.out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Model, Voxel};

    fn sample() -> VoxFile {
        let mut file = VoxFile::parse(&minimal_bytes()).unwrap();
        file.models[0].voxels.push(Voxel {
            x: 1,
            y: 1,
            z: 1,
            index: 42,
        });
        file
    }

    /// The smallest file the parser accepts: header, MAIN, SIZE, XYZI.
    fn minimal_bytes() -> Vec<u8> {
        let mut children = Writer::new();
        let mut size = Writer::new();
        size.u32(4);
        size.u32(4);
        size.u32(4);
        children.chunk(b"SIZE", &size.out);
        let mut xyzi = Writer::new();
        xyzi.u32(0);
        children.chunk(b"XYZI", &xyzi.out);

        let mut out = Writer::new();
        out.bytes(crate::MAGIC);
        out.u32(crate::VERSION_150);
        out.bytes(b"MAIN");
        out.u32(0);
        out.u32(children.out.len() as u32);
        out.bytes(&children.out);
        out.out
    }

    #[test]
    fn written_bytes_parse_back() {
        let original = sample();
        let bytes = original.to_bytes();
        let reparsed = VoxFile::parse(&bytes).expect("our own output must parse");

        assert_eq!(reparsed.version, original.version);
        assert_eq!(reparsed.models.len(), original.models.len());
        assert_eq!(reparsed.models[0].size, [4, 4, 4]);
        assert_eq!(reparsed.models[0].voxels.len(), 1);
        assert_eq!(reparsed.models[0].voxels[0].index, 42);
    }

    #[test]
    fn a_file_without_a_graph_gains_a_minimal_one() {
        let bytes = sample().to_bytes();
        let reparsed = VoxFile::parse(&bytes).unwrap();
        // root transform, group, transform, shape
        assert_eq!(reparsed.nodes.len(), 4);
        assert!(matches!(reparsed.nodes[0], SceneNode::Transform(_)));
        assert!(matches!(reparsed.nodes[1], SceneNode::Group(_)));
    }

    #[test]
    fn the_minimal_graph_sits_models_on_the_ground() {
        let reparsed = VoxFile::parse(&sample().to_bytes()).unwrap();
        // sample() is 4 high, so the editor's convention puts it at z = 2.
        let lifted = reparsed.nodes.iter().any(|n| match n {
            SceneNode::Transform(t) => t.translation(0) == [0, 0, 2],
            _ => false,
        });
        assert!(lifted, "model transform should carry _t = 0 0 2");
    }

    #[test]
    fn palette_survives_the_round_trip() {
        let mut file = sample();
        let mut entries = [Color::default(); 256];
        entries[0] = Color { r: 10, g: 20, b: 30, a: 255 };
        entries[9] = Color { r: 1, g: 2, b: 3, a: 255 };
        file.palette = crate::Palette::from_chunk(&entries);

        let reparsed = VoxFile::parse(&file.to_bytes()).unwrap();
        // Chunk entry 0 is voxel index 1; entry 9 is index 10.
        assert_eq!(reparsed.palette.color(1), entries[0]);
        assert_eq!(reparsed.palette.color(10), entries[9]);
    }

    #[test]
    fn empty_models_still_write_a_valid_file() {
        let file = VoxFile {
            version: crate::VERSION_150,
            models: vec![Model {
                size: [1, 1, 1],
                voxels: Vec::new(),
            }],
            ..VoxFile::parse(&minimal_bytes()).unwrap()
        };
        assert!(VoxFile::parse(&file.to_bytes()).is_ok());
    }

    #[test]
    fn a_new_file_has_one_grounded_model_and_the_default_palette() {
        let file = VoxFile::new_single([10, 20, 7]);
        assert_eq!(file.models.len(), 1);
        assert_eq!(file.models[0].size, [10, 20, 7]);
        assert!(!file.nodes.is_empty(), "the graph the writer adds is kept");
        let lifted = file.nodes.iter().any(|n| match n {
            SceneNode::Transform(t) => t.translation(0) == [0, 0, 3],
            _ => false,
        });
        assert!(lifted, "a 7-high model sits on the ground at z = 3");
        assert!(VoxFile::parse(&file.to_bytes()).is_ok());
    }

    #[test]
    fn the_specification_palette_has_its_landmarks() {
        let p = crate::Palette::specification_default();
        let rgb = |i: u8| {
            let c = p.color(i);
            (c.r, c.g, c.b)
        };
        assert_eq!(rgb(1), (0xff, 0xff, 0xff));
        assert_eq!(rgb(2), (0xff, 0xff, 0xcc), "blue changes fastest");
        assert_eq!(rgb(215), (0x00, 0x00, 0x33), "the cube ends just before black");
        assert_eq!(rgb(216), (0xee, 0x00, 0x00), "then the red ramp");
        assert_eq!(rgb(255), (0x11, 0x11, 0x11), "and the grey ramp closes it");
    }
}
