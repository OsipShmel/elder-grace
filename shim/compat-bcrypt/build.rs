fn main() {
    // Own image base: the default 0x180000000 is taken by libcef.dll and others, so no relocation is needed.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rustc-cdylib-link-arg=-Wl,--image-base=0x6f0000000");
    }
}
