//! Подмены на лету: включаем и выключаем подменённые шейдеры в работающей игре.
//!
//! PSO в D3D12 неизменяемы, поэтому для пайплайна с подменой создаются два PSO:
//! оригинальный (его получает игра) и подменённый. Командным спискам подставляется
//! копия vtable с перехватом `SetPipelineState`, `Reset` и `ClearState`: если подмены
//! включены и PSO есть в таблице пар, в D3DMetal уходит подменённый.
//!
//! Управление — TCP `127.0.0.1:47322` (`GRACE_D3D12_PORT`), протокол `grace-proto`.

use std::collections::BTreeSet;
use std::ffi::c_void;
use std::io::BufReader;
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

use grace_proto::{OverrideOp, OverrideStatus, PROTOCOL_VERSION, Request, Response, read_message, write_message};

use crate::debug;

// ---- таблица пар PSO -------------------------------------------------------------------------

/// Ёмкость таблицы (степень двойки). Подмен — единицы, пар — десятки; запас огромный.
const CAP: usize = 4096;

/// Открытая адресация: читатели без блокировок (горячий путь `SetPipelineState`),
/// писатель один под `INSERT`. Значение пишется раньше ключа, ключ публикуется с Release.
static KEYS: [AtomicUsize; CAP] = [const { AtomicUsize::new(0) }; CAP];
static VALUES: [AtomicUsize; CAP] = [const { AtomicUsize::new(0) }; CAP];
static INSERT: Mutex<()> = Mutex::new(());

static PAIRS: AtomicUsize = AtomicUsize::new(0);
static FAILED: AtomicUsize = AtomicUsize::new(0);
static SWAPS: AtomicU64 = AtomicU64::new(0);
static ENABLED: AtomicBool = AtomicBool::new(true);

fn slot_of(key: usize) -> usize {
    // Указатели выровнены минимум на 8: младшие биты бесполезны для хеша.
    ((key >> 4).wrapping_mul(0x9e37_79b9_7f4a_7c15) >> 52) & (CAP - 1)
}

/// Ставит пару «оригинал → подменённый» (`None` — убрать подмену у оригинала).
/// Возвращает true, если у оригинала до этого была активная подмена.
///
/// Оригинал должен оставаться живым, пока пара в таблице: иначе его адрес достанется
/// другому PSO. Ссылку на него держат рецепты (`recipes.rs`).
pub fn set_pair(orig: usize, over: Option<usize>) -> bool {
    if orig == 0 {
        return false;
    }
    let value = over.unwrap_or(orig);
    let Ok(_guard) = INSERT.lock() else { return false };
    let mut i = slot_of(orig);
    loop {
        let k = KEYS[i].load(Ordering::Relaxed);
        if k == orig {
            let prev = VALUES[i].swap(value, Ordering::AcqRel);
            return prev != orig;
        }
        if k == 0 {
            if over.is_none() {
                return false;
            }
            if PAIRS.load(Ordering::Relaxed) >= CAP / 2 {
                debug("таблица пар PSO заполнена, подмена не зарегистрирована");
                return false;
            }
            VALUES[i].store(value, Ordering::Relaxed);
            KEYS[i].store(orig, Ordering::Release);
            PAIRS.fetch_add(1, Ordering::Relaxed);
            return false;
        }
        i = (i + 1) & (CAP - 1);
    }
}

/// Активных пар (подменённый отличается от оригинала).
fn active_pairs() -> usize {
    KEYS.iter().zip(&VALUES).filter(|(k, v)| {
        let k = k.load(Ordering::Acquire);
        k != 0 && v.load(Ordering::Relaxed) != k
    }).count()
}

pub fn note_failed() {
    FAILED.fetch_add(1, Ordering::Relaxed);
}

// ---- запись привязок: какие шейдеры рисуют сейчас ---------------------------------------

static RECORDING: AtomicBool = AtomicBool::new(false);
static BOUND: Mutex<Option<std::collections::HashMap<usize, u64>>> = Mutex::new(None);

#[cold]
fn record_bind(pso: usize) {
    if let Ok(mut b) = BOUND.lock() {
        *b.get_or_insert_with(Default::default).entry(pso).or_default() += 1;
    }
}

