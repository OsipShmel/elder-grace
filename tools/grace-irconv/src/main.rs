//! `grace-irconv <каталог .dxbc> <каталог вывода>` — DXIL-шейдеры корпуса → `.metallib`.
//!
//! Использует Apple Metal Shader Converter (`libmetalirconverter.dylib`), который лежит
//! внутри D3DMetal в установленном GPTK. Библиотека только x86_64, поэтому инструмент
//! собирается под `x86_64-apple-darwin` и работает под Rosetta: это офлайн-шаг конвейера,
//! а результат — обычные `metallib`, которые грузит нативный arm64-рантайм.
//! Проприетарная библиотека используется локально и никуда не распространяется.
//!
//! Шейдеры DXBC SM4/5 этим путём не конвертируются (конвертер понимает только DXIL) —
//! их переводит вторая ветка `cargo xtask shaders` (vkd3d → SPIR-V → SPIRV-Cross → MSL).

use std::ffi::{CStr, CString, c_char, c_void};
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use grace_shader::{Bytecode, inspect};

const DEFAULT_LIB: &str =
    "/Applications/Game Porting Toolkit.app/Contents/Resources/wine/lib/external/D3DMetal.framework/Versions/A/Resources/libmetalirconverter.dylib";

unsafe extern "C" {
    fn dlopen(path: *const c_char, mode: i32) -> *mut c_void;
    fn dlsym(handle: *mut c_void, name: *const c_char) -> *mut c_void;
    fn dlerror() -> *const c_char;
}

type Ptr = *mut c_void;

/// Адрес символа → указатель на функцию типа поля `Ir` (тип выводится из поля).
///
/// # Safety
/// `F` — указатель на функцию с той же сигнатурой, что у символа.
unsafe fn func<F: Copy>(p: Ptr) -> F {
    assert_eq!(std::mem::size_of::<F>(), std::mem::size_of::<Ptr>());
    // SAFETY: размеры равны; совместимость сигнатуры — по контракту функции.
    unsafe { std::mem::transmute_copy(&p) }
}

/// Функции C API Metal Shader Converter, нужные для DXIL → metallib.
struct Ir {
    compiler_create: unsafe extern "C" fn() -> Ptr,
    compiler_destroy: unsafe extern "C" fn(Ptr),
    object_from_dxil: unsafe extern "C" fn(*const u8, usize, u32) -> Ptr,
    compile_and_link: unsafe extern "C" fn(Ptr, *const c_char, Ptr, *mut Ptr) -> Ptr,
    object_stage: unsafe extern "C" fn(Ptr) -> u32,
    object_metallib: unsafe extern "C" fn(Ptr, u32, Ptr) -> bool,
    object_destroy: unsafe extern "C" fn(Ptr),
    metallib_create: unsafe extern "C" fn() -> Ptr,
    metallib_destroy: unsafe extern "C" fn(Ptr),
    metallib_size: unsafe extern "C" fn(Ptr) -> usize,
    metallib_bytes: unsafe extern "C" fn(Ptr, *mut u8) -> usize,
    error_code: unsafe extern "C" fn(Ptr) -> u32,
    error_destroy: unsafe extern "C" fn(Ptr),
}

impl Ir {
    fn load(path: &str) -> Result<Self> {
        let cpath = CString::new(path)?;
        // SAFETY: dlopen с RTLD_NOW (2); ошибка проверяется.
        let h = unsafe { dlopen(cpath.as_ptr(), 2) };
        if h.is_null() {
            // SAFETY: dlerror возвращает строку C или null.
            let err = unsafe { dlerror() };
            let msg = if err.is_null() { "?".into() } else { unsafe { CStr::from_ptr(err) }.to_string_lossy().into_owned() };
            bail!("dlopen {path}: {msg}");
        }
        let sym = |name: &str| -> Result<*mut c_void> {
            let c = CString::new(name)?;
            // SAFETY: хендл получен выше.
            let p = unsafe { dlsym(h, c.as_ptr()) };
            if p.is_null() { bail!("нет символа {name}") } else { Ok(p) }
        };
        // SAFETY: сигнатуры по metal_irconverter.h (Metal Shader Converter).
        unsafe {
            Ok(Self {
                compiler_create: func(sym("IRCompilerCreate")?),
                compiler_destroy: func(sym("IRCompilerDestroy")?),
                object_from_dxil: func(sym("IRObjectCreateFromDXIL")?),
                compile_and_link: func(sym("IRCompilerAllocCompileAndLink")?),
                object_stage: func(sym("IRObjectGetMetalIRShaderStage")?),
                object_metallib: func(sym("IRObjectGetMetalLibBinary")?),
                object_destroy: func(sym("IRObjectDestroy")?),
                metallib_create: func(sym("IRMetalLibBinaryCreate")?),
                metallib_destroy: func(sym("IRMetalLibBinaryDestroy")?),
                metallib_size: func(sym("IRMetalLibGetBytecodeSize")?),
                metallib_bytes: func(sym("IRMetalLibGetBytecode")?),
                error_code: func(sym("IRErrorGetCode")?),
                error_destroy: func(sym("IRErrorDestroy")?),
            })
        }
    }

