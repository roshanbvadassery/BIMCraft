//! A plan as DXF (ASCII, R12 `AC1009`, which every CAD program opens): the plan's display list
//! ([`crate::render::plan`]) in model millimetres, x east and y north. A layer per category and what
//! is drawn of it (`WALL-CUT`, `DOOR`, `GRID`, `ANNOTATION`), each line in its nearest AutoCAD colour
//! and its dash pattern as a linetype. Fills (poché, room colours) are written as their outlines on
//! `<layer>-FILL`, since R12 has no hatch; lettering is drawn in strokes, as on the plan.

use std::collections::BTreeMap;
use std::fmt::Write;

use crate::model::Model;
use crate::render::{Drawing, Part, Prim, Rgb};

/// The plan `d` (of `m`) as a DXF file.
pub fn plan(d: &Drawing, m: &Model) -> String {
    let view = &d.view;
    let mm = |p: (f32, f32)| {
        let q = view.mm(p);
        (q.0, q.1)
    };
    let mut layers: BTreeMap<String, u8> = BTreeMap::new();
    let mut linetypes: Vec<Vec<f64>> = Vec::new();
    let mut entities = String::new();
    let (mut lo, mut hi) = ((f64::MAX, f64::MAX), (f64::MIN, f64::MIN));
    for (i, prim) in d.prims.iter().enumerate() {
        let owner = d.owners.get(i).copied().flatten();
        let category = owner.and_then(|id| m.elements.get(id)).map(|e| e.category());
        let part = d.parts.get(i).copied().unwrap_or_default();
        let base = match (category, part) {
            (_, Part::Text) => "ANNOTATION".to_string(),
            (Some(c), Part::Cut) => format!("{}-CUT", c.to_ascii_uppercase()),
            (Some(c), Part::Pattern) => format!("{}-PATTERN", c.to_ascii_uppercase()),
            (Some(c), _) => c.to_ascii_uppercase(),
            (None, _) => "ANNOTATION".to_string(),
        };
        let mut poly = |layer: &str, color: Rgb, ltype: &str, pts: Vec<(f64, f64)>, closed: bool, out: &mut String| {
            let aci = nearest_aci(color);
            layers.entry(layer.to_string()).or_insert(aci);
            for p in &pts {
                lo = (lo.0.min(p.0), lo.1.min(p.1));
                hi = (hi.0.max(p.0), hi.1.max(p.1));
            }
            if pts.len() == 2 && !closed {
                pair(out, 0, "LINE");
                pair(out, 8, layer);
                pair(out, 6, ltype);
                pair(out, 62, &aci.to_string());
                for (k, p) in pts.iter().enumerate() {
                    pair(out, 10 + k as i32, &num(p.0));
                    pair(out, 20 + k as i32, &num(p.1));
                    pair(out, 30 + k as i32, "0.0");
                }
                return;
            }
            pair(out, 0, "POLYLINE");
            pair(out, 8, layer);
            pair(out, 6, ltype);
            pair(out, 62, &aci.to_string());
            pair(out, 66, "1");
            pair(out, 70, if closed { "1" } else { "0" });
            for p in &pts {
                pair(out, 0, "VERTEX");
                pair(out, 8, layer);
                pair(out, 10, &num(p.0));
                pair(out, 20, &num(p.1));
                pair(out, 30, "0.0");
            }
            pair(out, 0, "SEQEND");
            pair(out, 8, layer);
        };
        match prim {
            Prim::Line { pts, color, dash, closed, .. } if pts.len() >= 2 => {
                let ltype = match dash {
                    Some(dash) if !dash.is_empty() && view.scale > 0.0 => {
                        let pattern: Vec<f64> = dash.iter().map(|v| f64::from(*v) / view.scale).collect();
                        let k = linetypes.iter().position(|l| *l == pattern).unwrap_or_else(|| {
                            linetypes.push(pattern);
                            linetypes.len() - 1
                        });
                        format!("BC-DASH-{}", k + 1)
                    }
                    _ => "CONTINUOUS".to_string(),
                };
                poly(&base, *color, &ltype, pts.iter().map(|&p| mm(p)).collect(), *closed, &mut entities);
            }
            Prim::Fill { rings, color } => {
                for ring in rings.iter().filter(|r| r.len() >= 3) {
                    poly(&format!("{base}-FILL"), *color, "CONTINUOUS", ring.iter().map(|&p| mm(p)).collect(), true, &mut entities);
                }
            }
            _ => {}
        }
    }
    if lo.0 > hi.0 {
        (lo, hi) = ((0.0, 0.0), (0.0, 0.0));
    }

    let mut out = String::new();
    pair(&mut out, 0, "SECTION");
    pair(&mut out, 2, "HEADER");
    pair(&mut out, 9, "$ACADVER");
    pair(&mut out, 1, "AC1009");
    pair(&mut out, 9, "$INSUNITS");
    pair(&mut out, 70, "4");
    pair(&mut out, 9, "$EXTMIN");
    pair(&mut out, 10, &num(lo.0));
    pair(&mut out, 20, &num(lo.1));
    pair(&mut out, 9, "$EXTMAX");
    pair(&mut out, 10, &num(hi.0));
    pair(&mut out, 20, &num(hi.1));
    pair(&mut out, 9, "$LTSCALE");
    pair(&mut out, 40, "1.0");
    pair(&mut out, 0, "ENDSEC");
    pair(&mut out, 0, "SECTION");
    pair(&mut out, 2, "TABLES");
    // linetypes: CONTINUOUS and each dash pattern the plan draws
    pair(&mut out, 0, "TABLE");
    pair(&mut out, 2, "LTYPE");
    pair(&mut out, 70, &(linetypes.len() + 1).to_string());
    ltype(&mut out, "CONTINUOUS", "Solid line", &[]);
    for (k, pattern) in linetypes.iter().enumerate() {
        ltype(&mut out, &format!("BC-DASH-{}", k + 1), "Dashed", pattern);
    }
    pair(&mut out, 0, "ENDTAB");
    pair(&mut out, 0, "TABLE");
    pair(&mut out, 2, "LAYER");
    pair(&mut out, 70, &(layers.len() + 1).to_string());
    for (name, aci) in std::iter::once(("0".to_string(), 7u8)).chain(layers) {
        pair(&mut out, 0, "LAYER");
        pair(&mut out, 2, &name);
        pair(&mut out, 70, "0");
        pair(&mut out, 62, &aci.to_string());
        pair(&mut out, 6, "CONTINUOUS");
    }
    pair(&mut out, 0, "ENDTAB");
    pair(&mut out, 0, "ENDSEC");
    pair(&mut out, 0, "SECTION");
    pair(&mut out, 2, "ENTITIES");
    out.push_str(&entities);
    pair(&mut out, 0, "ENDSEC");
    pair(&mut out, 0, "EOF");
    out
}