/// DXIL-контейнер: среди чанков есть `DXIL`.
fn is_dxil(bytes: &[u8]) -> bool {
    let u32_at = |o: usize| bytes.get(o..o + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    let Some(n) = u32_at(28) else { return false };
    (0..n as usize).filter_map(|i| u32_at(32 + i * 4)).any(|off| bytes.get(off as usize..off as usize + 4) == Some(b"DXIL"))
}

/// Записывает привязки PSO в течение `ms` и сводит их по шейдерам.
fn bound_report(ms: u32) -> grace_proto::BoundReport {
    if let Ok(mut b) = BOUND.lock() {
        *b = Some(Default::default());
    }
    RECORDING.store(true, Ordering::Release);
    std::thread::sleep(std::time::Duration::from_millis(u64::from(ms.clamp(16, 30_000))));
    RECORDING.store(false, Ordering::Release);
    let binds = BOUND.lock().ok().and_then(|mut b| b.take()).unwrap_or_default();
    let mut per: std::collections::HashMap<(u64, &'static str), (u64, u32)> = Default::default();
    let mut unknown = 0;
    for (&pso, &n) in &binds {
        let Some(shaders) = crate::recipes::shaders_of(pso) else {
            unknown += 1;
            continue;
        };
        for (stage, hash) in shaders {
            let e = per.entry((hash, stage)).or_default();
            e.0 += n;
            e.1 += 1;
        }
    }
    let dump = crate::dump_dir();
    let mut shaders: Vec<grace_proto::BoundShader> = per
        .into_iter()
        .map(|((hash, stage), (binds, pipelines))| {
            let name = format!("{hash:016x}.{stage}");
            let dxil = dump.and_then(|d| std::fs::read(d.join(format!("{name}.dxbc"))).ok()).is_some_and(|b| is_dxil(&b));
            grace_proto::BoundShader { name, binds, pipelines, dxil }
        })
        .collect();
    shaders.sort_by(|a, b| b.binds.cmp(&a.binds).then_with(|| a.name.cmp(&b.name)));
    grace_proto::BoundReport { ms, pipelines: binds.len(), unknown, shaders }
}

/// PSO, который надо отдать D3DMetal вместо `pso`.
#[inline]
fn swap(pso: *mut c_void) -> *mut c_void {
    if RECORDING.load(Ordering::Relaxed) && !pso.is_null() {
        record_bind(pso as usize);
    }
    if pso.is_null() || !ENABLED.load(Ordering::Relaxed) || PAIRS.load(Ordering::Relaxed) == 0 {
        return pso;
    }
    let key = pso as usize;
    let mut i = slot_of(key);
    loop {
        let k = KEYS[i].load(Ordering::Acquire);
        if k == 0 {
            return pso;
        }
        if k == key {
            let v = VALUES[i].load(Ordering::Acquire);
            if v == key {
                return pso;
            }
            SWAPS.fetch_add(1, Ordering::Relaxed);
            return v as *mut c_void;
        }
        i = (i + 1) & (CAP - 1);
    }
}

// ---- командные списки -----------------------------------------------------------------------

// Слоты ID3D12GraphicsCommandList (d3d12.h): IUnknown 0–2, ID3D12Object 3–6,
// GetDevice 7, GetType 8, Close 9, Reset 10, ClearState 11, …, SetPipelineState 25.
// У ID3D12GraphicsCommandList7 81 метод; берём с запасом, как у устройства (128 слотов
// там читаются без проблем — vtable D3DMetal лежит в общей секции констант).
const CL_SLOTS: usize = 96;
const CL_QI: usize = 0;
const CL_RESET: usize = 10;
const CL_CLEAR_STATE: usize = 11;
const CL_SET_PSO: usize = 25;

/// Известные исходные vtable и наши копии. У копии на индексе −1 лежит адрес исходной
/// vtable: перехватчик берёт оригинальный метод через `this`, поэтому разных vtable
/// (разные версии интерфейса) может быть несколько.
static CL_VTBLS: Mutex<Vec<(usize, usize)>> = Mutex::new(Vec::new());

/// IID ID3D12GraphicsCommandList … 7 (d3d12.h): QueryInterface к ним может вернуть другой объект.
const CL_IIDS: [[u8; 16]; 8] = [
    guid(0x5b160d0f, 0xac1b, 0x4185, [0x8b, 0xa8, 0xb3, 0xae, 0x42, 0xa5, 0xa4, 0x55]),
    guid(0x553103fb, 0x1fe7, 0x4557, [0xbb, 0x38, 0x94, 0x6d, 0x7d, 0x0e, 0x7c, 0xa7]),
    guid(0x38c3e585, 0xff17, 0x412c, [0x91, 0x50, 0x4f, 0xc6, 0xf9, 0xd7, 0x2a, 0x28]),
    guid(0x6fda83a7, 0xb84c, 0x4e38, [0x9a, 0xc8, 0xc7, 0xbd, 0x22, 0x01, 0x6b, 0x3d]),
    guid(0x8754318e, 0xd3a9, 0x4541, [0x98, 0xcf, 0x64, 0x5b, 0x50, 0xdc, 0x48, 0x74]),
    guid(0x55050859, 0x4024, 0x474c, [0x87, 0xf5, 0x64, 0x72, 0xea, 0xee, 0x44, 0xea]),
    guid(0xc3827890, 0xe548, 0x4cfa, [0x96, 0xcf, 0x56, 0x89, 0xa9, 0x37, 0x0f, 0x80]),
    guid(0xdd171223, 0x8b61, 0x4769, [0x90, 0xe3, 0x16, 0x0c, 0xcd, 0xe4, 0xe2, 0xc1]),
];

pub const fn guid(a: u32, b: u16, c: u16, d: [u8; 8]) -> [u8; 16] {
    let a = a.to_le_bytes();
    let b = b.to_le_bytes();
    let c = c.to_le_bytes();
    [a[0], a[1], a[2], a[3], b[0], b[1], c[0], c[1], d[0], d[1], d[2], d[3], d[4], d[5], d[6], d[7]]
}

/// Подставляет командному списку нашу копию vtable.
///
/// # Safety
/// `list` — живой объект ID3D12GraphicsCommandList (или null).
pub unsafe fn adopt_command_list(list: *mut c_void) {
    if list.is_null() {
        return;
    }
    let Ok(mut known) = CL_VTBLS.lock() else { return };
    // SAFETY: первое поле COM-объекта — указатель на vtable.
    let vtbl = unsafe { *(list as *const usize) };
    if known.iter().any(|&(_, clone)| clone == vtbl) {
        return;
    }
    let clone = match known.iter().find(|&&(orig, _)| orig == vtbl) {
        Some(&(_, clone)) => clone,
        None => {
            let mut copy: Box<[usize; CL_SLOTS + 1]> = Box::new([0; CL_SLOTS + 1]);
            copy[0] = vtbl;
            // SAFETY: читаем CL_SLOTS указателей vtable (константные данные D3DMetal).
            unsafe { std::ptr::copy_nonoverlapping(vtbl as *const usize, copy.as_mut_ptr().add(1), CL_SLOTS) };
            copy[1 + CL_QI] = cl_query_interface as *const () as usize;
            copy[1 + CL_RESET] = cl_reset as *const () as usize;
            copy[1 + CL_CLEAR_STATE] = cl_clear_state as *const () as usize;
            copy[1 + CL_SET_PSO] = cl_set_pipeline_state as *const () as usize;
            let clone = Box::leak(copy).as_ptr() as usize + 8;
            known.push((vtbl, clone));
            debug(&format!("командным спискам подставлена своя vtable (вариант {})", known.len()));
            clone
        }
    };
    // SAFETY: объект в куче, первое поле можно переписать.
    unsafe { std::ptr::write_volatile(list as *mut usize, clone) };
}

/// Оригинальный метод `slot` для объекта с нашей копией vtable.
///
/// # Safety
/// `this` — объект, чья vtable создана `adopt_command_list`.
#[inline]
unsafe fn original(this: *mut c_void, slot: usize) -> usize {
    // SAFETY: у нашей копии на индексе −1 лежит адрес исходной vtable.
    unsafe {
        let clone = *(this as *const *const usize);
        let orig = *clone.sub(1) as *const usize;
        *orig.add(slot)
    }
}

unsafe extern "system" fn cl_query_interface(this: *mut c_void, riid: *const [u8; 16], out: *mut *mut c_void) -> i32 {
    type F = unsafe extern "system" fn(*mut c_void, *const [u8; 16], *mut *mut c_void) -> i32;
    // SAFETY: оригинал из исходной vtable.
    let f: F = unsafe { std::mem::transmute(original(this, CL_QI)) };
    let hr = unsafe { f(this, riid, out) };
    if hr >= 0 && !out.is_null() && !riid.is_null() {
        // SAFETY: при успехе *out — объект запрошенного интерфейса.
        let obj = unsafe { *out };
        if obj != this && CL_IIDS.contains(unsafe { &*riid }) {
            unsafe { adopt_command_list(obj) };
        }
    }
    hr
}

unsafe extern "system" fn cl_reset(this: *mut c_void, allocator: *mut c_void, pso: *mut c_void) -> i32 {
    type F = unsafe extern "system" fn(*mut c_void, *mut c_void, *mut c_void) -> i32;
    // SAFETY: оригинал из исходной vtable.
    let f: F = unsafe { std::mem::transmute(original(this, CL_RESET)) };
    unsafe { f(this, allocator, swap(pso)) }
}

unsafe extern "system" fn cl_clear_state(this: *mut c_void, pso: *mut c_void) {
    type F = unsafe extern "system" fn(*mut c_void, *mut c_void);
    // SAFETY: оригинал из исходной vtable.
    let f: F = unsafe { std::mem::transmute(original(this, CL_CLEAR_STATE)) };
    unsafe { f(this, swap(pso)) }
}

unsafe extern "system" fn cl_set_pipeline_state(this: *mut c_void, pso: *mut c_void) {
    type F = unsafe extern "system" fn(*mut c_void, *mut c_void);
    // SAFETY: оригинал из исходной vtable.
    let f: F = unsafe { std::mem::transmute(original(this, CL_SET_PSO)) };
    unsafe { f(this, swap(pso)) }
}

/// Подставляемый в CreateCommandList начальный PSO.
pub fn initial_pso(pso: *mut c_void) -> *mut c_void {
    swap(pso)
}

// ---- применённые подмены и канал управления ------------------------------------------------

static APPLIED: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());
static LAST_RELOAD: Mutex<String> = Mutex::new(String::new());

pub fn note_applied(name: String) {
    if let Ok(mut a) = APPLIED.lock() {
        a.insert(name);
    }
}

fn status() -> OverrideStatus {
    OverrideStatus {
        enabled: ENABLED.load(Ordering::Relaxed),
        shaders_loaded: crate::overrides::loaded(),
        pairs: active_pairs(),
        failed: FAILED.load(Ordering::Relaxed),
        swaps: SWAPS.load(Ordering::Relaxed),
        applied: APPLIED.lock().map(|a| a.iter().cloned().collect()).unwrap_or_default(),
        recipes: crate::recipes::count(),
        last_reload: LAST_RELOAD.lock().map(|s| s.clone()).unwrap_or_default(),
    }
}

const DEFAULT_PORT: u16 = 47322;

/// Поднимает TCP-сервер управления (один раз, не из DllMain).
pub fn start_control() {
    static STARTED: OnceLock<()> = OnceLock::new();
    STARTED.get_or_init(|| {
        let port = std::env::var("GRACE_D3D12_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(DEFAULT_PORT);
        match TcpListener::bind(("127.0.0.1", port)) {
            Ok(listener) => {
                debug(&format!("управление подменами: tcp 127.0.0.1:{port}"));
                let _ = std::thread::Builder::new().name("grace-d3d12-ctl".into()).spawn(move || {
                    for conn in listener.incoming().flatten() {
                        let _ = std::thread::Builder::new().name("grace-d3d12-conn".into()).spawn(move || serve(conn));
                    }
                });
            }
            Err(e) => debug(&format!("порт управления {port} занят: {e}")),
        }
    });
}

fn serve(conn: TcpStream) {
    let Ok(mut writer) = conn.try_clone() else { return };
    let mut reader = BufReader::new(conn);
    loop {
        let response = match read_message::<_, Request>(&mut reader) {
            Ok(None) => return,
            Ok(Some(Request::Ping)) => Response::Pong {
                protocol: PROTOCOL_VERSION,
                app: "shim-d3d12".into(),
                version: env!("CARGO_PKG_VERSION").into(),
            },
            Ok(Some(Request::Stats)) => Response::Error { message: "статистика кадра — в shim dxgi (порт 47321)".into() },
            Ok(Some(Request::Camera { layout })) => Response::Camera(crate::resources::find_camera(&layout)),
            Ok(Some(Request::Bound { ms })) => {
                if crate::recipes::active() {
                    Response::Bound(bound_report(ms))
                } else {
                    Response::Error { message: "нет рецептов PSO: запусти игру с каталогом подмен (GRACE_SHADER_OVERRIDE)".into() }
                }
            }
            Ok(Some(Request::Overrides { op })) => {
                let before = ENABLED.load(Ordering::Relaxed);
                if op == OverrideOp::Reload {
                    let summary = crate::reload_overrides();
                    debug(&summary);
                    if let Ok(mut l) = LAST_RELOAD.lock() {
                        *l = summary;
                    }
                }
                let after = match op {
                    OverrideOp::Status | OverrideOp::Reload => before,
                    OverrideOp::On => true,
                    OverrideOp::Off => false,
                    OverrideOp::Toggle => !before,
                };
                if after != before {
                    ENABLED.store(after, Ordering::Relaxed);
                    debug(&format!("подмены шейдеров: {}", if after { "включены" } else { "выключены" }));
                }
                Response::Overrides(status())
            }
            Err(e) => Response::Error { message: format!("неверный запрос: {e}") },
        };
        if write_message(&mut writer, &response).is_err() {
            return;
        }
    }
}
