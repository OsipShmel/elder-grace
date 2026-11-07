//! Шейдерный конвейер Elder Grace.
//!
//! Сейчас: разбор контейнера DXBC (в нём живут и DXBC SM4/5, и DXIL SM6) и сводка
//! по корпусу шейдеров, который снимает shim `d3d12.dll` в `.local/shaders/<игра>/`.
//! Дальше: трансляция в MSL и сборка `metallib`.

use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum ContainerError {
    #[error("слишком короткий файл ({0} байт)")]
    TooShort(usize),
    #[error("нет сигнатуры DXBC")]
    BadMagic,
    #[error("чанк {index} выходит за пределы файла")]
    ChunkOutOfBounds { index: usize },
}

/// Вид программы из токена версии (`D3D10_SB_TOKENIZED_PROGRAM_TYPE` / DXIL program kind).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Stage {
    Pixel,
    Vertex,
    Geometry,
    Hull,
    Domain,
    Compute,
    Other(u16),
}

impl Stage {
    fn from_kind(kind: u16) -> Self {
        match kind {
            0 => Self::Pixel,
            1 => Self::Vertex,
            2 => Self::Geometry,
            3 => Self::Hull,
            4 => Self::Domain,
            5 => Self::Compute,
            k => Self::Other(k),
        }
    }

    pub fn short(self) -> &'static str {
        match self {
            Self::Pixel => "ps",
            Self::Vertex => "vs",
            Self::Geometry => "gs",
            Self::Hull => "hs",
            Self::Domain => "ds",
            Self::Compute => "cs",
            Self::Other(_) => "??",
        }
    }
}

/// Формат байткода внутри контейнера.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Bytecode {
    /// DXBC SM4/5: чанк `SHDR` или `SHEX`.
    Dxbc,
    /// DXIL SM6: чанк `DXIL`.
    Dxil,
}

#[derive(Debug, Clone)]
pub struct Chunk<'a> {
    pub fourcc: [u8; 4],
    pub data: &'a [u8],
}

#[derive(Debug, Clone)]
pub struct ShaderInfo {
    pub bytecode: Bytecode,
    pub stage: Stage,
    pub model: (u8, u8),
    /// Длина программы в 32-битных токенах (для DXBC) или в байтах битового кода (для DXIL).
    pub program_len: usize,
    pub inputs: usize,
    pub outputs: usize,
    pub has_root_signature: bool,
    pub chunks: Vec<String>,
}

fn u32_at(b: &[u8], off: usize) -> Option<u32> {
    b.get(off..off + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

/// Разбирает контейнер DXBC: заголовок (magic, 16 байт хеша, версия, размер, число чанков)
/// и таблицу смещений чанков.
pub fn chunks(data: &[u8]) -> Result<Vec<Chunk<'_>>, ContainerError> {
    if data.len() < 32 {
        return Err(ContainerError::TooShort(data.len()));
    }
    if &data[0..4] != b"DXBC" {
        return Err(ContainerError::BadMagic);
    }
    let count = u32_at(data, 28).unwrap_or(0) as usize;
    let mut out = Vec::with_capacity(count);
    for index in 0..count {
        let off = u32_at(data, 32 + index * 4).ok_or(ContainerError::ChunkOutOfBounds { index })? as usize;
        let size = u32_at(data, off + 4).ok_or(ContainerError::ChunkOutOfBounds { index })? as usize;
        let body = data.get(off + 8..off + 8 + size).ok_or(ContainerError::ChunkOutOfBounds { index })?;
        out.push(Chunk { fourcc: [data[off], data[off + 1], data[off + 2], data[off + 3]], data: body });
    }
    Ok(out)
}

/// Элемент сигнатуры входов/выходов (ISGN/OSGN, по 24 байта).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignatureElement {
    pub semantic: String,
    pub index: u32,
    pub system_value: u32,
    pub component_type: u32,
    pub register: u32,
    pub mask: u8,
}

/// Разбор чанка ISGN/OSGN: [count, 8, элементы по 24 байта], имена — смещения от начала чанка.
pub fn signature(chunk: &Chunk<'_>) -> Vec<SignatureElement> {
    let d = chunk.data;
    let count = u32_at(d, 0).unwrap_or(0) as usize;
    (0..count)
        .filter_map(|i| {
            let e = 8 + i * 24;
            let name_off = u32_at(d, e)? as usize;
            let name_end = d.get(name_off..)?.iter().position(|b| *b == 0)? + name_off;
            Some(SignatureElement {
                semantic: String::from_utf8_lossy(&d[name_off..name_end]).into_owned(),
                index: u32_at(d, e + 4)?,
                system_value: u32_at(d, e + 8)?,
                component_type: u32_at(d, e + 12)?,
                register: u32_at(d, e + 16)?,
                mask: *d.get(e + 20)?,
            })
        })
        .collect()
}

/// Сигнатуры входов и выходов контейнера DXBC (если есть ISGN/OSGN).
pub fn signatures(data: &[u8]) -> Result<(Vec<SignatureElement>, Vec<SignatureElement>), ContainerError> {
    let (mut inputs, mut outputs) = (Vec::new(), Vec::new());
    for c in chunks(data)? {
        match &c.fourcc {
            b"ISGN" => inputs = signature(&c),
            b"OSGN" => outputs = signature(&c),
            _ => {}
        }
    }
    Ok((inputs, outputs))
}