fn pair(out: &mut String, code: i32, value: &str) {
    let _ = write!(out, "{code:>3}\n{value}\n");
}

fn num(v: f64) -> String {
    if v.is_finite() { format!("{v:.3}") } else { "0.0".into() }
}

fn ltype(out: &mut String, name: &str, description: &str, pattern: &[f64]) {
    pair(out, 0, "LTYPE");
    pair(out, 2, name);
    pair(out, 70, "0");
    pair(out, 3, description);
    pair(out, 72, "65");
    pair(out, 73, &pattern.len().to_string());
    pair(out, 40, &num(pattern.iter().sum()));
    for (k, v) in pattern.iter().enumerate() {
        // on lengths positive, off lengths negative
        pair(out, 49, &num(if k % 2 == 0 { *v } else { -*v }));
    }
}

/// The AutoCAD colour index nearest `c`: dark and light greys are 7, which shows black on white and white on black.
fn nearest_aci(c: Rgb) -> u8 {
    let Rgb(r, g, b) = c;
    // near-black and near-white greys (lettering, cut lines) are 7, as CAD draws ink
    let (hi, lo) = (r.max(g).max(b), r.min(g).min(b));
    if hi - lo < 24 && !(96..=200).contains(&hi) {
        return 7;
    }
    let d = |x: Rgb| {
        let (dr, dg, db) = (i32::from(x.0) - i32::from(r), i32::from(x.1) - i32::from(g), i32::from(x.2) - i32::from(b));
        dr * dr + dg * dg + db * db
    };
    (1..=255u8).filter_map(|i| crate::dxf::aci(i).map(|x| (d(x), i))).min().map_or(7, |(_, i)| i)
}

#[cfg(test)]
mod tests {
    use crate::engine::Session;

    #[test]
    fn the_sample_plan_reads_back_with_layers_by_category_in_millimetres() {
        let mut s = Session::new();
        s.script(crate::sample::SAMPLE).unwrap();
        let level = s.model.levels().first().map(|(id, _)| *id).unwrap();
        let d = crate::render::plan(&crate::link::seen(&s.model), level, 1600, 1000);
        let text = super::plan(&d, &s.model);
        let dxf = crate::dxf::read_text(&text).unwrap();
        let names: Vec<&str> = dxf.layers.iter().map(|l| l.name.as_str()).collect();
        for want in ["WALL-CUT", "DOOR", "GRID", "ANNOTATION"] {
            assert!(names.contains(&want), "a {want} layer in {names:?}");
        }
        assert!(dxf.entities.len() > 100);
        let [lo, hi] = dxf.extents.unwrap();
        assert!(lo.0 < 0.0 && hi.0 > 10000.0 && hi.1 > 7000.0, "the house's 10 x 7 m in mm: {lo:?} {hi:?}");
        assert_eq!(dxf.scale, Some(1.0), "millimetres");
    }

    #[test]
    fn save_writes_a_level_plan_as_dxf() {
        let mut s = Session::new();
        s.script(crate::sample::SAMPLE).unwrap();
        let path = std::env::temp_dir().join(format!("bimcraft-test-{}-plan.dxf", std::process::id()));
        let r = s.execute("save", &serde_json::json!({"path": path, "view": "Level 1"})).unwrap();
        assert_eq!(r["format"], "DXF");
        assert!(std::fs::read_to_string(&path).unwrap().contains("AC1009"));
        let _ = std::fs::remove_file(&path);
    }
}
