//! STEP (ISO 10303-21, AP214 `AUTOMOTIVE_DESIGN`): the model's shaded elements as solids for
//! mechanical CAD (Fusion, SolidWorks, FreeCAD, Onshape). Each element is a `FACETED_BREP` named
//! `<category> <id>`, coloured as the 3D view colours it. Its coplanar triangles that share edges are
//! merged into planar faces with holes (`POLY_LOOP`s), so a wall, slab, beam, column or footing
//! comes out as the exact planar solid it is; a curved surface stays faceted. Millimetres, z up.
//! (IFC, `ifc.rs`, is the BIM exchange; this is for tools that only read STEP.)

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write;

use crate::glb::Tri;
use crate::model::Model;
use crate::render::Rgb;

type V3 = [f64; 3];

/// The model as a `.step` file, leaving out the categories in `hide`.
pub fn export(m: &Model, hide: &[String]) -> Result<String, String> {
    use crate::view3d::Export;
    let solids: Vec<Export> = crate::view3d::export_solids(m)
        .into_iter()
        .filter(|s| {
            let owner = match s {
                Export::Faces { owner, .. } | Export::Mesh { owner, .. } => *owner,
            };
            let category = m.elements.get(owner).map_or("other", |e| e.category());
            !hide.iter().any(|h| h == category)
        })
        .collect();
    if solids.is_empty() {
        return Err("there is nothing in 3D to export; the model has no shaded elements (or `hide` leaves none)".into());
    }
    let mut w = Writer::default();
    let name = if m.name.is_empty() { "BIMCraft model".to_string() } else { m.name.clone() };
    let ctx = w.header(&name);
    let (mut breps, mut styled) = (Vec::new(), Vec::new());
    let mut styles: HashMap<(u8, u8, u8), usize> = HashMap::new();
    let mut parts: HashMap<crate::ids::Id, usize> = HashMap::new();
    let mut faces_count = 0usize;
    for s in solids {
        let (owner, color, faces) = match s {
            // a box, slab or prism: exact, its faces turned out already
            Export::Faces { owner, color, faces } => {
                let faces: Vec<Face> =
                    faces.into_iter().map(|outer| Face { normal: unit_newell(&outer), outer, holes: Vec::new() }).collect();
                (owner, color, vec![faces])
            }
            // a family's form: its triangles, wound one way and merged into planar faces where that closes
            Export::Mesh { owner, color, tris } => {
                let refs: Vec<Tri> = tris.into_iter().map(|p| Tri { p, category: "", color }).collect();
                let refs: Vec<&Tri> = refs.iter().collect();
                (owner, color, pieces(&refs).iter().map(|p| faces(p)).collect())
            }
        };
        let category = m.elements.get(owner).map_or("other", |e| e.category());
        for f in faces {
            if f.is_empty() {
                continue;
            }
            faces_count += f.len();
            if faces_count > crate::glb::MAX_TRIS {
                return Err("the model has too many faces for STEP; hide some categories first".into());
            }
            let k = parts.entry(owner).or_default();
            *k += 1;
            let brep = w.brep(&format!("{category} {owner} part {k}"), &f);
            breps.push(brep);
            let Rgb(r, g, b) = color;
            let style = match styles.get(&(r, g, b)) {
                Some(s) => *s,
                None => {
                    let s = w.style((r, g, b));
                    styles.insert((r, g, b), s);
                    s
                }
            };
            styled.push(w.add(format!("STYLED_ITEM('color',(#{style}),#{brep})")));
        }
    }
    w.footer(ctx, &breps, &styled);
    Ok(w.out)
}

fn unit_newell(pts: &[V3]) -> V3 {
    let n = newell(pts);
    let l = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
    if l > 0.0 { [n[0] / l, n[1] / l, n[2] / l] } else { [0.0, 0.0, 1.0] }
}

