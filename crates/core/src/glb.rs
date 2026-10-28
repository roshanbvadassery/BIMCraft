//! glTF 2.0 binary (`.glb`): the model's shaded surfaces, as the 3D view draws them, for viewers on
//! the web and in other tools. One node per category (`wall`, `beam`, `footing`…), each a mesh whose
//! primitives are its triangles by colour; flat normals, metres, y up as glTF has it (BIMCraft is
//! millimetres, z up). Textures stay behind: each surface takes its material's colour.

use std::collections::BTreeMap;

use serde_json::{Value, json};

use crate::model::Model;
use crate::render::Rgb;
use crate::view3d::Scene;

/// The most triangles a file holds: past it the export is refused rather than written half.
pub const MAX_TRIS: usize = 8_000_000;

/// One primitive's vertices, three per triangle: positions then normals, as glTF reads them.
#[derive(Default)]
struct Run {
    positions: Vec<[f32; 3]>,
    normals: Vec<[f32; 3]>,
}

/// The model as a `.glb` file, leaving out the categories in `hide` (as `category` names them: `wall`,
/// `floor`…), as a frame is shown without the panels round it.
pub fn export(m: &Model, hide: &[String]) -> Result<Vec<u8>, String> {
    let scene = Scene::new(m);
    // category → colour → triangles.
    let mut by: BTreeMap<String, BTreeMap<(u8, u8, u8), Run>> = BTreeMap::new();
    let mut count = 0usize;
    for (p, owner, Rgb(r, g, b)) in scene.triangles() {
        let n = normal(&p);
        if n == [0.0; 3] {
            continue;
        }
        let category = m.elements.get(owner).map_or("other", |e| e.category());
        if hide.iter().any(|h| h == category) {
            continue;
        }
        count += 1;
        if count > MAX_TRIS {
            return Err(format!("the model has more than {MAX_TRIS} triangles; hide some categories or levels first"));
        }
        let run = by.entry(category.to_owned()).or_default().entry((r, g, b)).or_default();
        for q in p {
            run.positions.push(gl(q));
            run.normals.push(n);
        }
    }
    if count == 0 {
        return Err("there is nothing in 3D to export; the model has no shaded elements (or `hide` leaves none)".into());
    }

    let mut bin: Vec<u8> = Vec::new();
    let (mut views, mut accessors, mut meshes, mut nodes) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let mut materials: Vec<Value> = Vec::new();
    let mut material_of: BTreeMap<(u8, u8, u8), usize> = BTreeMap::new();
    for (category, runs) in &by {
        let mut primitives = Vec::new();
        for (rgb, run) in runs {
            let material = *material_of.entry(*rgb).or_insert_with(|| {
                materials.push(material(*rgb));
                materials.len() - 1
            });
            let (lo, hi) = bounds(&run.positions);
            let position = push(&mut bin, &mut views, &mut accessors, &run.positions, Some((lo, hi)));
            let normal = push(&mut bin, &mut views, &mut accessors, &run.normals, None);
            primitives.push(json!({"attributes": {"POSITION": position, "NORMAL": normal}, "material": material, "mode": 4}));
        }
        meshes.push(json!({"name": category, "primitives": primitives}));
        nodes.push(json!({"name": category, "mesh": meshes.len() - 1}));
    }
    let children: Vec<usize> = (0..nodes.len()).collect();
    let name = if m.name.is_empty() { "BIMCraft model".to_owned() } else { m.name.clone() };
    nodes.push(json!({"name": name, "children": children}));
    let root = nodes.len() - 1;
    let doc = json!({
        "asset": {"version": "2.0", "generator": "BIMCraft"},
        "scene": 0,
        "scenes": [{"name": name, "nodes": [root]}],
        "nodes": nodes,
        "meshes": meshes,
        "materials": materials,
        "accessors": accessors,
        "bufferViews": views,
        "buffers": [{"byteLength": bin.len()}],
    });
    Ok(container(&doc, bin))
}

/// BIMCraft's millimetres, z up, as glTF's metres, y up (x east, -z north).
fn gl(p: [f64; 3]) -> [f32; 3] {
    [(p[0] / 1000.0) as f32, (p[2] / 1000.0) as f32, (-p[1] / 1000.0) as f32]
}

