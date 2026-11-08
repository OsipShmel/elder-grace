//! Own image base and exports with the same ordinals as GPTK's dxgi.dll.

const EXPORTS: &[(&str, u16)] = &[
    ("CreateDXGIFactory", 1),
    ("CreateDXGIFactory1", 2),
    ("CreateDXGIFactory2", 3),
    ("DXGID3D10CreateDevice", 4),
    ("DXGID3D10RegisterLayers", 5),
    ("DXGIGetDebugInterface1", 6),
    ("DXGIDeclareAdapterRemovalSupport", 7),
];

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    println!("cargo:rustc-cdylib-link-arg=-Wl,--image-base=0x6f3000000");
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    let mut def = String::from("EXPORTS\n");
    for (name, ord) in EXPORTS {
        def += &format!("    {name} @{ord}\n");
    }
    let path = out.join("dxgi.def");
    std::fs::write(&path, def).expect("write .def");
    println!("cargo:rustc-cdylib-link-arg={}", path.display());
}