/// An element's triangles as the solids it is made of: by colour (a wall's layers), then into the
/// pieces their shared edges join, each without repeated triangles.
fn pieces(ts: &[&Tri]) -> Vec<Vec<[V3; 3]>> {
    let key = |p: V3| [(p[0] * 100.0).round() as i64, (p[1] * 100.0).round() as i64, (p[2] * 100.0).round() as i64];
    let mut by_colour: BTreeMap<(u8, u8, u8), Vec<&Tri>> = BTreeMap::new();
    for t in ts {
        let Rgb(r, g, b) = t.color;
        by_colour.entry((r, g, b)).or_default().push(t);
    }
    let mut out = Vec::new();
    for group in by_colour.into_values() {
        // drop a triangle drawn twice (the same corners, either way round)
        let mut seen = std::collections::HashSet::new();
        let group: Vec<&Tri> = group
            .into_iter()
            .filter(|t| {
                let mut k = [key(t.p[0]), key(t.p[1]), key(t.p[2])];
                k.sort_unstable();
                // and slivers: under 1 mm² or thinner than 0.05 mm, whose plane is noise
                let [a, b, c] = t.p;
                let (u, v) = ([b[0] - a[0], b[1] - a[1], b[2] - a[2]], [c[0] - a[0], c[1] - a[1], c[2] - a[2]]);
                let n = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
                let twice = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
                let longest = [u, v, [c[0] - b[0], c[1] - b[1], c[2] - b[2]]]
                    .iter()
                    .map(|e| (e[0] * e[0] + e[1] * e[1] + e[2] * e[2]).sqrt())
                    .fold(0.0, f64::max);
                twice >= 2.0 && twice / longest.max(1e-9) >= 0.05 && seen.insert(k)
            })
            .collect();
        // union-find over triangles sharing an edge
        let mut parent: Vec<usize> = (0..group.len()).collect();
        fn root(p: &mut [usize], mut i: usize) -> usize {
            while p[i] != i {
                p[i] = p[p[i]];
                i = p[i];
            }
            i
        }
        let mut edge_owner: HashMap<([i64; 3], [i64; 3]), usize> = HashMap::new();
        for (i, t) in group.iter().enumerate() {
            for (a, b) in [(t.p[0], t.p[1]), (t.p[1], t.p[2]), (t.p[2], t.p[0])] {
                let (ka, kb) = (key(a), key(b));
                let e = if ka < kb { (ka, kb) } else { (kb, ka) };
                match edge_owner.get(&e) {
                    Some(&j) => {
                        let (ri, rj) = (root(&mut parent, i), root(&mut parent, j));
                        parent[ri] = rj;
                    }
                    None => {
                        edge_owner.insert(e, i);
                    }
                }
            }
        }
        let mut comps: BTreeMap<usize, Vec<[V3; 3]>> = BTreeMap::new();
        for (i, t) in group.iter().enumerate() {
            let r = root(&mut parent, i);
            comps.entry(r).or_default().push(t.p);
        }
        out.extend(comps.into_values().map(oriented));
    }
    out
}

