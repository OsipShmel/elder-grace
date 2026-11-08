//! Own image base and exports with the same ordinals as the real d3d12.dll:
//! games import D3D12CreateDevice by ordinal 101 and D3D12GetDebugInterface by 102.

const EXPORTS: &[(&str, u16)] = &[
    ("GetBehaviorValue", 100),
    ("D3D12CreateDevice", 101),
    ("D3D12GetDebugInterface", 102),
    ("D3D12CoreCreateLayeredDevice", 103),
    ("D3D12CoreGetLayeredDeviceSize", 104),
    ("D3D12CoreRegisterLayers", 105),
    ("D3D12CreateRootSignatureDeserializer", 106),
    ("D3D12CreateVersionedRootSignatureDeserializer", 107),
    ("D3D12EnableExperimentalFeatures", 108),
    ("D3D12SerializeRootSignature", 109),
    ("D3D12SerializeVersionedRootSignature", 110),
    ("grace_shim_version", 200),
];

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    // The default 0x180000000 is taken by libcef.dll and others.
    println!("cargo:rustc-cdylib-link-arg=-Wl,--image-base=0x6f2000000");
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    let mut def = String::from("EXPORTS\n");
    for (name, ord) in EXPORTS {
        def += &format!("    {name} @{ord}\n");
    }
    let path = out.join("d3d12.def");
    std::fs::write(&path, def).expect("write .def");
    println!("cargo:rustc-cdylib-link-arg={}", path.display());
}