/// Число элементов в сигнатуре ISGN/OSGN/ISG1/OSG1 (первое поле чанка).
fn signature_len(chunk: &Chunk<'_>) -> usize {
    u32_at(chunk.data, 0).unwrap_or(0) as usize
}

pub fn inspect(data: &[u8]) -> Result<Option<ShaderInfo>, ContainerError> {
    let chunks = chunks(data)?;
    let mut info: Option<ShaderInfo> = None;
    let (mut inputs, mut outputs, mut rts) = (0, 0, false);
    for c in &chunks {
        match &c.fourcc {
            b"SHDR" | b"SHEX" | b"DXIL" => {
                let token = u32_at(c.data, 0).unwrap_or(0);
                let bytecode = if &c.fourcc == b"DXIL" { Bytecode::Dxil } else { Bytecode::Dxbc };
                let program_len = match bytecode {
                    Bytecode::Dxbc => u32_at(c.data, 4).unwrap_or(0) as usize,
                    Bytecode::Dxil => c.data.len(),
                };
                info = Some(ShaderInfo {
                    bytecode,
                    stage: Stage::from_kind((token >> 16) as u16),
                    model: (((token >> 4) & 0xf) as u8, (token & 0xf) as u8),
                    program_len,
                    inputs: 0,
                    outputs: 0,
                    has_root_signature: false,
                    chunks: Vec::new(),
                });
            }
            b"ISGN" | b"ISG1" => inputs = signature_len(c),
            b"OSGN" | b"OSG5" | b"OSG1" => outputs = signature_len(c),
            b"RTS0" => rts = true,
            _ => {}
        }
    }
    Ok(info.map(|mut i| {
        i.inputs = inputs;
        i.outputs = outputs;
        i.has_root_signature = rts;
        i.chunks = chunks.iter().map(|c| String::from_utf8_lossy(&c.fourcc).into_owned()).collect();
        i
    }))
}

/// Ключ сводки: формат, стадия, модель шейдера.
pub type StageKey = (Bytecode, Stage, (u8, u8));

/// Сводка по каталогу корпуса (`*.dxbc`).
#[derive(Debug, Default)]
pub struct Inventory {
    pub files: usize,
    pub bytes: u64,
    pub broken: usize,
    /// Ключ → (файлов, суммарная длина программ).
    pub by_stage: BTreeMap<StageKey, (usize, usize)>,
    pub largest: Option<(String, usize)>,
}

pub fn inventory(dir: &Path) -> std::io::Result<Inventory> {
    let mut inv = Inventory::default();
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.extension().and_then(|e| e.to_str()) != Some("dxbc") {
            continue;
        }
        let data = std::fs::read(&path)?;
        inv.files += 1;
        inv.bytes += data.len() as u64;
        match inspect(&data) {
            Ok(Some(i)) => {
                let e = inv.by_stage.entry((i.bytecode, i.stage, i.model)).or_default();
                e.0 += 1;
                e.1 += i.program_len;
                if inv.largest.as_ref().is_none_or(|(_, n)| i.program_len > *n) {
                    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    inv.largest = Some((name, i.program_len));
                }
            }
            _ => inv.broken += 1,
        }
    }
    Ok(inv)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Минимальный синтетический контейнер: один чанк SHEX с токеном ps_5_0 и длиной 2.
    fn tiny_ps50() -> Vec<u8> {
        let mut d = Vec::new();
        d.extend_from_slice(b"DXBC");
        d.extend_from_slice(&[0u8; 16]); // хеш
        d.extend_from_slice(&1u32.to_le_bytes()); // версия контейнера
        let total = 32 + 4 + 8 + 8;
        d.extend_from_slice(&(total as u32).to_le_bytes());
        d.extend_from_slice(&1u32.to_le_bytes()); // один чанк
        d.extend_from_slice(&36u32.to_le_bytes()); // смещение чанка
        d.extend_from_slice(b"SHEX");
        d.extend_from_slice(&8u32.to_le_bytes());
        d.extend_from_slice(&0x0000_0050u32.to_le_bytes()); // ps (тип 0), 5.0
        d.extend_from_slice(&2u32.to_le_bytes()); // длина программы в токенах
        d
    }

    #[test]
    fn parses_dxbc_container() {
        let info = inspect(&tiny_ps50()).expect("разбор").expect("есть программа");
        assert_eq!(info.bytecode, Bytecode::Dxbc);
        assert_eq!(info.stage, Stage::Pixel);
        assert_eq!(info.model, (5, 0));
        assert_eq!(info.program_len, 2);
        assert_eq!(info.chunks, vec!["SHEX".to_string()]);
    }

    #[test]
    fn rejects_garbage() {
        assert!(matches!(inspect(b"not a shader at all, definitely not"), Err(ContainerError::BadMagic)));
        assert!(matches!(inspect(b"DXBC"), Err(ContainerError::TooShort(4))));
    }

    /// Реальный корпус, если он снят (данных игры в git нет — иначе тест пропускается).
    #[test]
    fn real_corpus_if_present() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../.local/shaders/eldenring");
        let Ok(inv) = inventory(&dir) else { return };
        if inv.files == 0 {
            return;
        }
        assert_eq!(inv.broken, 0, "все файлы корпуса должны разбираться");
    }
}
