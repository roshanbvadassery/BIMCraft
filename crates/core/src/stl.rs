//! Binary STL (`.stl`): the model's shaded surfaces as one triangle soup, millimetres, z up, for slicers
//! and printing a scale model. The same triangles as `.glb` ([`crate::glb::triangles`]).

use crate::model::Model;

/// The model as a binary `.stl`, leaving out the categories in `hide`.
pub fn export(m: &Model, hide: &[String]) -> Result<Vec<u8>, String> {
    let tris = crate::glb::triangles(m, hide)?;
    let count = u32::try_from(tris.len()).map_err(|_| "too many triangles for STL".to_string())?;
    let mut out = Vec::with_capacity(84 + tris.len() * 50);
    let mut header = [b' '; 80];
    let name = format!("BIMCraft {}", m.name);
    for (slot, b) in header.iter_mut().zip(name.bytes()) {
        *slot = b;
    }
    out.extend_from_slice(&header);
    out.extend_from_slice(&count.to_le_bytes());
    for t in &tris {
        let [a, b, c] = t.p;
        let (u, v) = ([b[0] - a[0], b[1] - a[1], b[2] - a[2]], [c[0] - a[0], c[1] - a[1], c[2] - a[2]]);
        let n = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
        let l = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        for k in n {
            out.extend_from_slice(&((k / l) as f32).to_le_bytes());
        }
        for q in t.p {
            for k in q {
                out.extend_from_slice(&(k as f32).to_le_bytes());
            }
        }
        out.extend_from_slice(&0u16.to_le_bytes());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use crate::engine::Session;

    #[test]
    fn the_sample_house_writes_a_binary_stl_in_millimetres() {
        let mut s = Session::new();
        s.script(crate::sample::SAMPLE).unwrap();
        let stl = super::export(&s.model, &[]).unwrap();
        let n = u32::from_le_bytes([stl[80], stl[81], stl[82], stl[83]]) as usize;
        assert!(n > 100);
        assert_eq!(stl.len(), 84 + 50 * n, "50 bytes a triangle after the header");
        let f = |at: usize| f32::from_le_bytes([stl[at], stl[at + 1], stl[at + 2], stl[at + 3]]);
        let zs: Vec<f32> = (0..n).flat_map(|i| [f(84 + 50 * i + 20), f(84 + 50 * i + 32), f(84 + 50 * i + 44)]).collect();
        let top = zs.iter().copied().fold(f32::MIN, f32::max);
        assert!((2000.0..30000.0).contains(&top), "z up in mm: {top}");
    }

    #[test]
    fn save_writes_an_stl_without_what_it_hides() {
        let mut s = Session::new();
        s.script(crate::sample::SAMPLE).unwrap();
        let path = std::env::temp_dir().join(format!("bimcraft-test-{}-house.stl", std::process::id()));
        let all = s.execute("save", &serde_json::json!({"path": path})).unwrap();
        let some = s.execute("save", &serde_json::json!({"path": path, "hide": ["walls"]})).unwrap();
        assert_eq!(all["format"], "STL");
        assert!(some["triangles"].as_u64() < all["triangles"].as_u64());
        let _ = std::fs::remove_file(&path);
    }
}