/// A piece's triangles wound one way round (each turned to agree with the neighbour it was reached
/// from across an edge), then the whole piece turned to face out (positive volume).
fn oriented(mut tris: Vec<[V3; 3]>) -> Vec<[V3; 3]> {
    let key = |p: V3| [(p[0] * 100.0).round() as i64, (p[1] * 100.0).round() as i64, (p[2] * 100.0).round() as i64];
    let mut by_edge: HashMap<([i64; 3], [i64; 3]), Vec<usize>> = HashMap::new();
    for (i, t) in tris.iter().enumerate() {
        for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
            let (ka, kb) = (key(a), key(b));
            by_edge.entry(if ka < kb { (ka, kb) } else { (kb, ka) }).or_default().push(i);
        }
    }
    let runs = |t: &[V3; 3], a: [i64; 3], b: [i64; 3]| (0..3).any(|k| key(t[k]) == a && key(t[(k + 1) % 3]) == b);
    let mut done = vec![false; tris.len()];
    for start in 0..tris.len() {
        if done[start] {
            continue;
        }
        done[start] = true;
        let mut stack = vec![start];
        while let Some(i) = stack.pop() {
            let t = tris[i];
            for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
                let (ka, kb) = (key(a), key(b));
                let e = if ka < kb { (ka, kb) } else { (kb, ka) };
                for &j in by_edge.get(&e).map_or(&[][..], Vec::as_slice) {
                    if done[j] {
                        continue;
                    }
                    // a neighbour wound the same way runs this edge the other way round
                    if runs(&tris[j], ka, kb) {
                        tris[j].swap(1, 2);
                    }
                    done[j] = true;
                    stack.push(j);
                }
            }
        }
    }
    let volume: f64 = tris
        .iter()
        .map(|[a, b, c]| a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0]) + a[2] * (b[0] * c[1] - b[1] * c[0]))
        .sum();
    if volume < 0.0 {
        for t in &mut tris {
            t.swap(1, 2);
        }
    }
    tris
}

/// A planar face: its outer loop and its holes, each a list of corners, and the unit normal it faces.
struct Face {
    outer: Vec<V3>,
    holes: Vec<Vec<V3>>,
    normal: V3,
}

/// An element's triangles as faces: those on one plane that share edges merged into polygons (with
/// holes), where their boundary chains into closed loops; else each triangle a face of its own.
fn faces(ts: &[[V3; 3]]) -> Vec<Face> {
    let tri = |t: &[V3; 3]| *t;
    // weld corners to 0.01 mm
    let key = |p: V3| [(p[0] * 100.0).round() as i64, (p[1] * 100.0).round() as i64, (p[2] * 100.0).round() as i64];
    let mut index: HashMap<[i64; 3], usize> = HashMap::new();
    let mut points: Vec<V3> = Vec::new();
    let mut vid = |p: V3| {
        *index.entry(key(p)).or_insert_with(|| {
            points.push(p);
            points.len() - 1
        })
    };
    // group by plane: unit normal to 1e-4, offset to 0.1 mm
    let mut planes: BTreeMap<[i64; 4], Vec<[usize; 3]>> = BTreeMap::new();
    for t in ts {
        let p = tri(t);
        let n = unit_normal(&p);
        let d = n[0] * p[0][0] + n[1] * p[0][1] + n[2] * p[0][2];
        let k = [(n[0] * 1e4).round() as i64, (n[1] * 1e4).round() as i64, (n[2] * 1e4).round() as i64, (d * 10.0).round() as i64];
        let ids = [vid(p[0]), vid(p[1]), vid(p[2])];
        if ids[0] != ids[1] && ids[1] != ids[2] && ids[0] != ids[2] {
            planes.entry(k).or_default().push(ids);
        }
    }
    let mut out = Vec::new();
    for (k, group) in &planes {
        let n = [k[0] as f64 / 1e4, k[1] as f64 / 1e4, k[2] as f64 / 1e4];
        match merged(group, &points, n) {
            Some(mut fs) => out.append(&mut fs),
            None => out.extend(group.iter().map(|t: &[usize; 3]| {
                let outer: Vec<V3> = t.iter().map(|&i| points[i]).collect();
                let normal = unit_normal(&[outer[0], outer[1], outer[2]]);
                Face { outer, holes: Vec::new(), normal }
            })),
        }
    }
    // T-junctions: a corner of one face lying along another face's edge goes into that edge too, so
    // neighbouring faces share their edges and the shell closes
    for f in &mut out {
        f.outer = with_corners(&f.outer, &points);
        for h in &mut f.holes {
            *h = with_corners(h, &points);
        }
    }
    // the merged faces must still close: every edge run once each way; else the triangles as they are
    let mut runs: HashMap<([i64; 3], [i64; 3]), i32> = HashMap::new();
    for f in &out {
        for lp in std::iter::once(&f.outer).chain(&f.holes) {
            for (i, &a) in lp.iter().enumerate() {
                *runs.entry((key(a), key(lp[(i + 1) % lp.len()]))).or_default() += 1;
            }
        }
    }
    let closed = runs.iter().all(|(&(a, b), &n)| n == 1 && runs.get(&(b, a)) == Some(&1));
    if !closed {
        return ts.iter().map(|t| Face { outer: t.to_vec(), holes: Vec::new(), normal: unit_normal(t) }).collect();
    }
    out
}