    /// Один DXIL-контейнер → байты metallib.
    fn convert(&self, compiler: Ptr, dxil: &[u8]) -> Result<Vec<u8>, String> {
        // SAFETY: вызовы C API с проверкой каждого результата; все объекты освобождаются.
        unsafe {
            let input = (self.object_from_dxil)(dxil.as_ptr(), dxil.len(), 0); // IRBytecodeOwnershipNone
            if input.is_null() {
                return Err("IRObjectCreateFromDXIL вернул null".into());
            }
            let mut err: Ptr = std::ptr::null_mut();
            let out = (self.compile_and_link)(compiler, std::ptr::null(), input, &mut err);
            (self.object_destroy)(input);
            if out.is_null() {
                let code = if err.is_null() { 0 } else { (self.error_code)(err) };
                if !err.is_null() {
                    (self.error_destroy)(err);
                }
                return Err(format!("компиляция не удалась, код ошибки {code}"));
            }
            let stage = (self.object_stage)(out);
            let lib = (self.metallib_create)();
            let ok = (self.object_metallib)(out, stage, lib);
            let result = if ok {
                let mut bytes = vec![0u8; (self.metallib_size)(lib)];
                (self.metallib_bytes)(lib, bytes.as_mut_ptr());
                Ok(bytes)
            } else {
                Err(format!("нет metallib для стадии {stage}"))
            };
            (self.metallib_destroy)(lib);
            (self.object_destroy)(out);
            result
        }
    }
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (Some(input), Some(output)) = (args.first(), args.get(1)) else {
        bail!("использование: grace-irconv <каталог .dxbc> <каталог вывода>");
    };
    let lib = std::env::var("GRACE_IRCONVERTER").unwrap_or_else(|_| DEFAULT_LIB.into());
    let ir = Ir::load(&lib)?;
    let out_dir = PathBuf::from(output);
    std::fs::create_dir_all(&out_dir)?;

    // SAFETY: создание и уничтожение компилятора парные.
    let compiler = unsafe { (ir.compiler_create)() };
    if compiler.is_null() {
        bail!("IRCompilerCreate вернул null");
    }
    let (mut ok, mut failed, mut dxbc) = (0, 0, 0);
    let mut files: Vec<PathBuf> = std::fs::read_dir(input)
        .with_context(|| format!("чтение {input}"))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("dxbc"))
        .collect();
    files.sort();
    for path in files {
        let data = std::fs::read(&path)?;
        let Ok(Some(info)) = inspect(&data) else { continue };
        if info.bytecode != Bytecode::Dxil {
            dxbc += 1;
            continue;
        }
        let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        match ir.convert(compiler, &data) {
            Ok(bytes) => {
                std::fs::write(out_dir.join(format!("{stem}.metallib")), &bytes)?;
                ok += 1;
            }
            Err(e) => {
                eprintln!("{stem}: {e}");
                failed += 1;
            }
        }
    }
    // SAFETY: компилятор создан выше.
    unsafe { (ir.compiler_destroy)(compiler) };
    println!("DXIL → metallib: успешно {ok}, ошибок {failed}; DXBC SM4/5 пропущено {dxbc} (их переводит ветка vkd3d → SPIRV-Cross)");
    Ok(())
}
