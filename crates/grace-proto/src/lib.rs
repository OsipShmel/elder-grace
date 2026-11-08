//! Протокол управления: один JSON-объект на строку в обе стороны.
//! Его же будут говорить `grace-mcp` и shim внутри Wine, поэтому никаких
//! зависимостей от платформы здесь нет.

use std::io::{self, BufRead, Write};

use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Request {
    Ping,
    Stats,
    /// Подмены шейдеров в shim d3d12 (TCP 47322 под эталоном).
    Overrides { op: OverrideOp },
    /// Какие шейдеры рисуют сейчас: запись привязок PSO в течение `ms` миллисекунд.
    Bound { ms: u32 },
    /// Камера из памяти процесса игры (shim d3d12). Раскладка cbuffer'а —
    /// знание об игре, поэтому приходит в запросе (`grace-<игра>/data/camera-layout.json`).
    Camera { layout: CameraLayout },
    /// Отладка: прочитать `len` байт памяти игры по адресу (через ReadProcessMemory — безопасно).
    Peek { addr: u64, len: u32 },
}

/// Где в cbuffer'е камеры лежат поля (байтовые смещения от начала cbuffer'а).
/// Матрицы — `column_major` HLSL: элемент [строка r][столбец c] по смещению c·16 + r·4.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CameraLayout {
    /// float3 положения камеры в мире.
    pub position: u32,
    /// float4x4 «вид → мир»; столбец 3 должен совпасть с `position` — это и есть проверка.
    pub inv_view: u32,
    /// float4x4 проекции.
    pub proj: u32,
    /// float4 размера экрана (необязательно).
    #[serde(default)]
    pub screen: Option<u32>,
    /// Выравнивание CBV на GPU (256); в памяти процесса shim ищет с шагом 16.
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
    /// Перечитать каталог подмен и пересоздать подменённые PSO.
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
    Error {
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Stats {
    pub frames: u64,
    /// Среднее время кадра CPU-стороны за последнюю секунду, мс.
    pub frame_ms: f64,
    pub fps: f64,
    pub drawable_width: u32,
    pub drawable_height: u32,
    pub device: String,
}

/// Состояние подмен шейдеров в shim d3d12.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OverrideStatus {
    /// Подменённые PSO подставляются в командные списки.
    pub enabled: bool,
    /// Файлов подмен загружено из `GRACE_SHADER_OVERRIDE`.
    pub shaders_loaded: usize,
    /// Пар PSO «оригинал → подменённый».
    pub pairs: usize,
    /// Подменённых PSO, которые D3DMetal не смог создать (игра получила оригинал).
    pub failed: usize,
    /// Сколько раз подставлен подменённый PSO с запуска.
    pub swaps: u64,
    /// Применённые подмены: `<хеш>.<стадия>`.
    pub applied: Vec<String>,
    /// Записанных рецептов PSO (для перезагрузки с диска).
    #[serde(default)]
    pub recipes: usize,
    /// Итог последней перезагрузки, пусто — не было.
    #[serde(default)]
    pub last_reload: String,
}

/// Найденные камеры: обычно по одной на кадр в кольцевом буфере (значения близки).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CameraReport {
    /// Сколько участков памяти просмотрено и сколько в них байт.
    pub buffers: usize,
    pub bytes: u64,
    pub candidates: Vec<CameraSample>,
    /// Отладка: перспективные проекции где угодно в памяти (адрес, fovy°, P00/P11).
    #[serde(default)]
    pub projections: Vec<(u64, f32, f32)>,
}

/// Перспективная проекция по адресу (column_major или построчно): (fovy в градусах, P00/P11).
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
    /// Как в памяти (column_major: по столбцам).
    pub inv_view: [f32; 16],
    pub proj: [f32; 16],
    #[serde(default)]
    pub screen: Option<[f32; 4]>,
}

/// Шейдеры, участвовавшие в кадрах за окно записи.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BoundReport {
    pub ms: u32,
    /// Разных PSO привязано за окно.
    pub pipelines: usize,
    /// PSO без рецепта (шейдеры неизвестны).
    pub unknown: usize,
    /// По убыванию числа привязок.
    pub shaders: Vec<BoundShader>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BoundShader {
    /// `<хеш>.<стадия>` — как в дампе и в каталоге подмен.
    pub name: String,
    /// Привязок PSO с этим шейдером за окно.
    pub binds: u64,
    /// Разных PSO с этим шейдером.
    pub pipelines: u32,
    /// DXIL (SM6) — иначе DXBC (SM4/5).
    pub dxil: bool,
}

/// Проверка кандидата по раскладке; `f(off)` читает f32 по смещению от начала cbuffer'а.
pub fn camera_at(layout: &CameraLayout, f: &dyn Fn(usize) -> f32) -> Option<CameraSample> {
    // Шейдер объявляет матрицы column_major, но на стороне CPU игра может хранить их
    // построчно — пробуем обе ориентации, на выходе всегда по столбцам.
    camera_oriented(layout, f, false).or_else(|| camera_oriented(layout, f, true))
}

