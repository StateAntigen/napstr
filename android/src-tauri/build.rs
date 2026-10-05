fn main() {
    // Android's own logger. `__android_log_write` lives in liblog, which is not
    // linked by default - without this line the phone's diagnostics compile and
    // then fail to link, which is a puzzling way to learn it.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("android") {
        println!("cargo:rustc-link-lib=log");
    }
    tauri_build::build()
}
