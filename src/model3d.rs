//! 3D model previews (STL, OBJ, glTF): triangles read from the file and drawn by a
//! small software renderer with a depth buffer, turned by dragging.
use crate::i18n::t;
use crate::theme::Theme;
use crate::util::{ext_of, Res};
use base64::Engine;
use std::path::Path;

/// Models beyond this are shown incomplete, with a note.
const MAX_TRIANGLES: usize = 4_000_000;
/// Parsing a model needs all of its files in memory.
pub const MAX_FILE: u64 = 768 * 1024 * 1024;
/// Scene graphs can share and even loop nodes: this is how many get visited.
const MAX_NODE_VISITS: usize = 200_000;
/// While dragging, at most this many triangles are drawn per frame.
const DRAG_TRIANGLES: usize = 250_000;
const DEFAULT_COLOR: [u8; 3] = [166, 176, 196];

pub fn is_model(name: &str) -> bool {
    matches!(ext_of(name).as_str(), "stl" | "obj" | "gltf" | "glb")
}

#[derive(Clone, Copy)]
pub struct Tri {
    pub v: [[f32; 3]; 3],
    pub color: [u8; 3],
}

pub struct Mesh {
    pub format: &'static str,
    pub tris: Vec<Tri>,
    pub min: [f32; 3],
    pub max: [f32; 3],
    /// Some triangles were left out.
    pub truncated: bool,
}

/// Read a model. `dir` is the folder of the file, for glTF's separate buffer files.
pub fn load(name: &str, bytes: &[u8], dir: Option<&Path>) -> Res<Mesh> {
    let (format, mut tris) = match ext_of(name).as_str() {
        "stl" => ("STL", parse_stl(bytes)?),
        "obj" => ("OBJ", parse_obj(&String::from_utf8_lossy(bytes))),
        "glb" => ("glTF binary", parse_glb(bytes, dir)?),
        _ => ("glTF", parse_gltf_json(bytes, None, dir)?),
    };
    tris.retain(|t| t.v.iter().flatten().all(|c| c.is_finite()));
    if tris.is_empty() {
        return Err(t("This file has no triangles to show."));
    }
    let truncated = tris.len() > MAX_TRIANGLES;
    tris.truncate(MAX_TRIANGLES);
    let (mut min, mut max) = ([f32::MAX; 3], [f32::MIN; 3]);
    for p in tris.iter().flat_map(|t| t.v.iter()) {
        for i in 0..3 {
            min[i] = min[i].min(p[i]);
            max[i] = max[i].max(p[i]);
        }
    }
    Ok(Mesh { format, tris, min, max, truncated })
}

// ---------------------------------------------------------------- STL

fn parse_stl(b: &[u8]) -> Res<Vec<Tri>> {
    // Binary files can start with "solid" too; their exact size gives them away.
    if b.len() >= 84 {
        let n = u32::from_le_bytes([b[80], b[81], b[82], b[83]]) as usize;
        if n.checked_mul(50).and_then(|s| s.checked_add(84)) == Some(b.len()) {
            let f = |c: &[u8], at: usize| f32::from_le_bytes([c[at], c[at + 1], c[at + 2], c[at + 3]]);
            return Ok(b[84..]
                .chunks_exact(50)
                .take(MAX_TRIANGLES + 1)
                .map(|c| Tri {
                    v: [[f(c, 12), f(c, 16), f(c, 20)], [f(c, 24), f(c, 28), f(c, 32)], [f(c, 36), f(c, 40), f(c, 44)]],
                    color: DEFAULT_COLOR,
                })
                .collect());
        }
    }
    if !b.get(..512.min(b.len())).is_some_and(|h| String::from_utf8_lossy(h).trim_start().starts_with("solid")) {
        return Err(t("Not a valid STL file."));
    }
    let text = String::from_utf8_lossy(b);
    let mut tris = vec![];
    let mut v: Vec<[f32; 3]> = vec![];
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        if parts.next() == Some("vertex") {
            let c: Vec<f32> = parts.filter_map(|x| x.parse().ok()).collect();
            if let [x, y, z] = c[..] {
                v.push([x, y, z]);
                if let [a, b, c] = v[..] {
                    tris.push(Tri { v: [a, b, c], color: DEFAULT_COLOR });
                    v.clear();
                }
            }
        }
    }
    Ok(tris)
}

// ---------------------------------------------------------------- OBJ

