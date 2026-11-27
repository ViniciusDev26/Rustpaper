// Entry points of the Windows artifacts (no C runtime: rust-lld would
// otherwise look for the CRT's). The .exe starts at `mainCRTStartup`
// (src/main.rs); the DLL has no entry point at all: it needs no
// per-process or per-thread setup.
fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rustc-link-arg-bins=/ENTRY:mainCRTStartup");
        println!("cargo:rustc-link-arg-bins=/SUBSYSTEM:CONSOLE");
        println!("cargo:rustc-link-arg-cdylib=/NOENTRY");
    }
}
