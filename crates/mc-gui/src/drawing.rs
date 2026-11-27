//! A small DXF writer (AutoCAD 2010, micrometres). It fills an empty drawing made by ezdxf
//! (`tools/make_dxf_template.py`), so the tables, layouts and dictionaries AutoCAD expects are all there;
//! this adds layers, lines, polylines, texts and raster images with their definitions.

const TEMPLATE: &str = include_str!("drawing_template.dxf");
/// handles inside the template (printed by the template script; checked by a test)
const ROOT: &str = "A";
const MODEL_SPACE: &str = "17";
const LAYER_TABLE: &str = "1";
const PLOT_STYLE: &str = "13";
const MATERIAL: &str = "21";
const FIRST_FREE: u64 = 0x32;

pub struct Dxf {
    next: u64,
    layers: String,
    entities: String,
    objects: String,
    images: Vec<(String, String)>, // (name, IMAGEDEF handle)
    image_dict: String,
    raster_vars: String,
}

/// one pair: the group code right-aligned in 3, then the value
fn pair(out: &mut String, code: i32, v: impl std::fmt::Display) {
    use std::fmt::Write;
    let _ = write!(out, "{code:>3}\n{v}\n");
}

/// a number exactly as f64 (the shortest text that reads back to the same number)
fn num(v: f64) -> String {
    let s = format!("{v:?}");
    if s.contains('e') {
        format!("{v:.12}")
    } else {
        s
    }
}

impl Dxf {
    pub fn new() -> Dxf {
        let mut d = Dxf { next: FIRST_FREE, layers: String::new(), entities: String::new(), objects: String::new(), images: vec![], image_dict: String::new(), raster_vars: String::new() };
        d.image_dict = d.handle();
        d.raster_vars = d.handle();
        d
    }

    fn handle(&mut self) -> String {
        let h = format!("{:X}", self.next);
        self.next += 1;
        h
    }

    /// colour: AutoCAD index 1..255; rgb: true colour shown instead when given
    pub fn layer(&mut self, name: &str, color: u8, rgb: Option<[u8; 3]>) {
        let h = self.handle();
        let o = &mut self.layers;
        pair(o, 0, "LAYER");
        pair(o, 5, &h);
        pair(o, 330, LAYER_TABLE);
        pair(o, 100, "AcDbSymbolTableRecord");
        pair(o, 100, "AcDbLayerTableRecord");
        pair(o, 2, name);
        pair(o, 70, 0);
        pair(o, 62, color);
        if let Some([r, g, b]) = rgb {
            pair(o, 420, (r as u32) << 16 | (g as u32) << 8 | b as u32);
        }
        pair(o, 6, "Continuous");
        pair(o, 370, -3);
        pair(o, 390, PLOT_STYLE);
        pair(o, 347, MATERIAL);
    }

    fn entity(&mut self, kind: &str, layer: &str) -> String {
        let h = self.handle();
        let o = &mut self.entities;
        pair(o, 0, kind);
        pair(o, 5, &h);
        pair(o, 330, MODEL_SPACE);
        pair(o, 100, "AcDbEntity");
        pair(o, 8, layer);
        h
    }

    pub fn line(&mut self, layer: &str, a: [f64; 2], b: [f64; 2]) {
        self.entity("LINE", layer);
        let o = &mut self.entities;
        pair(o, 100, "AcDbLine");
        for (c, v) in [(10, a[0]), (20, a[1]), (30, 0.0), (11, b[0]), (21, b[1]), (31, 0.0)] {
            pair(o, c, num(v));
        }
    }

    pub fn polyline(&mut self, layer: &str, pts: &[[f64; 2]], closed: bool) {
        self.entity("LWPOLYLINE", layer);
        let o = &mut self.entities;
        pair(o, 100, "AcDbPolyline");
        pair(o, 90, pts.len());
        pair(o, 70, if closed { 1 } else { 0 });
        for p in pts {
            pair(o, 10, num(p[0]));
            pair(o, 20, num(p[1]));
        }
    }

    /// text whose lower left corner is at `at`
    pub fn text(&mut self, layer: &str, at: [f64; 2], height: f64, s: &str) {
        self.entity("TEXT", layer);
        let o = &mut self.entities;
        pair(o, 100, "AcDbText");
        for (c, v) in [(10, at[0]), (20, at[1]), (30, 0.0), (40, height)] {
            pair(o, c, num(v));
        }
        pair(o, 1, s);
        pair(o, 100, "AcDbText");
    }