fn parse_obj(text: &str) -> Vec<Tri> {
    let mut verts: Vec<[f32; 3]> = vec![];
    let mut tris = vec![];
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        match parts.next() {
            Some("v") => {
                let c: Vec<f32> = parts.take(3).filter_map(|x| x.parse().ok()).collect();
                if let [x, y, z] = c[..] {
                    verts.push([x, y, z]);
                }
            }
            Some("f") => {
                let face: Vec<[f32; 3]> = parts
                    .filter_map(|p| {
                        // "12/3/4": only the position index matters; negative counts from the end.
                        let i: i64 = p.split('/').next()?.parse().ok()?;
                        let at = if i < 0 { verts.len() as i64 + i } else { i - 1 };
                        verts.get(usize::try_from(at).ok()?).copied()
                    })
                    .collect();
                for k in 2..face.len() {
                    tris.push(Tri { v: [face[0], face[k - 1], face[k]], color: DEFAULT_COLOR });
                }
                if tris.len() > MAX_TRIANGLES {
                    break;
                }
            }
            _ => {}
        }
    }
    tris
}

// ---------------------------------------------------------------- glTF

fn parse_glb(b: &[u8], dir: Option<&Path>) -> Res<Vec<Tri>> {
    if b.len() < 20 || &b[..4] != b"glTF" {
        return Err(t("Not a valid glTF file."));
    }
    let u32_at = |at: usize| b.get(at..at + 4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]) as usize);
    let mut at = 12;
    let (mut json, mut bin) = (None, None);
    while let (Some(len), Some(kind)) = (u32_at(at), b.get(at + 4..at + 8)) {
        let Some(chunk) = at.checked_add(8).and_then(|s| s.checked_add(len)).and_then(|e| b.get(at + 8..e)) else { break };
        match kind {
            b"JSON" => json = Some(chunk),
            b"BIN\0" => bin = Some(chunk),
            _ => {}
        }
        at += 8 + len;
    }
    parse_gltf_json(json.ok_or_else(|| t("Not a valid glTF file."))?, bin, dir)
}

type Mat4 = [f32; 16];
const IDENTITY: Mat4 = [1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.];

fn mul(a: &Mat4, b: &Mat4) -> Mat4 {
    let mut m = [0.0; 16];
    for c in 0..4 {
        for r in 0..4 {
            m[c * 4 + r] = (0..4).map(|k| a[k * 4 + r] * b[c * 4 + k]).sum();
        }
    }
    m
}

fn transform(m: &Mat4, p: [f32; 3]) -> [f32; 3] {
    [m[0] * p[0] + m[4] * p[1] + m[8] * p[2] + m[12], m[1] * p[0] + m[5] * p[1] + m[9] * p[2] + m[13], m[2] * p[0] + m[6] * p[1] + m[10] * p[2] + m[14]]
}

fn floats<const N: usize>(v: Option<&serde_json::Value>, default: [f32; N]) -> [f32; N] {
    let mut out = default;
    if let Some(list) = v.and_then(|v| v.as_array()) {
        for (o, x) in out.iter_mut().zip(list) {
            *o = x.as_f64().unwrap_or(f64::from(*o)) as f32;
        }
    }
    out
}

fn node_matrix(node: &serde_json::Value) -> Mat4 {
    if node.get("matrix").is_some() {
        return floats(node.get("matrix"), IDENTITY);
    }
    let [tx, ty, tz] = floats(node.get("translation"), [0.0; 3]);
    let [x, y, z, w] = floats(node.get("rotation"), [0., 0., 0., 1.]);
    let [sx, sy, sz] = floats(node.get("scale"), [1.0; 3]);
    let (xx, yy, zz, xy, xz, yz, wx, wy, wz) = (x * x, y * y, z * z, x * y, x * z, y * z, w * x, w * y, w * z);
    [
        (1. - 2. * (yy + zz)) * sx,
        2. * (xy + wz) * sx,
        2. * (xz - wy) * sx,
        0.,
        2. * (xy - wz) * sy,
        (1. - 2. * (xx + zz)) * sy,
        2. * (yz + wx) * sy,
        0.,
        2. * (xz + wy) * sz,
        2. * (yz - wx) * sz,
        (1. - 2. * (xx + yy)) * sz,
        0.,
        tx,
        ty,
        tz,
        1.,
    ]
}

