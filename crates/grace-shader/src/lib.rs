//! Elder Grace shader pipeline.
//!
//! Now: DXBC container parsing (it holds both DXBC SM4/5 and DXIL SM6) and a summary
//! of the shader corpus that the `d3d12.dll` shim dumps into `.local/shaders/<game>/`.
//! Next: translation to MSL and `metallib` builds.

use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum ContainerError {
    #[error("file too short ({0} bytes)")]
    TooShort(usize),
    #[error("no DXBC signature")]
    BadMagic,
    #[error("chunk {index} goes past the end of the file")]
    ChunkOutOfBounds { index: usize },
}

/// Program kind from the version token (`D3D10_SB_TOKENIZED_PROGRAM_TYPE` / DXIL program kind).
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

/// Bytecode format inside the container.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Bytecode {
    /// DXBC SM4/5: `SHDR` or `SHEX` chunk.
    Dxbc,
    /// DXIL SM6: `DXIL` chunk.
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
    /// Program length in 32-bit tokens (DXBC) or in bitcode bytes (DXIL).
    pub program_len: usize,
    pub inputs: usize,
    pub outputs: usize,
    pub has_root_signature: bool,
    pub chunks: Vec<String>,
}

fn u32_at(b: &[u8], off: usize) -> Option<u32> {
    b.get(off..off + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

/// Parses a DXBC container: header (magic, 16-byte hash, version, size, chunk count)
/// and the chunk offset table.
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

/// Input/output signature element (ISGN/OSGN, 24 bytes each).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignatureElement {
    pub semantic: String,
    pub index: u32,
    pub system_value: u32,
    pub component_type: u32,
    pub register: u32,
    pub mask: u8,
}

/// Parses an ISGN/OSGN chunk: [count, 8, 24-byte elements]; names are offsets from the chunk start.
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

/// Parses an ISG1/OSG1 chunk (DXIL): [count, 8, 32-byte elements]:
/// stream, name (offset from the chunk start), index, system value, component type,
/// register, mask, read/write mask, 2 padding bytes, min precision.
pub fn signature1(chunk: &Chunk<'_>) -> Vec<SignatureElement> {
    let d = chunk.data;
    let count = u32_at(d, 0).unwrap_or(0) as usize;
    (0..count)
        .filter_map(|i| {
            let e = 8 + i * 32;
            let name_off = u32_at(d, e + 4)? as usize;
            let name_end = d.get(name_off..)?.iter().position(|b| *b == 0)? + name_off;
            Some(SignatureElement {
                semantic: String::from_utf8_lossy(&d[name_off..name_end]).into_owned(),
                index: u32_at(d, e + 8)?,
                system_value: u32_at(d, e + 12)?,
                component_type: u32_at(d, e + 16)?,
                register: u32_at(d, e + 20)?,
                mask: *d.get(e + 24)?,
            })
        })
        .collect()
}

/// Input and output signatures of a container (ISGN/OSGN for DXBC, ISG1/OSG1 for DXIL).
pub fn signatures(data: &[u8]) -> Result<(Vec<SignatureElement>, Vec<SignatureElement>), ContainerError> {
    let (mut inputs, mut outputs) = (Vec::new(), Vec::new());
    for c in chunks(data)? {
        match &c.fourcc {
            b"ISGN" => inputs = signature(&c),
            b"OSGN" => outputs = signature(&c),
            b"ISG1" => inputs = signature1(&c),
            b"OSG1" => outputs = signature1(&c),
            _ => {}
        }
    }
    Ok((inputs, outputs))
}

/// Shader debug name from the ILDN chunk (DXIL): { u16 flags, u16 length, name\0 }.
/// For games built with PDBs this is the source path, which lets you search shaders by meaning.
pub fn debug_name(data: &[u8]) -> Option<String> {
    let c = chunks(data).ok()?.into_iter().find(|c| &c.fourcc == b"ILDN")?;
    let len = u16::from_le_bytes([*c.data.get(2)?, *c.data.get(3)?]) as usize;
    let name = c.data.get(4..4 + len)?;
    Some(String::from_utf8_lossy(name).into_owned())
}

/// Short name from a debug path: the file without directory and without `.ppo.pdb`/`.pdb`.
pub fn short_name(debug: &str) -> &str {
    let file = debug.rsplit(['\\', '/']).next().unwrap_or(debug);
    file.split('.').next().unwrap_or(file)
}

/// Element count of an ISGN/OSGN/ISG1/OSG1 signature (first field of the chunk).
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

/// Summary key: format, stage, shader model.
pub type StageKey = (Bytecode, Stage, (u8, u8));

/// Summary of a corpus directory (`*.dxbc`).
#[derive(Debug, Default)]
pub struct Inventory {
    pub files: usize,
    pub bytes: u64,
    pub broken: usize,
    /// Key -> (files, total program length).
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

    /// Minimal synthetic container: one SHEX chunk with a ps_5_0 token and length 2.
    fn tiny_ps50() -> Vec<u8> {
        let mut d = Vec::new();
        d.extend_from_slice(b"DXBC");
        d.extend_from_slice(&[0u8; 16]); // hash
        d.extend_from_slice(&1u32.to_le_bytes()); // container version
        let total = 32 + 4 + 8 + 8;
        d.extend_from_slice(&(total as u32).to_le_bytes());
        d.extend_from_slice(&1u32.to_le_bytes()); // one chunk
        d.extend_from_slice(&36u32.to_le_bytes()); // chunk offset
        d.extend_from_slice(b"SHEX");
        d.extend_from_slice(&8u32.to_le_bytes());
        d.extend_from_slice(&0x0000_0050u32.to_le_bytes()); // ps (type 0), 5.0
        d.extend_from_slice(&2u32.to_le_bytes()); // program length in tokens
        d
    }

    /// Container with a single chunk.
    fn container(fourcc: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut d = Vec::new();
        d.extend_from_slice(b"DXBC");
        d.extend_from_slice(&[0u8; 16]);
        d.extend_from_slice(&1u32.to_le_bytes());
        d.extend_from_slice(&((36 + 8 + body.len()) as u32).to_le_bytes());
        d.extend_from_slice(&1u32.to_le_bytes());
        d.extend_from_slice(&36u32.to_le_bytes());
        d.extend_from_slice(fourcc);
        d.extend_from_slice(&(body.len() as u32).to_le_bytes());
        d.extend_from_slice(body);
        d
    }

    #[test]
    fn parses_dxil_signature_and_debug_name() {
        // ISG1: one element TEXCOORD5, register 5, mask xyzw, float (3).
        let mut b = Vec::new();
        for v in [1u32, 8, 0, 40, 5, 0, 3, 5] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        b.extend_from_slice(&[0x0f, 0x0f, 0, 0]);
        b.extend_from_slice(&0u32.to_le_bytes());
        b.extend_from_slice(b"TEXCOORD\0");
        let (inputs, outputs) = signatures(&container(b"ISG1", &b)).expect("parse");
        assert!(outputs.is_empty());
        assert_eq!(
            inputs,
            vec![SignatureElement { semantic: "TEXCOORD".into(), index: 5, system_value: 0, component_type: 3, register: 5, mask: 0x0f }]
        );

        let path = b"N:\\GR\\shader\\M_Skin_Velo.ppo.pdb";
        let mut n = vec![0u8, 0];
        n.extend_from_slice(&(path.len() as u16).to_le_bytes());
        n.extend_from_slice(path);
        n.push(0);
        let name = debug_name(&container(b"ILDN", &n)).expect("name");
        assert_eq!(name, "N:\\GR\\shader\\M_Skin_Velo.ppo.pdb");
        assert_eq!(short_name(&name), "M_Skin_Velo");
    }

    #[test]
    fn parses_dxbc_container() {
        let info = inspect(&tiny_ps50()).expect("parse").expect("has a program");
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

    /// Real corpora, if dumped (game data is not in git, otherwise the test is skipped).
    #[test]
    fn real_corpus_if_present() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../.local/shaders");
        let Ok(games) = std::fs::read_dir(&root) else { return };
        for game in games.flatten() {
            let Ok(inv) = inventory(&game.path()) else { continue };
            assert_eq!(inv.broken, 0, "every corpus file must parse: {}", game.path().display());
        }
    }
}