/// The loop with every point of `points` that lies on one of its edges (within 0.05 mm, strictly
/// between its ends) put into that edge, in order along it.
fn with_corners(lp: &[V3], points: &[V3]) -> Vec<V3> {
    let mut out = Vec::with_capacity(lp.len());
    for (i, &a) in lp.iter().enumerate() {
        let b = lp[(i + 1) % lp.len()];
        out.push(a);
        let ab = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let len2 = ab[0] * ab[0] + ab[1] * ab[1] + ab[2] * ab[2];
        if len2 < 1e-6 {
            continue;
        }
        let mut on: Vec<(f64, V3)> = points
            .iter()
            .filter_map(|&p| {
                let ap = [p[0] - a[0], p[1] - a[1], p[2] - a[2]];
                let t = (ap[0] * ab[0] + ap[1] * ab[1] + ap[2] * ab[2]) / len2;
                if t <= 1e-6 || t >= 1.0 - 1e-6 {
                    return None;
                }
                let q = [a[0] + ab[0] * t, a[1] + ab[1] * t, a[2] + ab[2] * t];
                let d2 = (p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2) + (p[2] - q[2]).powi(2);
                (d2 < 0.0025).then_some((t, p))
            })
            .collect();
        on.sort_by(|x, y| x.0.total_cmp(&y.0));
        out.extend(on.into_iter().map(|(_, p)| p));
    }
    out
}

/// One plane's triangles as polygons: the directed edges no other triangle runs back along are the
/// boundary; chained head to tail they close into loops, outer ones turning with the normal and holes
/// against it. `None` when the boundary doesn't chain (a T-junction), for the caller to keep triangles.
fn merged(group: &[[usize; 3]], points: &[V3], n: V3) -> Option<Vec<Face>> {
    let mut count: HashMap<(usize, usize), i32> = HashMap::new();
    for t in group {
        for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
            *count.entry((a, b)).or_default() += 1;
        }
    }
    let mut next: HashMap<usize, Vec<usize>> = HashMap::new();
    let mut edges = 0usize;
    for (&(a, b), &c) in &count {
        let back = count.get(&(b, a)).copied().unwrap_or(0);
        for _ in 0..(c - back).max(0) {
            next.entry(a).or_default().push(b);
            edges += 1;
        }
    }
    let mut loops: Vec<Vec<usize>> = Vec::new();
    let mut used = 0usize;
    let mut starts: Vec<usize> = next.keys().copied().collect();
    starts.sort_unstable();
    for s in starts {
        while next.get(&s).is_some_and(|v| !v.is_empty()) {
            let mut lp = vec![s];
            let mut at = s;
            loop {
                let to = next.get_mut(&at)?.pop()?;
                used += 1;
                if to == s {
                    break;
                }
                if lp.len() > edges {
                    return None;
                }
                lp.push(to);
                at = to;
            }
            if lp.len() >= 3 {
                loops.push(lp);
            }
        }
    }
    if used != edges {
        return None;
    }
    // outer loops turn with the normal (Newell), holes against it
    let (mut outers, mut holes): (Vec<Vec<V3>>, Vec<Vec<V3>>) = (Vec::new(), Vec::new());
    for lp in loops {
        let pts: Vec<V3> = lp.iter().map(|&i| points[i]).collect();
        let nl = newell(&pts);
        if nl[0] * n[0] + nl[1] * n[1] + nl[2] * n[2] >= 0.0 {
            outers.push(pts);
        } else {
            holes.push(pts);
        }
    }
    let axis = dominant(n);
    let mut faces: Vec<Face> = outers.into_iter().map(|outer| Face { outer, holes: Vec::new(), normal: n }).collect();
    for h in holes {
        let p = flat(h[0], axis);
        let host = faces.iter_mut().find(|f| inside(p, &f.outer.iter().map(|&q| flat(q, axis)).collect::<Vec<_>>()))?;
        host.holes.push(h);
    }
    // every loop must lie on the plane and not cross itself or another loop of its face, or readers
    // can't bound the face: then the caller keeps this plane's triangles
    let d0 = faces.first().map(|f| f.outer[0]).map_or(0.0, |p| n[0] * p[0] + n[1] * p[1] + n[2] * p[2]);
    for f in &faces {
        let loops: Vec<Vec<(f64, f64)>> =
            std::iter::once(&f.outer).chain(&f.holes).map(|l| l.iter().map(|&q| flat(q, axis)).collect()).collect();
        if std::iter::once(&f.outer).chain(&f.holes).flatten().any(|p| (n[0] * p[0] + n[1] * p[1] + n[2] * p[2] - d0).abs() > 0.5) {
            return None;
        }
        if crossing(&loops) {
            return None;
        }
    }
    Some(faces)
}