struct Gltf<'a> {
    root: &'a serde_json::Value,
    buffers: Vec<std::borrow::Cow<'a, [u8]>>,
}

impl Gltf<'_> {
    fn list(&self, key: &str) -> &[serde_json::Value] {
        self.root.get(key).and_then(|v| v.as_array()).map_or(&[], Vec::as_slice)
    }

    /// An accessor's data, its element count, and the distance between elements.
    fn accessor(&self, index: usize) -> Option<(&[u8], usize, usize, u64, &str)> {
        let a = self.list("accessors").get(index)?;
        let component = a.get("componentType")?.as_u64()?;
        let count = a.get("count")?.as_u64()? as usize;
        let kind = a.get("type")?.as_str()?;
        let view = self.list("bufferViews").get(a.get("bufferView")?.as_u64()? as usize)?;
        let buffer = self.buffers.get(view.get("buffer")?.as_u64()? as usize)?;
        let start = view.get("byteOffset").and_then(|v| v.as_u64()).unwrap_or(0) as usize + a.get("byteOffset").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
        let component_size = match component {
            5120 | 5121 => 1,
            5122 | 5123 => 2,
            5125 | 5126 => 4,
            _ => return None,
        };
        let parts = match kind {
            "SCALAR" => 1,
            "VEC2" => 2,
            "VEC3" => 3,
            "VEC4" => 4,
            _ => return None,
        };
        let stride = view.get("byteStride").and_then(|v| v.as_u64()).map_or(component_size * parts, |s| s as usize);
        Some((buffer.get(start..)?, count, stride, component, kind))
    }

    fn positions(&self, index: usize) -> Option<Vec<[f32; 3]>> {
        let (data, count, stride, component, kind) = self.accessor(index)?;
        if component != 5126 || kind != "VEC3" {
            return None;
        }
        let f = |at: usize| Some(f32::from_le_bytes(data.get(at..at + 4)?.try_into().ok()?));
        (0..count).map(|i| Some([f(i * stride)?, f(i * stride + 4)?, f(i * stride + 8)?])).collect()
    }

    fn indices(&self, index: usize) -> Option<Vec<usize>> {
        let (data, count, stride, component, kind) = self.accessor(index)?;
        if kind != "SCALAR" {
            return None;
        }
        (0..count)
            .map(|i| {
                let at = i * stride;
                Some(match component {
                    5121 => usize::from(*data.get(at)?),
                    5123 => usize::from(u16::from_le_bytes(data.get(at..at + 2)?.try_into().ok()?)),
                    5125 => u32::from_le_bytes(data.get(at..at + 4)?.try_into().ok()?) as usize,
                    _ => return None,
                })
            })
            .collect()
    }

    fn material_color(&self, index: Option<u64>) -> [u8; 3] {
        let factor = index
            .and_then(|i| self.list("materials").get(i as usize))
            .and_then(|m| m.get("pbrMetallicRoughness"))
            .and_then(|p| p.get("baseColorFactor"));
        match factor {
            // Linear colour values to display values.
            Some(_) => {
                let [r, g, b, _] = floats(factor, [1.0; 4]);
                [r, g, b].map(|c| (c.clamp(0.0, 1.0).powf(1.0 / 2.2) * 255.0) as u8)
            }
            None => DEFAULT_COLOR,
        }
    }

    fn mesh_triangles(&self, mesh: usize, m: &Mat4, out: &mut Vec<Tri>) {
        let Some(mesh) = self.list("meshes").get(mesh) else { return };
        for prim in mesh.get("primitives").and_then(|p| p.as_array()).map_or(&[][..], Vec::as_slice) {
            let mode = prim.get("mode").and_then(|v| v.as_u64()).unwrap_or(4);
            let Some(position) = prim.get("attributes").and_then(|a| a.get("POSITION")).and_then(|v| v.as_u64()) else { continue };
            let Some(points) = self.positions(position as usize) else { continue };
            let points: Vec<[f32; 3]> = points.into_iter().map(|p| transform(m, p)).collect();
            let order: Vec<usize> = match prim.get("indices").and_then(|v| v.as_u64()) {
                Some(i) => self.indices(i as usize).unwrap_or_default(),
                None => (0..points.len()).collect(),
            };
            let color = self.material_color(prim.get("material").and_then(|v| v.as_u64()));
            let mut push = |a: usize, b: usize, c: usize| {
                if let (Some(&a), Some(&b), Some(&c)) = (order.get(a).and_then(|i| points.get(*i)), order.get(b).and_then(|i| points.get(*i)), order.get(c).and_then(|i| points.get(*i))) {
                    out.push(Tri { v: [a, b, c], color });
                }
            };
            match mode {
                4 => (0..order.len() / 3).for_each(|i| push(i * 3, i * 3 + 1, i * 3 + 2)),
                5 => (2..order.len()).for_each(|i| if i % 2 == 0 { push(i - 2, i - 1, i) } else { push(i - 1, i - 2, i) }),
                6 => (2..order.len()).for_each(|i| push(0, i - 1, i)),
                _ => {}
            }
        }
    }

    fn visit(&self, node: usize, parent: &Mat4, depth: usize, visits: &mut usize, out: &mut Vec<Tri>) {
        *visits += 1;
        if depth > 64 || *visits > MAX_NODE_VISITS || out.len() > MAX_TRIANGLES {
            return;
        }
        let Some(n) = self.list("nodes").get(node) else { return };
        let m = mul(parent, &node_matrix(n));
        if let Some(mesh) = n.get("mesh").and_then(|v| v.as_u64()) {
            self.mesh_triangles(mesh as usize, &m, out);
        }
        for child in n.get("children").and_then(|c| c.as_array()).map_or(&[][..], Vec::as_slice) {
            if let Some(c) = child.as_u64() {
                self.visit(c as usize, &m, depth + 1, visits, out);
            }
        }
    }
}

