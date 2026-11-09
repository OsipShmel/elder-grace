//! Control protocol: one JSON object per line in both directions.
//! `grace-mcp` and the shim inside Wine will speak it too, so there are no
//! platform dependencies here.

use std::io::{self, BufRead, Write};

use serde::{Deserialize, Serialize};

pub mod mirror;
pub use mirror::MirrorRegion;

pub const PROTOCOL_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Request {
    Ping,
    Stats,
    /// Shader overrides in the d3d12 shim (TCP 47322 under the oracle).
    Overrides { op: OverrideOp },
    /// Which shaders are drawing now: records PSO bindings for `ms` milliseconds.
    Bound { ms: u32 },
    /// Camera from the game process memory (d3d12 shim). The cbuffer layout is
    /// knowledge about the game, so it comes with the request (`grace-<game>/data/camera-layout.json`).
    Camera { layout: CameraLayout },
    /// Debug: read `len` bytes of game memory at an address (via ReadProcessMemory, safe).
    Peek { addr: u64, len: u32 },
    /// Debug: addresses of consecutive f32 values within the [min, max] ranges (4-byte step).
    FindFloats { ranges: Vec<(f32, f32)>, limit: u32 },
    /// Live mirror (dxgi shim): copy these regions into the mirror file at every `Present`.
    /// An empty list stops mirroring. See `mirror`.
    Mirror { regions: Vec<MirrorRegion> },
    /// Modules loaded in the game process (d3d12 shim): for static addresses (image base + RVA).
    Modules,
    /// Debug: addresses of 8-aligned u64 values within [lo, hi] in writable memory (a pointer scan).
    FindPointers { lo: u64, hi: u64, limit: u32 },
}

/// A module of the game process.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModuleInfo {
    pub name: String,
    pub base: u64,
    pub size: u64,
}

/// Where the fields are in the camera cbuffer (byte offsets from the start of the cbuffer).
/// Matrices are HLSL `column_major`: element [row r][column c] is at offset c*16 + r*4.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CameraLayout {
    /// float3 camera position in the world.
    pub position: u32,
    /// float4x4 view-to-world; column 3 must match `position`, which is the check.
    pub inv_view: u32,
    /// float4x4 projection.
    pub proj: u32,
    /// float4 screen size (optional).
    #[serde(default)]
    pub screen: Option<u32>,
    /// CBV alignment on the GPU (256); in process memory the shim searches with a 16-byte step.
    #[serde(default = "default_align")]
    pub align: u32,
}