fn camera_oriented(layout: &CameraLayout, f: &dyn Fn(usize) -> f32, transposed: bool) -> Option<CameraSample> {
    let (pos, inv, proj) = (layout.position as usize, layout.inv_view as usize, layout.proj as usize);
    let position = [f(pos), f(pos + 4), f(pos + 8)];
    let len = position.iter().map(|v| v * v).sum::<f32>().sqrt();
    if !position.iter().all(|v| v.is_finite()) || len < 1e-3 || len > 1e6 {
        return None;
    }
    // [r][c]: по столбцам — смещение c·16 + r·4, построчно — r·16 + c·4.
    let m = |base: usize, r: usize, c: usize| if transposed { f(base + r * 16 + c * 4) } else { f(base + c * 16 + r * 4) };
    let close = |a: f32, b: f32, eps: f32| (a - b).abs() <= eps;
    let eps = 1e-3 * (1.0 + len);
    // Перенос «вид → мир» = положение камеры, либо 0 (рендер относительно камеры).
    let absolute = (0..3).all(|r| close(m(inv, r, 3), position[r], eps));
    let relative = (0..3).all(|r| close(m(inv, r, 3), 0.0, 1e-4));
    if !(absolute || relative) || !close(m(inv, 3, 3), 1.0, 1e-4) || !(0..3).all(|c| close(m(inv, 3, c), 0.0, 1e-4)) {
        return None;
    }
    // Размер экрана (если задан): ширина и высота — правдоподобные целые пиксели.
    if let Some(sc) = layout.screen {
        let (w, h) = (f(sc as usize), f(sc as usize + 4));
        let pixels = |v: f32| (64.0..=16384.0).contains(&v) && (v - v.round()).abs() < 1e-3;
        if !pixels(w) || !pixels(h) {
            return None;
        }
    }
    // Перспектива: P[3][2] = ±1, P[3][3] = 0, P[0][0] и P[1][1] > 0.
    if !close(m(proj, 3, 2).abs(), 1.0, 1e-4) || !close(m(proj, 3, 3), 0.0, 1e-4) || m(proj, 0, 0) <= 0.0 || m(proj, 1, 1) <= 0.0 {
        return None;
    }
    let canon = |base: usize| -> [f32; 16] { std::array::from_fn(|k| m(base, k % 4, k / 4)) };
    let (inv_view, proj_m) = (canon(inv), canon(proj));
    // Не камера, если в матрицах NaN/бесконечность (и JSON такие числа не передаст).
    if !inv_view.iter().chain(&proj_m).all(|v| v.is_finite()) {
        return None;
    }
    // Поворот камеры ортонормирован: столбцы 0–2 единичные и взаимно перпендикулярны.
    let col = |c: usize| [inv_view[c * 4], inv_view[c * 4 + 1], inv_view[c * 4 + 2]];
    let dot = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let (c0, c1, c2) = (col(0), col(1), col(2));
    if [c0, c1, c2].iter().any(|c| (dot(*c, *c) - 1.0).abs() > 1e-2) || [dot(c0, c1), dot(c0, c2), dot(c1, c2)].iter().any(|d| d.abs() > 1e-2) {
        return None;
    }
    // Разумный угол обзора: 10°..150° (P[1][1] = ctg(fovy/2)).
    let fovy = 2.0 * (1.0 / proj_m[5]).atan();
    if !(0.17..2.62).contains(&fovy) {
        return None;
    }
    let screen = layout.screen.map(|s| std::array::from_fn(|k| f(s as usize + k * 4))).filter(|v: &[f32; 4]| v.iter().all(|x| x.is_finite()));
    Some(CameraSample { position, inv_view, proj: proj_m, screen })
}

pub fn write_message<W: Write, T: Serialize>(w: &mut W, msg: &T) -> io::Result<()> {
    serde_json::to_writer(&mut *w, msg)?;
    w.write_all(b"\n")?;
    w.flush()
}

/// `Ok(None)` — собеседник закрыл соединение.
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
        // inv_view: единичный поворот, перенос в столбце 3 (смещение 320 + 48).
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
        // Та же камера, но матрицы построчно: тоже находится, на выходе — по столбцам.
        let mut t = cb.clone();
        for base in [320usize, 256] {
            for r in 0..4 {
                for c in 0..4 {
                    t[(base + r * 16 + c * 4) / 4] = cb[(base + c * 16 + r * 4) / 4];
                }
            }
        }
        assert_eq!(camera_at(&layout, &|o: usize| t[o / 4]).unwrap(), s);
        // Рендер относительно камеры: перенос 0, положение отдельно — тоже камера.
        let mut rel = cb.clone();
        rel[(320 + 48) / 4..(320 + 60) / 4].copy_from_slice(&[0.0; 3]);
        assert!(camera_at(&layout, &|o: usize| rel[o / 4]).is_some());
        let mut tiny = cb.clone();
        tiny[4] = 1.0; // «экран» 1×1117 — не камера
        assert!(camera_at(&layout, &|o: usize| tiny[o / 4]).is_none());
        cb[(320 + 52) / 4] = 3.0; // перенос не совпал с положением — не камера
        assert!(camera_at(&layout, &|o: usize| cb[o / 4]).is_none());
    }
}