/// A buffer file named by a glTF: only a plain path below the model's own folder.
fn read_sibling(dir: Option<&Path>, uri: &str) -> Res<Vec<u8>> {
    let dir = dir.ok_or_else(|| t("This glTF uses a separate buffer file, which cannot be read from here."))?;
    let rel = Path::new(uri);
    if rel.components().any(|c| !matches!(c, std::path::Component::Normal(_))) {
        return Err(t("This glTF uses a separate buffer file, which cannot be read from here."));
    }
    let path = dir.join(rel);
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > MAX_FILE) {
        return Err(t("This model is too large to preview."));
    }
    std::fs::read(path).map_err(|e| format!("{uri}: {e}"))
}

fn parse_gltf_json(json: &[u8], bin: Option<&[u8]>, dir: Option<&Path>) -> Res<Vec<Tri>> {
    let root: serde_json::Value = serde_json::from_slice(json).map_err(|e| format!("glTF: {e}"))?;
    let required = root.get("extensionsRequired").and_then(|e| e.as_array());
    if required.is_some_and(|e| e.iter().any(|x| x.as_str() == Some("KHR_draco_mesh_compression") || x.as_str() == Some("EXT_meshopt_compression"))) {
        return Err(t("This glTF uses mesh compression, which the preview does not support."));
    }
    let mut buffers = vec![];
    for (i, b) in root.get("buffers").and_then(|b| b.as_array()).map_or(&[][..], Vec::as_slice).iter().enumerate() {
        buffers.push(match b.get("uri").and_then(|u| u.as_str()) {
            Some(uri) if uri.starts_with("data:") => {
                let data = uri.split_once(',').map_or("", |(_, d)| d);
                base64::engine::general_purpose::STANDARD.decode(data).map_err(|e| format!("glTF: {e}"))?.into()
            }
            Some(uri) => read_sibling(dir, uri)?.into(),
            None if i == 0 => bin.unwrap_or_default().into(),
            None => std::borrow::Cow::Borrowed(&[][..]),
        });
    }
    let g = Gltf { root: &root, buffers };
    let scene = root.get("scene").and_then(|s| s.as_u64()).unwrap_or(0) as usize;
    let mut tris = vec![];
    let mut visits = 0;
    let scene_nodes = g.list("scenes").get(scene).and_then(|s| s.get("nodes")).and_then(|n| n.as_array());
    match scene_nodes {
        Some(nodes) => nodes.iter().filter_map(|n| n.as_u64()).for_each(|n| g.visit(n as usize, &IDENTITY, 0, &mut visits, &mut tris)),
        // No scene: every mesh as it is.
        None => (0..g.list("meshes").len()).for_each(|m| g.mesh_triangles(m, &IDENTITY, &mut tris)),
    }
    Ok(tris)
}

// ---------------------------------------------------------------- rendering

pub struct Camera {
    pub yaw: f32,
    pub pitch: f32,
    pub zoom: f32,
}