/// A triangle's unit normal in glTF's axes, or zero where it has no area.
fn normal(p: &[[f64; 3]; 3]) -> [f32; 3] {
    let (a, b) = (sub(p[1], p[0]), sub(p[2], p[0]));
    let n = [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
    let l = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
    if !l.is_finite() || l < 1e-9 {
        return [0.0; 3];
    }
    [(n[0] / l) as f32, (n[2] / l) as f32, (-n[1] / l) as f32]
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn bounds(points: &[[f32; 3]]) -> ([f32; 3], [f32; 3]) {
    points.iter().fold(([f32::MAX; 3], [f32::MIN; 3]), |(lo, hi), p| {
        (std::array::from_fn(|k| lo[k].min(p[k])), std::array::from_fn(|k| hi[k].max(p[k])))
    })
}

/// A colour as a matte, two-sided PBR material (glTF's base colour is linear, ours sRGB).
fn material((r, g, b): (u8, u8, u8)) -> Value {
    let lin = |c: u8| {
        let c = f64::from(c) / 255.0;
        if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
    };
    json!({
        "name": format!("#{r:02x}{g:02x}{b:02x}"),
        "pbrMetallicRoughness": {"baseColorFactor": [lin(r), lin(g), lin(b), 1.0], "metallicFactor": 0.0, "roughnessFactor": 0.85},
        "doubleSided": true,
    })
}

/// Appends `data` as a VEC3 float accessor with a buffer view of its own; returns the accessor's index.
fn push(
    bin: &mut Vec<u8>,
    views: &mut Vec<Value>,
    accessors: &mut Vec<Value>,
    data: &[[f32; 3]],
    minmax: Option<([f32; 3], [f32; 3])>,
) -> usize {
    let offset = bin.len();
    for v in data {
        for c in v {
            bin.extend_from_slice(&c.to_le_bytes());
        }
    }
    views.push(json!({"buffer": 0, "byteOffset": offset, "byteLength": bin.len() - offset, "target": 34962}));
    let mut accessor = json!({"bufferView": views.len() - 1, "componentType": 5126, "count": data.len(), "type": "VEC3"});
    if let Some((lo, hi)) = minmax {
        accessor["min"] = json!(lo);
        accessor["max"] = json!(hi);
    }
    accessors.push(accessor);
    accessors.len() - 1
}

/// The GLB container: a 12-byte header, the JSON chunk padded with spaces, the binary chunk with zeros.
fn container(doc: &Value, mut bin: Vec<u8>) -> Vec<u8> {
    let mut text = doc.to_string().into_bytes();
    while !text.len().is_multiple_of(4) {
        text.push(b' ');
    }
    while !bin.len().is_multiple_of(4) {
        bin.push(0);
    }
    let total = 12 + 8 + text.len() + 8 + bin.len();
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(b"glTF");
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&(text.len() as u32).to_le_bytes());
    out.extend_from_slice(b"JSON");
    out.extend_from_slice(&text);
    out.extend_from_slice(&(bin.len() as u32).to_le_bytes());
    out.extend_from_slice(b"BIN\0");
    out.extend_from_slice(&bin);
    out
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use crate::engine::Session;

    fn u32_at(b: &[u8], at: usize) -> usize {
        b.get(at..at + 4).map_or(0, |s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]) as usize)
    }

    #[test]
    fn the_sample_house_exports_as_a_valid_glb_by_category() {
        let mut s = Session::new();
        s.script(crate::sample::SAMPLE).unwrap();
        let glb = super::export(&s.model, &[]).unwrap();
        assert_eq!(&glb[..4], b"glTF");
        assert_eq!(u32_at(&glb, 8), glb.len(), "the header's length is the file's");
        let json_len = u32_at(&glb, 12);
        assert_eq!(&glb[16..20], b"JSON");
        let doc: Value = serde_json::from_slice(&glb[20..20 + json_len]).unwrap();
        let bin_len = u32_at(&glb, 20 + json_len);
        assert_eq!(&glb[24 + json_len..28 + json_len], b"BIN\0");
        assert!(doc["buffers"][0]["byteLength"].as_u64().unwrap() as usize <= bin_len);
        let names: Vec<&str> = doc["meshes"].as_array().unwrap().iter().filter_map(|m| m["name"].as_str()).collect();
        for want in ["wall", "door", "window", "roof", "floor"] {
            assert!(names.contains(&want), "a `{want}` mesh in {names:?}");
        }
        for a in doc["accessors"].as_array().unwrap() {
            assert_eq!(a["count"].as_u64().unwrap() % 3, 0, "whole triangles");
        }
        // The house stands on y = 0 and up, metres: a few metres tall, not thousands.
        let tallest = doc["accessors"].as_array().unwrap().iter().filter_map(|a| a["max"][1].as_f64()).fold(0.0, f64::max);
        assert!((2.0..30.0).contains(&tallest), "{tallest} m");
    }

    #[test]
    fn an_empty_model_says_there_is_nothing_to_export() {
        let s = Session::new();
        assert!(super::export(&s.model, &[]).is_err());
    }

    fn mesh_names(glb: &[u8]) -> Vec<String> {
        let json_len = u32_at(glb, 12);
        let doc: Value = serde_json::from_slice(&glb[20..20 + json_len]).unwrap();
        doc["meshes"].as_array().unwrap().iter().filter_map(|m| m["name"].as_str().map(str::to_owned)).collect()
    }

    #[test]
    fn save_leaves_out_the_categories_it_hides() {
        let mut s = Session::new();
        s.script(crate::sample::SAMPLE).unwrap();
        let path = std::env::temp_dir().join(format!("bimcraft-test-{}-frame.glb", std::process::id()));
        s.execute("save", &serde_json::json!({"path": path, "hide": ["Walls", "roof"]})).unwrap();
        let names = mesh_names(&std::fs::read(&path).unwrap());
        assert!(!names.iter().any(|n| n == "wall" || n == "roof"), "{names:?}");
        assert!(names.iter().any(|n| n == "floor"), "{names:?}");
        let _ = std::fs::remove_file(&path);
        assert!(s.execute("save", &serde_json::json!({"path": path, "hide": ["wombats"]})).is_err(), "an unknown category is refused");
    }

    #[test]
    fn save_writes_a_glb() {
        let mut s = Session::new();
        s.script(crate::sample::SAMPLE).unwrap();
        let path = std::env::temp_dir().join(format!("bimcraft-test-{}-house.glb", std::process::id()));
        let r = s.execute("save", &serde_json::json!({"path": path})).unwrap();
        assert_eq!(r["format"], "glTF");
        assert_eq!(&std::fs::read(&path).unwrap()[..4], b"glTF");
        let _ = std::fs::remove_file(&path);
    }
}
