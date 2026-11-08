//! Нативный рендер Elder Grace (Metal, arm64). Не знает об играх: на вход — меши в мировых
//! координатах, на выход — окно с орбитальной камерой или PNG-кадр.

pub mod gpu;
pub mod math;
pub mod viewer;

pub use gpu::{Gpu, GpuScene};

/// Меш в мировых координатах: треугольники списком.
#[derive(Debug, Clone, Default)]
pub struct MeshData {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub indices: Vec<u32>,
    /// Цвет (линейный); `None` — из палитры по номеру меша.
    pub color: Option<[f32; 4]>,
}

#[derive(Debug, Clone, Default)]
pub struct Scene {
    pub meshes: Vec<MeshData>,
}

impl Scene {
    /// Границы всех вершин: (min, max).
    pub fn bounds(&self) -> ([f32; 3], [f32; 3]) {
        let mut lo = [f32::MAX; 3];
        let mut hi = [f32::MIN; 3];
        for p in self.meshes.iter().flat_map(|m| &m.positions) {
            for a in 0..3 {
                lo[a] = lo[a].min(p[a]);
                hi[a] = hi[a].max(p[a]);
            }
        }
        if lo[0] > hi[0] { ([0.0; 3], [0.0; 3]) } else { (lo, hi) }
    }

    pub fn triangles(&self) -> usize {
        self.meshes.iter().map(|m| m.indices.len() / 3).sum()
    }
}

/// Орбитальная камера вокруг цели.
#[derive(Debug, Clone, Copy)]
pub struct Camera {
    pub target: [f32; 3],
    pub distance: f32,
    /// Поворот вокруг вертикали и наклон, радианы.
    pub yaw: f32,
    pub pitch: f32,
    pub fovy: f32,
    pub near: f32,
    pub far: f32,
}

impl Camera {
    /// Камера, в кадр которой целиком попадают границы сцены.
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
        math::mul(&math::perspective(self.fovy, aspect, self.near, far), &math::look_at(self.eye(), self.target, [0.0, 1.0, 0.0]))
    }
}