fn default_align() -> u32 {
    256
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OverrideOp {
    Status,
    On,
    Off,
    Toggle,
    /// Re-read the override directory and recreate overridden PSOs.
    Reload,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Response {
    Pong {
        protocol: u32,
        app: String,
        version: String,
    },
    Stats(Stats),
    Overrides(OverrideStatus),
    Bound(BoundReport),
    Camera(CameraReport),
    Peek { addr: u64, bytes: Vec<u8> },
    Found { regions: usize, addrs: Vec<u64> },
    Mirror { generation: u32 },
    Modules { modules: Vec<ModuleInfo> },
    Error {
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Stats {
    pub frames: u64,
    /// Average CPU-side frame time over the last second, ms.
    pub frame_ms: f64,
    pub fps: f64,
    pub drawable_width: u32,
    pub drawable_height: u32,
    pub device: String,
}

/// Shader override state in the d3d12 shim.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OverrideStatus {
    /// Overridden PSOs are substituted into command lists.
    pub enabled: bool,
    /// Override files loaded from `GRACE_SHADER_OVERRIDE`.
    pub shaders_loaded: usize,
    /// "original -> override" PSO pairs.
    pub pairs: usize,
    /// Override PSOs that D3DMetal failed to create (the game got the original).
    pub failed: usize,
    /// How many times an override PSO has been substituted since start.
    pub swaps: u64,
    /// Applied overrides: `<hash>.<stage>`.
    pub applied: Vec<String>,
    /// Recorded PSO recipes (for reloading from disk).
    #[serde(default)]
    pub recipes: usize,
    /// Result of the last reload; empty if there was none.
    #[serde(default)]
    pub last_reload: String,
}

/// Cameras found: usually one per frame in a ring buffer (values are close).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CameraReport {
    /// How many memory regions were scanned and how many bytes they hold.
    pub buffers: usize,
    pub bytes: u64,
    pub candidates: Vec<CameraSample>,
    /// Debug: perspective projections anywhere in memory (address, fovy°, P00/P11).
    #[serde(default)]
    pub projections: Vec<(u64, f32, f32)>,
    /// Matrices with the same rotation as the first camera but a different translation (address, translation):
    /// candidates for the "world" camera when the cbuffer camera is local.
    #[serde(default)]
    pub same_rotation: Vec<(u64, [f32; 3])>,
}

impl CameraReport {
    /// World position: among the matrices with the same rotation, the one farthest from the local position
    /// (more than 10 m away; closer ones are player-attached camera copies in the same local space).
    pub fn world(&self) -> Option<[f32; 3]> {
        let local = self.candidates.first()?.position;
        let dist = |t: &[f32; 3]| (0..3).map(|a| (t[a] - local[a]).powi(2)).sum::<f32>().sqrt();
        self.same_rotation.iter().map(|(_, t)| *t).filter(|t| dist(t) > 10.0).max_by(|a, b| dist(a).total_cmp(&dist(b)))
    }
}

/// Translation of the 4x4 matrix at the address if its rotation matches `rot` (3 per column, column-major)
/// within 1e-4 and the bottom row is (0, 0, 0, 1). Both storage orders.
pub fn rotation_match(rot: &[f32; 9], f: &dyn Fn(usize) -> f32) -> Option<[f32; 3]> {
    for transposed in [false, true] {
        let m = |r: usize, c: usize| if transposed { f(r * 16 + c * 4) } else { f(c * 16 + r * 4) };
        if (m(3, 3) - 1.0).abs() > 1e-4 || (0..3).any(|c| m(3, c).abs() > 1e-4) {
            continue;
        }
        if (0..3).all(|c| (0..3).all(|r| (m(r, c) - rot[c * 3 + r]).abs() < 1e-4)) {
            let t = [m(0, 3), m(1, 3), m(2, 3)];
            if t.iter().all(|v| v.is_finite()) {
                return Some(t);
            }
        }
    }
    None
}

/// Perspective projection at the address (column_major or row-major): (fovy in degrees, P00/P11).
pub fn projection_at(f: &dyn Fn(usize) -> f32) -> Option<(f32, f32)> {
    for transposed in [false, true] {
        let m = |r: usize, c: usize| if transposed { f(r * 16 + c * 4) } else { f(c * 16 + r * 4) };
        let (p00, p11, p32, p33) = (m(0, 0), m(1, 1), m(3, 2), m(3, 3));
        let off_zero = [m(0, 1), m(1, 0), m(3, 0), m(3, 1)].iter().all(|v| v.abs() < 1e-6);
        if p00 > 0.0 && p11 > 0.0 && (p32.abs() - 1.0).abs() < 1e-5 && p33.abs() < 1e-6 && off_zero && p00.is_finite() && p11.is_finite() {
            let fovy = 2.0 * (1.0 / p11).atan();
            if (0.17..2.62).contains(&fovy) {
                return Some((fovy.to_degrees(), p00 / p11));
            }
        }
    }
    None
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CameraSample {
    pub position: [f32; 3],
    /// As in memory (column_major: by columns).
    pub inv_view: [f32; 16],
    pub proj: [f32; 16],
    #[serde(default)]
    pub screen: Option<[f32; 4]>,
    /// Camera world position if the cbuffer camera is local (a matrix with the same rotation
    /// and a "world" translation, see `CameraReport::world`).
    #[serde(default)]
    pub world: Option<[f32; 3]>,
}

/// Shaders that took part in frames during the recording window.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BoundReport {
    pub ms: u32,
    /// Distinct PSOs bound during the window.
    pub pipelines: usize,
    /// PSOs without a recipe (shaders unknown).
    pub unknown: usize,
    /// By bind count, descending.
    pub shaders: Vec<BoundShader>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BoundShader {
    /// `<hash>.<stage>`, as in the dump and in the override directory.
    pub name: String,
    /// PSO binds with this shader during the window.
    pub binds: u64,
    /// Distinct PSOs with this shader.
    pub pipelines: u32,
    /// DXIL (SM6); otherwise DXBC (SM4/5).
    pub dxil: bool,
}

/// Checks a candidate against the layout; `f(off)` reads an f32 at an offset from the start of the cbuffer.
pub fn camera_at(layout: &CameraLayout, f: &dyn Fn(usize) -> f32) -> Option<CameraSample> {
    // The shader declares column_major matrices, but on the CPU side the game may store them
    // row-major: try both orders, the output is always column-major.
    camera_oriented(layout, f, false).or_else(|| camera_oriented(layout, f, true))
}

fn camera_oriented(layout: &CameraLayout, f: &dyn Fn(usize) -> f32, transposed: bool) -> Option<CameraSample> {
    let (pos, inv, proj) = (layout.position as usize, layout.inv_view as usize, layout.proj as usize);
    let position = [f(pos), f(pos + 4), f(pos + 8)];
    let len = position.iter().map(|v| v * v).sum::<f32>().sqrt();
    if !position.iter().all(|v| v.is_finite()) || len < 1e-3 || len > 1e6 {
        return None;
    }
    // [r][c]: column-major offset c*16 + r*4, row-major r*16 + c*4.
    let m = |base: usize, r: usize, c: usize| if transposed { f(base + r * 16 + c * 4) } else { f(base + c * 16 + r * 4) };
    let close = |a: f32, b: f32, eps: f32| (a - b).abs() <= eps;
    let eps = 1e-3 * (1.0 + len);
    // View-to-world translation = camera position, or 0 (camera-relative rendering).
    let absolute = (0..3).all(|r| close(m(inv, r, 3), position[r], eps));
    let relative = (0..3).all(|r| close(m(inv, r, 3), 0.0, 1e-4));
    if !(absolute || relative) || !close(m(inv, 3, 3), 1.0, 1e-4) || !(0..3).all(|c| close(m(inv, 3, c), 0.0, 1e-4)) {
        return None;
    }
    // Screen size (if present): width and height are plausible whole pixels.
    if let Some(sc) = layout.screen {
        let (w, h) = (f(sc as usize), f(sc as usize + 4));
        let pixels = |v: f32| (64.0..=16384.0).contains(&v) && (v - v.round()).abs() < 1e-3;
        if !pixels(w) || !pixels(h) {
            return None;
        }
    }
    // Perspective: P[3][2] = ±1, P[3][3] = 0, P[0][0] and P[1][1] > 0.
    if !close(m(proj, 3, 2).abs(), 1.0, 1e-4) || !close(m(proj, 3, 3), 0.0, 1e-4) || m(proj, 0, 0) <= 0.0 || m(proj, 1, 1) <= 0.0 {
        return None;
    }
    let canon = |base: usize| -> [f32; 16] { std::array::from_fn(|k| m(base, k % 4, k / 4)) };
    let (inv_view, proj_m) = (canon(inv), canon(proj));
    // Not a camera if the matrices contain NaN/infinity (JSON could not carry those either).
    if !inv_view.iter().chain(&proj_m).all(|v| v.is_finite()) {
        return None;
    }
    // Camera rotation is orthonormal: columns 0-2 are unit length and mutually perpendicular.
    let col = |c: usize| [inv_view[c * 4], inv_view[c * 4 + 1], inv_view[c * 4 + 2]];
    let dot = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let (c0, c1, c2) = (col(0), col(1), col(2));
    if [c0, c1, c2].iter().any(|c| (dot(*c, *c) - 1.0).abs() > 1e-2) || [dot(c0, c1), dot(c0, c2), dot(c1, c2)].iter().any(|d| d.abs() > 1e-2) {
        return None;
    }
    // Sensible field of view: 10°..150° (P[1][1] = cot(fovy/2)).
    let fovy = 2.0 * (1.0 / proj_m[5]).atan();
    if !(0.17..2.62).contains(&fovy) {
        return None;
    }
    let screen = layout.screen.map(|s| std::array::from_fn(|k| f(s as usize + k * 4))).filter(|v: &[f32; 4]| v.iter().all(|x| x.is_finite()));
    Some(CameraSample { position, inv_view, proj: proj_m, screen, world: None })
}

pub fn write_message<W: Write, T: Serialize>(w: &mut W, msg: &T) -> io::Result<()> {
    serde_json::to_writer(&mut *w, msg)?;
    w.write_all(b"\n")?;
    w.flush()
}

/// `Ok(None)`: the peer closed the connection.
pub fn read_message<R: BufRead, T: for<'de> Deserialize<'de>>(r: &mut R) -> io::Result<Option<T>> {
    let mut line = String::new();
    if r.read_line(&mut line)? == 0 {
        return Ok(None);
    }
    serde_json::from_str(&line).map(Some).map_err(io::Error::other)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_over_lines() {
        let mut buf = Vec::new();
        write_message(&mut buf, &Request::Ping).unwrap();
        write_message(&mut buf, &Request::Stats).unwrap();
        assert_eq!(String::from_utf8_lossy(&buf), "{\"cmd\":\"ping\"}\n{\"cmd\":\"stats\"}\n");

        let mut r = io::BufReader::new(buf.as_slice());
        assert_eq!(read_message::<_, Request>(&mut r).unwrap(), Some(Request::Ping));
        assert_eq!(read_message::<_, Request>(&mut r).unwrap(), Some(Request::Stats));
        assert_eq!(read_message::<_, Request>(&mut r).unwrap(), None);
    }

    #[test]
    fn camera_layout_defaults() {
        let l: CameraLayout = serde_json::from_str(r#"{"position":48,"inv_view":320,"proj":256}"#).unwrap();
        assert_eq!((l.align, l.screen), (256, None));
        let mut buf = Vec::new();
        write_message(&mut buf, &Request::Camera { layout: l }).unwrap();
        let mut r = io::BufReader::new(buf.as_slice());
        assert_eq!(read_message::<_, Request>(&mut r).unwrap(), Some(Request::Camera { layout: l }));
    }

    #[test]
    fn rotation_match_finds_world_copy() {
        let rot = [0.157, 0.0, 0.988, 0.0, 1.0, 0.0, -0.988, 0.0, 0.157];
        let mut m = [0f32; 16];
        for c in 0..3 {
            for r in 0..3 {
                m[c * 4 + r] = rot[c * 3 + r];
            }
        }
        m[12..16].copy_from_slice(&[-113.9, 15.5, 13.0, 1.0]);
        assert_eq!(rotation_match(&rot, &|o: usize| m[o / 4]), Some([-113.9, 15.5, 13.0]));
        m[0] = 0.5;
        assert_eq!(rotation_match(&rot, &|o: usize| m[o / 4]), None);
    }

    #[test]
    fn overrides_wire_format() {
        let mut buf = Vec::new();
        write_message(&mut buf, &Request::Overrides { op: OverrideOp::Toggle }).unwrap();
        assert_eq!(String::from_utf8_lossy(&buf), "{\"cmd\":\"overrides\",\"op\":\"toggle\"}\n");
        let status = Response::Overrides(OverrideStatus {
            enabled: true,
            shaders_loaded: 1,
            pairs: 2,
            failed: 0,
            swaps: 42,
            applied: vec!["cfb409a01172e9fb.ps".into()],
            recipes: 8875,
            last_reload: String::new(),
        });
        let mut buf = Vec::new();
        write_message(&mut buf, &status).unwrap();
        let mut r = io::BufReader::new(buf.as_slice());
        assert_eq!(read_message::<_, Response>(&mut r).unwrap(), Some(status));
    }

    #[test]
    fn camera_signature() {
        let layout = CameraLayout { position: 48, inv_view: 320, proj: 256, screen: Some(16), align: 256 };
        let mut cb = vec![0f32; 512 / 4];
        cb[4..6].copy_from_slice(&[1728.0, 1117.0]);
        cb[12..15].copy_from_slice(&[10.0, 2.0, -5.0]);
        // inv_view: identity rotation, translation in column 3 (offset 320 + 48).
        for (r, c) in [(0, 0), (1, 1), (2, 2), (3, 3)] {
            cb[(320 + c * 16 + r * 4) / 4] = 1.0;
        }
        cb[(320 + 48) / 4..(320 + 60) / 4].copy_from_slice(&[10.0, 2.0, -5.0]);
        cb[(256) / 4] = 1.2;
        cb[(256 + 20) / 4] = 2.0;
        cb[(256 + 44) / 4] = 1.0;
        let f = |o: usize| cb[o / 4];
        let s = camera_at(&layout, &f).unwrap();
        assert_eq!(s.position, [10.0, 2.0, -5.0]);
        // Same camera with row-major matrices: also found, output is column-major.
        let mut t = cb.clone();
        for base in [320usize, 256] {
            for r in 0..4 {
                for c in 0..4 {
                    t[(base + r * 16 + c * 4) / 4] = cb[(base + c * 16 + r * 4) / 4];
                }
            }
        }
        assert_eq!(camera_at(&layout, &|o: usize| t[o / 4]).unwrap(), s);
        // Camera-relative rendering: translation 0, position separate: still a camera.
        let mut rel = cb.clone();
        rel[(320 + 48) / 4..(320 + 60) / 4].copy_from_slice(&[0.0; 3]);
        assert!(camera_at(&layout, &|o: usize| rel[o / 4]).is_some());
        let mut tiny = cb.clone();
        tiny[4] = 1.0; // a 1x1117 "screen" is not a camera
        assert!(camera_at(&layout, &|o: usize| tiny[o / 4]).is_none());
        cb[(320 + 52) / 4] = 3.0; // translation does not match the position: not a camera
        assert!(camera_at(&layout, &|o: usize| cb[o / 4]).is_none());
    }
}