/// Whether any two edges of these loops cross, or one loop passes through a corner twice.
fn crossing(loops: &[Vec<(f64, f64)>]) -> bool {
    let mut edges = Vec::new();
    for l in loops {
        let mut seen = std::collections::HashSet::new();
        for (i, a) in l.iter().enumerate() {
            if !seen.insert(((a.0 * 100.0).round() as i64, (a.1 * 100.0).round() as i64)) {
                return true;
            }
            edges.push((*a, l[(i + 1) % l.len()]));
        }
    }
    let cross = |a: (f64, f64), b: (f64, f64), c: (f64, f64)| (b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0);
    for i in 0..edges.len() {
        for j in i + 1..edges.len() {
            let ((p, q), (r, t)) = (edges[i], edges[j]);
            let shares = [p, q].iter().any(|u| [r, t].iter().any(|v| (u.0 - v.0).abs() < 0.01 && (u.1 - v.1).abs() < 0.01));
            if shares {
                continue;
            }
            let (d1, d2, d3, d4) = (cross(r, t, p), cross(r, t, q), cross(p, q, r), cross(p, q, t));
            if d1 * d2 < 0.0 && d3 * d4 < 0.0 {
                return true;
            }
        }
    }
    false
}

fn unit_normal(p: &[V3; 3]) -> V3 {
    let (u, v) = ([p[1][0] - p[0][0], p[1][1] - p[0][1], p[1][2] - p[0][2]], [p[2][0] - p[0][0], p[2][1] - p[0][1], p[2][2] - p[0][2]]);
    let n = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
    let l = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
    if l > 0.0 { [n[0] / l, n[1] / l, n[2] / l] } else { [0.0, 0.0, 1.0] }
}

fn newell(pts: &[V3]) -> V3 {
    let mut n = [0.0; 3];
    for (i, a) in pts.iter().enumerate() {
        let b = pts[(i + 1) % pts.len()];
        n[0] += (a[1] - b[1]) * (a[2] + b[2]);
        n[1] += (a[2] - b[2]) * (a[0] + b[0]);
        n[2] += (a[0] - b[0]) * (a[1] + b[1]);
    }
    n
}

/// The axis a plane faces most along, dropped to see it flat.
fn dominant(n: V3) -> usize {
    let a = [n[0].abs(), n[1].abs(), n[2].abs()];
    if a[0] >= a[1] && a[0] >= a[2] {
        0
    } else if a[1] >= a[2] {
        1
    } else {
        2
    }
}

fn flat(p: V3, axis: usize) -> (f64, f64) {
    match axis {
        0 => (p[1], p[2]),
        1 => (p[2], p[0]),
        _ => (p[0], p[1]),
    }
}