    /// A raster picture `file` (path relative to the drawing) of `px` pixels; its lower left corner at `at`,
    /// one pixel `du` x `dv` drawing units. Row 0 of the file is the top of the picture.
    pub fn image(&mut self, layer: &str, file: &str, px: [usize; 2], at: [f64; 2], du: f64, dv: f64) {
        let def = self.handle();
        let reactor = self.handle();
        let e = self.entity("IMAGE", layer);
        let o = &mut self.entities;
        pair(o, 100, "AcDbRasterImage");
        pair(o, 90, 0);
        for (c, v) in [(10, at[0]), (20, at[1]), (30, 0.0), (11, du), (21, 0.0), (31, 0.0), (12, 0.0), (22, dv), (32, 0.0), (13, px[0] as f64), (23, px[1] as f64)] {
            pair(o, c, num(v));
        }
        pair(o, 340, &def);
        pair(o, 70, 1 | 2 | 8); // show, show when turned, transparency
        pair(o, 280, 0);
        pair(o, 281, 50);
        pair(o, 282, 50);
        pair(o, 283, 0);
        pair(o, 360, &reactor);
        pair(o, 71, 1);
        pair(o, 91, 2);
        for (c, v) in [(14, -0.5), (24, -0.5), (14, px[0] as f64 - 0.5), (24, px[1] as f64 - 0.5)] {
            pair(o, c, num(v));
        }
        pair(o, 290, 0);
        // its definition (owned by the image dictionary) and the reactor that ties the two
        let o = &mut self.objects;
        pair(o, 0, "IMAGEDEF");
        pair(o, 5, &def);
        pair(o, 102, "{ACAD_REACTORS");
        pair(o, 330, &self.image_dict);
        pair(o, 330, &reactor);
        pair(o, 102, "}");
        pair(o, 330, &self.image_dict);
        pair(o, 100, "AcDbRasterImageDef");
        pair(o, 90, 0);
        pair(o, 1, file);
        pair(o, 10, num(px[0] as f64));
        pair(o, 20, num(px[1] as f64));
        pair(o, 11, num(1.0));
        pair(o, 21, num(1.0));
        pair(o, 280, 1);
        pair(o, 281, 0);
        pair(o, 0, "IMAGEDEF_REACTOR");
        pair(o, 5, &reactor);
        pair(o, 330, &e);
        pair(o, 100, "AcDbRasterImageDefReactor");
        pair(o, 90, 2);
        pair(o, 330, &e);
        // names in the image dictionary must be unique
        let name = format!("{}_{}", file.replace(['/', '\\', '.'], "_"), self.images.len());
        self.images.push((name, def));
    }

    pub fn text_out(mut self) -> String {
        let mut classes = String::new();
        let mut root = String::new();
        let mut objects = String::new();
        if !self.images.is_empty() {
            for (name, cpp, flags, ent) in [("RASTERVARIABLES", "AcDbRasterVariables", 0, 0), ("IMAGE", "AcDbRasterImage", 2175, 1), ("IMAGEDEF", "AcDbRasterImageDef", 0, 0), ("IMAGEDEF_REACTOR", "AcDbRasterImageDefReactor", 1, 0)] {
                pair(&mut classes, 0, "CLASS");
                pair(&mut classes, 1, name);
                pair(&mut classes, 2, cpp);
                pair(&mut classes, 3, "ISM");
                pair(&mut classes, 90, flags);
                pair(&mut classes, 91, 0);
                pair(&mut classes, 280, 0);
                pair(&mut classes, 281, ent);
            }
            pair(&mut root, 3, "ACAD_IMAGE_DICT");
            pair(&mut root, 350, &self.image_dict);
            pair(&mut root, 3, "ACAD_IMAGE_VARS");
            pair(&mut root, 350, &self.raster_vars);
            let o = &mut objects;
            pair(o, 0, "RASTERVARIABLES");
            pair(o, 5, &self.raster_vars);
            pair(o, 102, "{ACAD_REACTORS");
            pair(o, 330, ROOT);
            pair(o, 102, "}");
            pair(o, 330, ROOT);
            pair(o, 100, "AcDbRasterVariables");
            pair(o, 90, 0);
            pair(o, 70, 0); // no frames around the pictures
            pair(o, 71, 1); // high quality
            pair(o, 72, 0); // no units
            pair(o, 0, "DICTIONARY");
            pair(o, 5, &self.image_dict);
            pair(o, 102, "{ACAD_REACTORS");
            pair(o, 330, ROOT);
            pair(o, 102, "}");
            pair(o, 330, ROOT);
            pair(o, 100, "AcDbDictionary");
            pair(o, 281, 1);
            for (name, h) in &self.images {
                pair(o, 3, name);
                pair(o, 350, h);
            }
        }
        objects.push_str(&std::mem::take(&mut self.objects));
        let seed = format!("{:X}", self.next);
        let mut out = String::with_capacity(TEMPLATE.len() + self.entities.len() + objects.len() + 4096);
        for line in TEMPLATE.split('\n') {
            let part: &str = match line {
                "@HANDSEED@" => &seed,
                "@CLASSES@" => &classes,
                "@LAYERS@" => &self.layers,
                "@ENTITIES@" => &self.entities,
                "@ROOT@" => &root,
                "@OBJECTS@" => &objects,
                _ => {
                    out.push_str(line);
                    out.push('\n');
                    continue;
                }
            };
            out.push_str(part);
            if line == "@HANDSEED@" {
                out.push('\n');
            }
        }
        // the template ends with "EOF" and a newline already
        while out.ends_with("\n\n") {
            out.pop();
        }
        out
    }

    pub fn save(self, path: &std::path::Path) -> Result<(), String> {
        let tmp = path.with_extension("dxf.tmp");
        std::fs::write(&tmp, self.text_out()).map_err(|e| format!("{}: {e}", path.display()))?;
        std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn template_handles_are_the_ones_used() {
        let t = super::TEMPLATE;
        for m in ["@HANDSEED@", "@CLASSES@", "@LAYERS@", "@ENTITIES@", "@ROOT@", "@OBJECTS@"] {
            assert_eq!(t.split('\n').filter(|l| *l == m).count(), 1, "{m}");
        }
        assert!(t.contains("DICTIONARY\n  5\nA\n330\n0\n"));
        assert!(t.contains("BLOCK_RECORD\n  5\n17\n"));
        // the template's own seed, before the marker replaced it: nothing in the template uses FIRST_FREE or above
        let max = t.split("\n  5\n").skip(1).filter_map(|s| u64::from_str_radix(s.split('\n').next()?, 16).ok()).max().unwrap();
        assert!(max < super::FIRST_FREE, "{max:X}");
    }
}
