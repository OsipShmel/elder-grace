fn main() {
    // Свой базовый адрес: дефолтный 0x180000000 занят libcef.dll и др., релокация не нужна.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rustc-cdylib-link-arg=-Wl,--image-base=0x6f1000000");
    }
}