impl Default for Camera {
    fn default() -> Self {
        Self { yaw: -0.6, pitch: 0.45, zoom: 1.0 }
    }
}

/// Draw the mesh into an RGBA image: lit by a light at the viewer, transparent where
/// there is no model. `step` > 1 draws only every step-th triangle (while dragging).
pub fn render(mesh: &Mesh, cam: &Camera, w: usize, h: usize, step: usize) -> Vec<u8> {
    let mut rgba = vec![0u8; w * h * 4];
    let mut depth = vec![f32::MIN; w * h];
    let center = [0, 1, 2].map(|i| (mesh.min[i] + mesh.max[i]) / 2.0);
    let radius = (0..3).map(|i| (mesh.max[i] - mesh.min[i]) / 2.0).map(|d| d * d).sum::<f32>().sqrt().max(1e-6);
    let (sy, cy, sp, cp) = (cam.yaw.sin(), cam.yaw.cos(), cam.pitch.sin(), cam.pitch.cos());
    let view = |p: [f32; 3]| {
        let (x, y, z) = ((p[0] - center[0]) / radius, (p[1] - center[1]) / radius, (p[2] - center[2]) / radius);
        let (x, z) = (x * cy + z * sy, -x * sy + z * cy);
        let (y, z) = (y * cp - z * sp, y * sp + z * cp);
        [x, y, z]
    };
    let distance = 3.2 / cam.zoom.max(0.05);
    let scale = w.min(h) as f32 * 0.5 * 2.9;
    let (cx, cyy) = (w as f32 / 2.0, h as f32 / 2.0);
    for tri in mesh.tris.iter().step_by(step.max(1)) {
        let v = tri.v.map(view);
        // Behind the viewer: leave it out rather than clip it.
        if v.iter().any(|p| distance - p[2] < 0.05) {
            continue;
        }
        let e1 = [v[1][0] - v[0][0], v[1][1] - v[0][1], v[1][2] - v[0][2]];
        let e2 = [v[2][0] - v[0][0], v[2][1] - v[0][1], v[2][2] - v[0][2]];
        let n = [e1[1] * e2[2] - e1[2] * e2[1], e1[2] * e2[0] - e1[0] * e2[2], e1[0] * e2[1] - e1[1] * e2[0]];
        let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        if len < 1e-12 {
            continue;
        }
        // Lit from the viewer and a little above, whichever way the triangle faces.
        let facing = ((n[2] * 0.87 + n[1] * 0.5) / len).abs();
        let light = 0.28 + 0.72 * facing;
        let shade = tri.color.map(|c| (f32::from(c) * light).min(255.0) as u8);
        let project = |p: [f32; 3]| {
            let d = 1.0 / (distance - p[2]);
            (cx + p[0] * d * scale, cyy - p[1] * d * scale, d)
        };
        let [a, b, c] = v.map(project);
        let area = (b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0);
        if area.abs() < 1e-6 {
            continue;
        }
        let x0 = a.0.min(b.0).min(c.0).floor().max(0.0) as usize;
        let y0 = a.1.min(b.1).min(c.1).floor().max(0.0) as usize;
        let x1 = (a.0.max(b.0).max(c.0).ceil() as isize + 1).clamp(0, w as isize) as usize;
        let y1 = (a.1.max(b.1).max(c.1).ceil() as isize + 1).clamp(0, h as isize) as usize;
        for y in y0..y1 {
            let py = y as f32 + 0.5;
            for x in x0..x1 {
                let px = x as f32 + 0.5;
                let w0 = ((b.0 - px) * (c.1 - py) - (b.1 - py) * (c.0 - px)) / area;
                let w1 = ((c.0 - px) * (a.1 - py) - (c.1 - py) * (a.0 - px)) / area;
                let w2 = 1.0 - w0 - w1;
                if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                    continue;
                }
                // 1/distance interpolates linearly on the screen: larger is nearer.
                let z = w0 * a.2 + w1 * b.2 + w2 * c.2;
                let at = y * w + x;
                if z > depth[at] {
                    depth[at] = z;
                    let px = &mut rgba[at * 4..at * 4 + 4];
                    px[..3].copy_from_slice(&shade);
                    px[3] = 255;
                }
            }
        }
    }
    rgba
}

// ---------------------------------------------------------------- the view