fn inside(p: (f64, f64), poly: &[(f64, f64)]) -> bool {
    let mut odd = false;
    for (i, a) in poly.iter().enumerate() {
        let b = poly[(i + 1) % poly.len()];
        if (a.1 > p.1) != (b.1 > p.1) && p.0 < (b.0 - a.0) * (p.1 - a.1) / (b.1 - a.1) + a.0 {
            odd = !odd;
        }
    }
    odd
}

/// The STEP file as it's written: entity instances numbered from 1.
#[derive(Default)]
struct Writer {
    out: String,
    data: String,
    next: usize,
    /// The product definition shape the representation is of.
    shape: usize,
}

fn text(s: &str) -> String {
    // STEP strings: ' doubled; nothing outside printable ASCII
    s.chars().filter(|c| c.is_ascii() && !c.is_ascii_control()).collect::<String>().replace('\'', "''")
}

/// A direction's component: six places, always with its point.
fn real6(v: f64) -> String {
    format!("{v:.6}").trim_end_matches('0').to_string()
}

/// A STEP real: always with its point (`5000.`, `0.25`).
fn real(v: f64) -> String {
    format!("{v:.4}").trim_end_matches('0').to_string()
}

impl Writer {
    fn add(&mut self, entity: String) -> usize {
        self.next += 1;
        let _ = writeln!(self.data, "#{}={entity};", self.next);
        self.next
    }

    /// The product, its units and its geometric context; returns the context's instance.
    fn header(&mut self, name: &str) -> usize {
        let n = text(name);
        let _ = write!(
            self.out,
            "ISO-10303-21;\nHEADER;\nFILE_DESCRIPTION(('BIMCraft model'),'2;1');\nFILE_NAME('{n}.step','',(''),(''),'BIMCraft','BIMCraft','');\nFILE_SCHEMA(('AUTOMOTIVE_DESIGN {{ 1 0 10303 214 1 1 1 1 }}'));\nENDSEC;\nDATA;\n"
        );
        let app = self.add("APPLICATION_CONTEXT('automotive design')".into());
        self.add(format!("APPLICATION_PROTOCOL_DEFINITION('international standard','automotive_design',2000,#{app})"));
        let pctx = self.add(format!("PRODUCT_CONTEXT('',#{app},'mechanical')"));
        let product = self.add(format!("PRODUCT('{n}','{n}','',(#{pctx}))"));
        let formation = self.add(format!("PRODUCT_DEFINITION_FORMATION('','',#{product})"));
        let dctx = self.add(format!("PRODUCT_DEFINITION_CONTEXT('part definition',#{app},'design')"));
        let def = self.add(format!("PRODUCT_DEFINITION('design','',#{formation},#{dctx})"));
        let shape = self.add(format!("PRODUCT_DEFINITION_SHAPE('','',#{def})"));
        self.add(format!("PRODUCT_RELATED_PRODUCT_CATEGORY('part',$,(#{product}))"));
        let mm = self.add("(LENGTH_UNIT()NAMED_UNIT(*)SI_UNIT(.MILLI.,.METRE.))".into());
        let rad = self.add("(NAMED_UNIT(*)PLANE_ANGLE_UNIT()SI_UNIT($,.RADIAN.))".into());
        let sr = self.add("(NAMED_UNIT(*)SI_UNIT($,.STERADIAN.)SOLID_ANGLE_UNIT())".into());
        let unc = self.add(format!("UNCERTAINTY_MEASURE_WITH_UNIT(LENGTH_MEASURE(0.01),#{mm},'distance_accuracy_value','')"));
        let ctx = self.add(format!(
            "(GEOMETRIC_REPRESENTATION_CONTEXT(3)GLOBAL_UNCERTAINTY_ASSIGNED_CONTEXT((#{unc}))GLOBAL_UNIT_ASSIGNED_CONTEXT((#{mm},#{rad},#{sr}))REPRESENTATION_CONTEXT('',''))"
        ));
        self.shape = shape;
        ctx
    }

