// libsteam_api.so (Steamworks SDK redistributable, bundled by steamworks-sys)
// is linked dynamically. The binary looks for it next to itself (target/
// <profile>/ while developing) and in ../lib/rustpaper (/usr/bin ->
// /usr/lib/rustpaper when installed; packaging/dist.sh puts it there).
//
// steamworks-sys extracts the library into its build script's OUT_DIR; being
// a build-dependency here, that has run before this script, so the copy is
// found under target/<profile>/build. Cargo's build-dir layout changed
// (`build/<pkg>-<hash>/out` -> `build/<pkg>/<hash>/out` in newer releases),
// so both are searched instead of assuming one.
use std::path::{Path, PathBuf};

// The build-dependency is here for its side effect (extracting the library
// before this script runs); naming it marks it as used for cargo's
// `unused_dependencies` lint.
use steamworks_sys as _;

fn main() {
    println!("cargo:rustc-link-arg-bins=-Wl,-rpath,$ORIGIN:$ORIGIN/../lib/rustpaper");
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    // the `build` directory above OUT_DIR, whose parent is target/<profile>
    let Some(build) = out
        .ancestors()
        .find(|p| p.file_name().is_some_and(|n| n == "build"))
    else {
        println!("cargo:warning=no build directory above {}", out.display());
        return;
    };
    let Some(profile_dir) = build.parent() else {
        return;
    };
    match find_lib(build) {
        Some(lib) => {
            if let Err(e) = std::fs::copy(&lib, profile_dir.join("libsteam_api.so")) {
                println!("cargo:warning=copying {}: {e}", lib.display());
            }
        }
        None => println!(
            "cargo:warning=libsteam_api.so not found under {}",
            build.display()
        ),
    }
}

/// `<build>/steamworks-sys-<hash>/out/libsteam_api.so` (older cargo) or
/// `<build>/steamworks-sys/<hash>/out/libsteam_api.so` (newer).
fn find_lib(build: &Path) -> Option<PathBuf> {
    let dirs = std::fs::read_dir(build).ok()?;
    for d in dirs.flatten() {
        if !d
            .file_name()
            .to_string_lossy()
            .starts_with("steamworks-sys")
        {
            continue;
        }
        let direct = d.path().join("out").join("libsteam_api.so");
        if direct.exists() {
            return Some(direct);
        }
        let nested = std::fs::read_dir(d.path()).ok()?.flatten().find_map(|h| {
            let p = h.path().join("out").join("libsteam_api.so");
            p.exists().then_some(p)
        });
        if nested.is_some() {
            return nested;
        }
    }
    None
}
