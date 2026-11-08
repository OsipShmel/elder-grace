//! Elder Grace native renderer (Metal, arm64). Knows nothing about games: input is meshes in world
//! space, output is a window with an orbit camera or a PNG frame.

pub mod dds;
pub mod gpu;
pub mod math;
pub mod compare;
pub mod viewer;

pub use dds::TextureData;
pub use gpu::{Gpu, GpuScene};

/// Mesh in world space: a triangle list.
#[derive(Debug, Clone, Default)]
pub struct MeshData {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    /// Per-vertex UVs (may be empty: then no texture).
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
    /// Color without a texture (linear); `None` picks from the palette by mesh index.
    pub color: Option<[f32; 4]>,
    /// Index into `Scene::textures`: albedo.
    pub texture: Option<usize>,
}

/// Mesh instance: a model-to-world matrix (column-major).
#[derive(Debug, Clone, Copy)]
pub struct Instance {
    pub mesh: usize,
    pub transform: math::Mat4,
}

#[derive(Debug, Clone, Default)]
pub struct Scene {
    pub meshes: Vec<MeshData>,
    pub textures: Vec<TextureData>,
    /// Empty: every mesh is drawn once as is.
    pub instances: Vec<Instance>,
}

impl Scene {
    /// Bounds of all vertices in world space (instances included): (min, max).
    pub fn bounds(&self) -> ([f32; 3], [f32; 3]) {
        let mut lo = [f32::MAX; 3];
        let mut hi = [f32::MIN; 3];
        let mut add = |p: [f32; 3]| {
            for a in 0..3 {
                lo[a] = lo[a].min(p[a]);
                hi[a] = hi[a].max(p[a]);
            }
        };
        if self.instances.is_empty() {
            self.meshes.iter().flat_map(|m| &m.positions).for_each(|&p| add(p));
        } else {
            // Bbox corners of each mesh transformed by the instance: fast and conservative.
            let local: Vec<Option<([f32; 3], [f32; 3])>> = self.meshes.iter().map(|m| mesh_bounds(&m.positions)).collect();
            for inst in &self.instances {
                let Some(Some((a, b))) = local.get(inst.mesh) else { continue };
                for k in 0..8 {
                    let c = [if k & 1 == 0 { a[0] } else { b[0] }, if k & 2 == 0 { a[1] } else { b[1] }, if k & 4 == 0 { a[2] } else { b[2] }];
                    let w = math::transform(&inst.transform, c);
                    add([w[0], w[1], w[2]]);
                }
            }
        }
        if lo[0] > hi[0] { ([0.0; 3], [0.0; 3]) } else { (lo, hi) }
    }

    pub fn triangles(&self) -> usize {
        if self.instances.is_empty() {
            self.meshes.iter().map(|m| m.indices.len() / 3).sum()
        } else {
            self.instances.iter().filter_map(|i| self.meshes.get(i.mesh)).map(|m| m.indices.len() / 3).sum()
        }
    }
}

fn mesh_bounds(p: &[[f32; 3]]) -> Option<([f32; 3], [f32; 3])> {
    let first = *p.first()?;
    Some(p.iter().fold((first, first), |(lo, hi), v| (std::array::from_fn(|a| lo[a].min(v[a])), std::array::from_fn(|a| hi[a].max(v[a])))))
}

/// Orbit camera around a target.
#[derive(Debug, Clone, Copy)]
pub struct Camera {
    pub target: [f32; 3],
    pub distance: f32,
    /// Rotation around the vertical axis and pitch, radians.
    pub yaw: f32,
    pub pitch: f32,
    pub fovy: f32,
    pub near: f32,
    pub far: f32,
}

impl Camera {
    /// Camera whose frame fully contains the scene bounds.
    pub fn framing(lo: [f32; 3], hi: [f32; 3]) -> Self {
        let target = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0, (lo[2] + hi[2]) / 2.0];
        let d = math::sub(hi, lo);
        let radius = (math::dot(d, d).sqrt() / 2.0).max(0.01);
        Self { target, distance: radius * 2.4, yaw: 0.7, pitch: 0.35, fovy: 0.9, near: radius * 0.01, far: radius * 20.0 }
    }

    pub fn eye(&self) -> [f32; 3] {
        let (cp, sp) = (self.pitch.cos(), self.pitch.sin());
        [self.target[0] + self.distance * cp * self.yaw.sin(), self.target[1] + self.distance * sp, self.target[2] + self.distance * cp * self.yaw.cos()]
    }

    pub fn view_proj(&self, aspect: f32) -> math::Mat4 {
        let far = self.far.max(self.distance * 4.0);
        // Near plane depends on the distance to the target: works both for a scene overview and right next to a wall.
        let near = (self.distance * 0.01).clamp(0.02, self.near.max(0.02));
        math::mul(&math::perspective(self.fovy, aspect, near, far), &math::look_at(self.eye(), self.target, [0.0, 1.0, 0.0]))
    }

    /// Camera from an eye position and a view direction (viewer world, right-handed).
    pub fn looking(eye: [f32; 3], forward: [f32; 3], fovy: f32) -> Self {
        let f = math::normalize(forward);
        let distance = 1.0;
        let target = [eye[0] + f[0] * distance, eye[1] + f[1] * distance, eye[2] + f[2] * distance];
        // eye − target = −f·d = d·(cos p·sin y, sin p, cos p·cos y).
        let yaw = (-f[0]).atan2(-f[2]);
        let pitch = (-f[1]).clamp(-1.0, 1.0).asin();
        Self { target, distance, yaw, pitch, fovy, near: 0.05, far: 5000.0 }
    }

    /// Moves the target along the camera axes: forward (horizontal), right, up.
    pub fn fly(&mut self, forward: f32, right: f32, up: f32) {
        let (sy, cy) = self.yaw.sin_cos();
        // The view goes from the eye to the target, i.e. against the (sin yaw, cos yaw) direction.
        let f = [-sy, 0.0, -cy];
        let r = [cy, 0.0, -sy];
        for a in 0..3 {
            self.target[a] += f[a] * forward + r[a] * right;
        }
        self.target[1] += up;
    }
}