    fn brep(&mut self, name: &str, faces: &[Face]) -> usize {
        let mut pts: HashMap<[i64; 3], usize> = HashMap::new();
        let mut point = |w: &mut Writer, p: V3| {
            let k = [(p[0] * 100.0).round() as i64, (p[1] * 100.0).round() as i64, (p[2] * 100.0).round() as i64];
            *pts.entry(k).or_insert_with(|| w.add(format!("CARTESIAN_POINT('',({},{},{}))", real(p[0]), real(p[1]), real(p[2]))))
        };
        let mut face_ids = Vec::with_capacity(faces.len());
        for f in faces {
            let mut bounds = Vec::new();
            for (i, lp) in std::iter::once(&f.outer).chain(&f.holes).enumerate() {
                let ids: Vec<String> = lp.iter().map(|&p| format!("#{}", point(self, p))).collect();
                let poly = self.add(format!("POLY_LOOP('',({}))", ids.join(",")));
                let kind = if i == 0 { "FACE_OUTER_BOUND" } else { "FACE_BOUND" };
                bounds.push(format!("#{}", self.add(format!("{kind}('',#{poly},.T.)"))));
            }
            // its plane, through its first corner, facing out (readers want a FACE_SURFACE, not a bare FACE)
            let (o, n) = (f.outer[0], f.normal);
            let r = if n[2].abs() < 0.9 { [-n[1], n[0], 0.0] } else { [1.0, 0.0, 0.0] };
            let rl = (r[0] * r[0] + r[1] * r[1] + r[2] * r[2]).sqrt();
            let at = self.add(format!("CARTESIAN_POINT('',({},{},{}))", real(o[0]), real(o[1]), real(o[2])));
            let axis = self.add(format!("DIRECTION('',({},{},{}))", real6(n[0]), real6(n[1]), real6(n[2])));
            let refd = self.add(format!("DIRECTION('',({},{},{}))", real6(r[0] / rl), real6(r[1] / rl), real6(r[2] / rl)));
            let placement = self.add(format!("AXIS2_PLACEMENT_3D('',#{at},#{axis},#{refd})"));
            let plane = self.add(format!("PLANE('',#{placement})"));
            face_ids.push(format!("#{}", self.add(format!("FACE_SURFACE('',({}),#{plane},.T.)", bounds.join(",")))));
        }
        let shell = self.add(format!("CLOSED_SHELL('',({}))", face_ids.join(",")));
        self.add(format!("FACETED_BREP('{}',#{shell})", text(name)))
    }

    /// A surface colour, as a presentation style assignment.
    fn style(&mut self, (r, g, b): (u8, u8, u8)) -> usize {
        let c = |v: u8| real(f64::from(v) / 255.0);
        let colour = self.add(format!("COLOUR_RGB('',{},{},{})", c(r), c(g), c(b)));
        let fill_colour = self.add(format!("FILL_AREA_STYLE_COLOUR('',#{colour})"));
        let fill = self.add(format!("FILL_AREA_STYLE('',(#{fill_colour}))"));
        let area = self.add(format!("SURFACE_STYLE_FILL_AREA(#{fill})"));
        let side = self.add(format!("SURFACE_SIDE_STYLE('',(#{area}))"));
        let usage = self.add(format!("SURFACE_STYLE_USAGE(.BOTH.,#{side})"));
        self.add(format!("PRESENTATION_STYLE_ASSIGNMENT((#{usage}))"))
    }

