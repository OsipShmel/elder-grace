//! Нативный рендер Elder Grace (Metal, arm64). Не знает об играх: на вход — меши в мировых
//! координатах, на выход — окно с орбитальной камерой или PNG-кадр.

pub mod dds;
pub mod gpu;
pub mod math;
pub mod compare;
pub mod viewer;

pub use dds::TextureData;
pub use gpu::{Gpu, GpuScene};

/// Меш в мировых координатах: треугольники списком.
#[derive(Debug, Clone, Default)]
pub struct MeshData {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    /// UV на вершину (может быть пустым — тогда без текстуры).
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
    /// Цвет без текстуры (линейный); `None` — из палитры по номеру меша.
    pub color: Option<[f32; 4]>,
    /// Индекс в `Scene::textures` — альбедо.
    pub texture: Option<usize>,
}

/// Экземпляр меша: матрица «модель → мир» (по столбцам).
#[derive(Debug, Clone, Copy)]
pub struct Instance {
    pub mesh: usize,
    pub transform: math::Mat4,
}

#[derive(Debug, Clone, Default)]
pub struct Scene {
    pub meshes: Vec<MeshData>,
    pub textures: Vec<TextureData>,
    /// Пусто — каждый меш рисуется один раз как есть.
    pub instances: Vec<Instance>,
}

impl Scene {
    /// Границы всех вершин в мире (с учётом экземпляров): (min, max).
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
            // Углы bbox каждого меша, перенесённые экземпляром, — быстро и с запасом.
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
        // Ближняя плоскость — от расстояния до цели: и обзор сцены сверху, и шаг вплотную к стене.
        let near = (self.distance * 0.01).clamp(0.02, self.near.max(0.02));
        math::mul(&math::perspective(self.fovy, aspect, near, far), &math::look_at(self.eye(), self.target, [0.0, 1.0, 0.0]))
    }

    /// Камера из положения глаза и направления взгляда (мир просмотра, правая СК).
    pub fn looking(eye: [f32; 3], forward: [f32; 3], fovy: f32) -> Self {
        let f = math::normalize(forward);
        let distance = 1.0;
        let target = [eye[0] + f[0] * distance, eye[1] + f[1] * distance, eye[2] + f[2] * distance];
        // eye − target = −f·d = d·(cos p·sin y, sin p, cos p·cos y).
        let yaw = (-f[0]).atan2(-f[2]);
        let pitch = (-f[1]).clamp(-1.0, 1.0).asin();
        Self { target, distance, yaw, pitch, fovy, near: 0.05, far: 5000.0 }
    }

    /// Сдвиг цели в осях камеры: вперёд (по горизонтали), вправо, вверх.
    pub fn fly(&mut self, forward: f32, right: f32, up: f32) {
        let (sy, cy) = self.yaw.sin_cos();
        // Взгляд — от глаза к цели, т.е. против направления (sin yaw, cos yaw).
        let f = [-sy, 0.0, -cy];
        let r = [cy, 0.0, -sy];
        for a in 0..3 {
            self.target[a] += f[a] * forward + r[a] * right;
        }
        self.target[1] += up;
    }
}
