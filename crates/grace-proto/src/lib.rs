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
}