    fn footer(&mut self, ctx: usize, breps: &[usize], styled: &[usize]) {
        let origin = self.add("CARTESIAN_POINT('',(0.,0.,0.))".into());
        let placement = self.add(format!("AXIS2_PLACEMENT_3D('',#{origin},$,$)"));
        let items: Vec<String> = std::iter::once(placement).chain(breps.iter().copied()).map(|i| format!("#{i}")).collect();
        let rep = self.add(format!("FACETED_BREP_SHAPE_REPRESENTATION('',({}),#{ctx})", items.join(",")));
        let shape = self.shape;
        self.add(format!("SHAPE_DEFINITION_REPRESENTATION(#{shape},#{rep})"));
        if !styled.is_empty() {
            let list: Vec<String> = styled.iter().map(|i| format!("#{i}")).collect();
            self.add(format!("MECHANICAL_DESIGN_GEOMETRIC_PRESENTATION_REPRESENTATION('',({}),#{ctx})", list.join(",")));
        }
        self.out.push_str(&self.data);
        self.out.push_str("ENDSEC;\nEND-ISO-10303-21;\n");
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use crate::engine::Session;
    use crate::step::{V, parse};

    fn of<'a>(f: &'a crate::step::File, kind: &str) -> Vec<&'a [V]> {
        f.entities.values().filter(|e| e.name == kind).map(|e| e.args.as_slice()).collect()
    }

    #[test]
    fn each_element_is_a_named_coloured_faceted_solid() {
        let mut s = Session::new();
        s.script(crate::sample::SAMPLE).unwrap();
        let text = super::export(&s.model, &[]).unwrap();
        let f = parse(text.as_bytes()).unwrap();
        let breps = of(&f, "FACETED_BREP");
        let walls: std::collections::BTreeSet<&str> = breps
            .iter()
            .filter_map(|p| match p.first() {
                Some(V::Str(n)) if n.starts_with("wall ") => n.split(' ').nth(1),
                _ => None,
            })
            .collect();
        assert_eq!(walls.len(), 7, "solids for each of the house's seven walls (a layered wall's layers each its own)");
        assert_eq!(of(&f, "STYLED_ITEM").len(), breps.len(), "each one coloured");
        assert!(text.contains("SI_UNIT(.MILLI.,.METRE.)"));
    }

    #[test]
    fn a_column_and_a_beam_are_six_face_closed_solids() {
        let mut s = Session::new();
        s.script("new {}\nstructural_column {\"at\": [0, 0]}\nbeam {\"points\": [[0, 0], [5000, 0]]}").unwrap();
        let text = super::export(&s.model, &[]).unwrap();
        let f = parse(text.as_bytes()).unwrap();
        // every face of a box is one planar polygon: four corners, six faces
        let loops = of(&f, "POLY_LOOP");
        let faces = of(&f, "FACE_SURFACE");
        assert_eq!(of(&f, "FACETED_BREP").len(), 2);
        assert_eq!(faces.len(), 12, "two boxes, six faces each");
        assert!(loops.iter().all(|p| matches!(p.get(1), Some(V::List(l)) if l.len() == 4)), "each face a rectangle");
        // closed: every edge between corners is used by exactly two faces, once each way
        let mut edges: HashMap<(u64, u64), i32> = HashMap::new();
        for p in &loops {
            let Some(V::List(l)) = p.get(1) else { continue };
            let ids: Vec<u64> = l.iter().filter_map(|v| if let V::Ref(r) = v { Some(*r) } else { None }).collect();
            for (i, a) in ids.iter().enumerate() {
                let b = ids[(i + 1) % ids.len()];
                *edges.entry((*a, b)).or_default() += 1;
            }
        }
        assert!(edges.iter().all(|(&(a, b), &n)| n == 1 && edges.get(&(b, a)) == Some(&1)), "a closed, consistently turned shell");
    }

    #[test]
    fn save_writes_a_step_file() {
        let mut s = Session::new();
        s.script(crate::sample::SAMPLE).unwrap();
        let path = std::env::temp_dir().join(format!("bimcraft-test-{}-house.step", std::process::id()));
        let r = s.execute("save", &serde_json::json!({"path": path, "hide": ["walls"]})).unwrap();
        assert_eq!(r["format"], "STEP AP214");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("ISO-10303-21;") && !text.contains("'wall "));
        let _ = std::fs::remove_file(&path);
    }
}