/// A length with no more digits than it needs: `1`, `2.5`, `0.0312`.
fn size_text(v: f32) -> String {
    let text = format!("{v:.4}");
    text.trim_end_matches('0').trim_end_matches('.').to_string()
}

pub struct ModelView {
    mesh: Mesh,
    cam: Camera,
    texture: Option<egui::TextureHandle>,
    /// What the texture shows: size, yaw, pitch, zoom, and whether it is the full-quality one.
    drawn: Option<([usize; 2], [f32; 3], bool)>,
}

impl ModelView {
    pub fn new(mesh: Mesh) -> Self {
        Self { mesh, cam: Camera::default(), texture: None, drawn: None }
    }

    #[cfg(test)]
    pub fn describe(&self) -> String {
        format!("{} tris={} min={:?} max={:?}", self.mesh.format, self.mesh.tris.len(), self.mesh.min, self.mesh.max)
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, name: &str, key: &str, th: &Theme) {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(name).strong().color(th.accent));
            let size = [0, 1, 2].map(|i| self.mesh.max[i] - self.mesh.min[i]);
            let mut facts = format!("{} · {} {} · {} × {} × {}", self.mesh.format, crate::table::fmt_count(self.mesh.tris.len() as u64), t("triangles"), size_text(size[0]), size_text(size[1]), size_text(size[2]));
            if self.mesh.truncated {
                facts.push_str(&format!(" · {}", t("(truncated)")));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(t("Reset view")).clicked() {
                    self.cam = Camera::default();
                }
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    ui.add(egui::Label::new(egui::RichText::new(facts).color(th.weak)).truncate());
                });
            });
        });
        ui.separator();
        let (rect, resp) = ui.allocate_exact_size(ui.available_size(), egui::Sense::click_and_drag());
        let dragging = resp.dragged();
        if dragging {
            let d = resp.drag_delta();
            self.cam.yaw += d.x * 0.01;
            self.cam.pitch = (self.cam.pitch + d.y * 0.01).clamp(-1.55, 1.55);
        }
        if resp.double_clicked() {
            self.cam = Camera::default();
        }
        let wheel = if resp.hovered() { ui.input(|i| i.smooth_scroll_delta.y) } else { 0.0 };
        if wheel != 0.0 {
            self.cam.zoom = (self.cam.zoom * (wheel * 0.003).exp()).clamp(0.2, 20.0);
        }
        let moving = dragging || wheel != 0.0;
        if moving {
            // One more frame after it stops, at full quality.
            ui.ctx().request_repaint();
        }
        // Full quality (twice the pixels) when still, plain and possibly thinned out while moving.
        let ppp = ui.ctx().pixels_per_point();
        let quality = if moving { 1.0 } else { 2.0 };
        // Within what the graphics card takes: going over it is a fatal error.
        let max_side = ui.input(|i| i.max_texture_side).clamp(1, 3200);
        let px = |v: f32| ((v * ppp * quality) as usize).clamp(1, max_side);
        let size = [px(rect.width()), px(rect.height())];
        let state = (size, [self.cam.yaw, self.cam.pitch, self.cam.zoom], !moving);
        if self.drawn != Some(state) {
            let step = if moving { (self.mesh.tris.len() / DRAG_TRIANGLES).max(1) } else { 1 };
            let rgba = render(&self.mesh, &self.cam, size[0], size[1], step);
            let image = egui::ColorImage::from_rgba_premultiplied(size, &rgba);
            match &mut self.texture {
                Some(tex) => tex.set(image, egui::TextureOptions::LINEAR),
                None => self.texture = Some(crate::preview::upload_texture(ui.ctx(), format!("model:{key}"), image)),
            }
            self.drawn = Some(state);
        }
        if let Some(tex) = &self.texture {
            ui.painter().image(tex.id(), rect, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), egui::Color32::WHITE);
        }
        let hint = resp.on_hover_cursor(egui::CursorIcon::Grab);
        ui.painter().text(hint.rect.left_bottom() + egui::vec2(6.0, -6.0), egui::Align2::LEFT_BOTTOM, t("Drag to turn · scroll to zoom · double-click to reset"), egui::FontId::proportional(11.5), th.weak);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CUBE_OBJ: &str = "# a cube\nv 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\nv 0 0 1\nv 1 0 1\nv 1 1 1\nv 0 1 1\nvn 0 0 1\nf 1/1/1 2/1/1 3/1/1 4/1/1\nf -4 -3 -2 -1\nf 1 2 6 5\nf 2 3 7 6\nf 3 4 8 7\nf 4 1 5 8\nf 1 99 2\n";

    fn binary_stl(tris: u32) -> Vec<u8> {
        let mut b = b"solid looks like text".to_vec();
        b.resize(80, b' ');
        b.extend(tris.to_le_bytes());
        for i in 0..tris {
            for f in [0.0f32, 0.0, 1.0, i as f32, 0.0, 0.0, i as f32 + 1.0, 0.0, 0.0, i as f32, 1.0, 0.0] {
                b.extend(f.to_le_bytes());
            }
            b.extend([0, 0]);
        }
        b
    }

    #[test]
    fn stl_binary_and_text() {
        let m = load("a.stl", &binary_stl(3), None).unwrap();
        assert_eq!(m.format, "STL");
        assert_eq!(m.tris.len(), 3);
        assert_eq!(m.max, [3.0, 1.0, 0.0]);
        let text = "solid t\nfacet normal 0 0 1\nouter loop\nvertex 0 0 0\nvertex 2 0 0\nvertex 0 1.5 0\nendloop\nendfacet\nendsolid t\n";
        let m = load("a.stl", text.as_bytes(), None).unwrap();
        assert_eq!(m.tris.len(), 1);
        assert_eq!(m.max[1], 1.5);
        assert!(load("a.stl", b"not a model", None).is_err());
    }

    #[test]
    fn obj_faces_become_triangles() {
        let m = load("a.obj", CUBE_OBJ.as_bytes(), None).unwrap();
        // Six quads, two triangles each; the face with a bad index has too few points left.
        assert_eq!(m.tris.len(), 12);
        assert_eq!(m.min, [0.0; 3]);
        assert_eq!(m.max, [1.0; 3]);
        assert!(load("a.obj", b"# nothing", None).is_err());
    }

    fn gltf_json(uri: &str) -> Vec<u8> {
        serde_json::json!({
            "scene": 0, "scenes": [{"nodes": [0]}],
            "nodes": [{"mesh": 0, "translation": [10.0, 0.0, 0.0], "children": [1]}, {"mesh": 0, "scale": [2.0, 2.0, 2.0]}],
            "meshes": [{"primitives": [{"attributes": {"POSITION": 1}, "indices": 0, "material": 0}]}],
            "materials": [{"pbrMetallicRoughness": {"baseColorFactor": [1.0, 0.0, 0.0, 1.0]}}],
            "accessors": [
                {"bufferView": 0, "componentType": 5123, "count": 3, "type": "SCALAR"},
                {"bufferView": 1, "componentType": 5126, "count": 3, "type": "VEC3"}
            ],
            "bufferViews": [{"buffer": 0, "byteOffset": 0, "byteLength": 6}, {"buffer": 0, "byteOffset": 8, "byteLength": 36}],
            "buffers": [{"byteLength": 44, "uri": uri}]
        })
        .to_string()
        .into_bytes()
    }

    fn gltf_buffer() -> Vec<u8> {
        let mut b = vec![];
        for i in [0u16, 1, 2, 0] {
            b.extend(i.to_le_bytes());
        }
        for f in [0.0f32, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0] {
            b.extend(f.to_le_bytes());
        }
        b
    }

    #[test]
    fn gltf_embedded_with_node_transforms() {
        let uri = format!("data:application/octet-stream;base64,{}", base64::engine::general_purpose::STANDARD.encode(gltf_buffer()));
        let m = load("a.gltf", &gltf_json(&uri), None).unwrap();
        assert_eq!(m.tris.len(), 2, "the mesh is used by two nodes");
        // The child is scaled by 2 and moved with its parent.
        assert_eq!(m.max, [12.0, 2.0, 0.0]);
        assert_eq!(m.min, [10.0, 0.0, 0.0]);
        assert_eq!(m.tris[0].color, [255, 0, 0]);
    }

    #[test]
    fn glb_container() {
        let mut json = gltf_json("x");
        // A GLB's first buffer has no uri.
        let mut v: serde_json::Value = serde_json::from_slice(&json).unwrap();
        v["buffers"][0].as_object_mut().unwrap().remove("uri");
        json = v.to_string().into_bytes();
        while json.len() % 4 != 0 {
            json.push(b' ');
        }
        let bin = gltf_buffer();
        let mut f = b"glTF".to_vec();
        f.extend(2u32.to_le_bytes());
        f.extend(((12 + 8 + json.len() + 8 + bin.len()) as u32).to_le_bytes());
        f.extend((json.len() as u32).to_le_bytes());
        f.extend(b"JSON");
        f.extend(&json);
        f.extend((bin.len() as u32).to_le_bytes());
        f.extend(b"BIN\0");
        f.extend(&bin);
        assert_eq!(load("a.glb", &f, None).unwrap().tris.len(), 2);
        assert!(load("a.glb", b"glTF", None).is_err());
    }

    #[test]
    fn gltf_buffer_files_stay_in_the_models_folder() {
        let dir = std::env::temp_dir().join(format!("mf-gltf-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("m.bin"), gltf_buffer()).unwrap();
        assert_eq!(load("a.gltf", &gltf_json("m.bin"), Some(&dir)).unwrap().tris.len(), 2);
        assert!(load("a.gltf", &gltf_json("../m.bin"), Some(&dir)).is_err(), "no way out of the folder");
        assert!(load("a.gltf", &gltf_json("/etc/hosts"), Some(&dir)).is_err());
        assert!(load("a.gltf", &gltf_json("m.bin"), None).is_err(), "nowhere to look, inside an archive");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn broken_gltf_does_not_panic() {
        for json in ["{}", "[]", "not json", r#"{"meshes":[{"primitives":[{"attributes":{"POSITION":9}}]}]}"#, r#"{"accessors":[{"bufferView":99,"componentType":5126,"count":99999999,"type":"VEC3"}],"meshes":[{"primitives":[{"attributes":{"POSITION":0}}]}]}"#] {
            assert!(load("a.gltf", json.as_bytes(), None).is_err(), "{json}");
        }
    }

    #[test]
    fn models_are_cut_at_the_triangle_limit() {
        let n = (MAX_TRIANGLES + 50) as u32;
        let mut b = vec![0u8; 84 + 50 * n as usize];
        b[80..84].copy_from_slice(&n.to_le_bytes());
        let tris = parse_stl(&b).unwrap();
        assert_eq!(tris.len(), MAX_TRIANGLES + 1, "the rest is never turned into triangles");
        drop(b);
        let m = load("a.stl", &{
            let mut small = vec![0u8; 84];
            small[80] = 0;
            small
        }, None);
        assert!(m.is_err(), "no triangles");
    }

    #[test]
    fn looping_scene_graphs_end() {
        // Each node has itself twice as a child: 2^64 paths without a limit.
        let json = serde_json::json!({
            "scene": 0, "scenes": [{"nodes": [0]}],
            "nodes": [{"children": [0, 0]}],
            "meshes": [], "buffers": []
        });
        assert!(load("a.gltf", json.to_string().as_bytes(), None).is_err());
    }

    #[test]
    fn sizes_read_plainly() {
        assert_eq!(size_text(1.0), "1");
        assert_eq!(size_text(2.5), "2.5");
        assert_eq!(size_text(0.03125), "0.0312");
        assert_eq!(size_text(120.0), "120");
    }

    #[test]
    fn nearer_triangles_hide_farther_ones() {
        // Two parallel squares facing the viewer: red in front of blue, in either file order.
        let square = |z: f32, color: [u8; 3]| {
            let (a, b, c, d) = ([-1.0, -1.0, z], [1.0, -1.0, z], [1.0, 1.0, z], [-1.0, 1.0, z]);
            [Tri { v: [a, b, c], color }, Tri { v: [a, c, d], color }]
        };
        for reversed in [false, true] {
            let mut tris: Vec<Tri> = square(0.5, [255, 0, 0]).into_iter().chain(square(-0.5, [0, 0, 255])).collect();
            if reversed {
                tris.reverse();
            }
            let mesh = Mesh { format: "test", tris, min: [-1.0, -1.0, -0.5], max: [1.0, 1.0, 0.5], truncated: false };
            let (w, h) = (64, 64);
            let rgba = render(&mesh, &Camera { yaw: 0.0, pitch: 0.0, zoom: 1.0 }, w, h, 1);
            let center = &rgba[(h / 2 * w + w / 2) * 4..][..4];
            assert!(center[0] > 100 && center[2] == 0 && center[3] == 255, "{center:?}");
            assert_eq!(rgba[3], 0, "nothing in the corner");
        }
    }
}
