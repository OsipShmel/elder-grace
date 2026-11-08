//! Минимум линейной алгебры для камеры: матрицы 4×4 по столбцам (как в MSL).

pub type Vec3 = [f32; 3];
pub type Mat4 = [f32; 16];

pub fn sub(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

pub fn dot(a: Vec3, b: Vec3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

pub fn cross(a: Vec3, b: Vec3) -> Vec3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

pub fn normalize(a: Vec3) -> Vec3 {
    let l = dot(a, a).sqrt().max(1e-12);
    [a[0] / l, a[1] / l, a[2] / l]
}

/// Перспектива (правая система, глубина 0..1 как в Metal).
pub fn perspective(fovy: f32, aspect: f32, near: f32, far: f32) -> Mat4 {
    let f = 1.0 / (fovy / 2.0).tan();
    let r = far / (near - far);
    [f / aspect, 0.0, 0.0, 0.0, 0.0, f, 0.0, 0.0, 0.0, 0.0, r, -1.0, 0.0, 0.0, near * r, 0.0]
}

pub fn look_at(eye: Vec3, target: Vec3, up: Vec3) -> Mat4 {
    let z = normalize(sub(eye, target));
    let x = normalize(cross(up, z));
    let y = cross(z, x);
    [x[0], y[0], z[0], 0.0, x[1], y[1], z[1], 0.0, x[2], y[2], z[2], 0.0, -dot(x, eye), -dot(y, eye), -dot(z, eye), 1.0]
}

pub fn mul(a: &Mat4, b: &Mat4) -> Mat4 {
    let mut m = [0.0; 16];
    for c in 0..4 {
        for r in 0..4 {
            m[c * 4 + r] = (0..4).map(|k| a[k * 4 + r] * b[c * 4 + k]).sum();
        }
    }
    m
}

pub fn transform(m: &Mat4, p: Vec3) -> [f32; 4] {
    let v = [p[0], p[1], p[2], 1.0];
    std::array::from_fn(|r| (0..4).map(|k| m[k * 4 + r] * v[k]).sum())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_projects_to_center() {
        let vp = mul(&perspective(1.0, 1.5, 0.1, 100.0), &look_at([3.0, 2.0, 5.0], [0.0, 1.0, 0.0], [0.0, 1.0, 0.0]));
        let c = transform(&vp, [0.0, 1.0, 0.0]);
        let (x, y, z) = (c[0] / c[3], c[1] / c[3], c[2] / c[3]);
        assert!(x.abs() < 1e-5 && y.abs() < 1e-5, "цель не в центре: {x} {y}");
        assert!((0.0..1.0).contains(&z), "глубина вне 0..1: {z}");
    }
}
