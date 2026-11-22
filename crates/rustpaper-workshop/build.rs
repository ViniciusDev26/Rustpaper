// libsteam_api.so (Steamworks SDK redistributable, bundled by steamworks-sys)
// is linked dynamically. The binary looks for it next to itself (target/
// <profile>/ while developing) and in ../lib/rustpaper (/usr/bin ->
// /usr/lib/rustpaper when installed; packaging/dist.sh puts it there).
//
// steamworks-sys extracts the library into its build script's OUT_DIR; being
// a build-dependency here, that has run before this script, so the copy is
// found under target/<profile>/build/steamworks-sys-*/out.
use std::path::Path;

fn main() {
    println!("cargo:rustc-link-arg-bins=-Wl,-rpath,$ORIGIN:$ORIGIN/../lib/rustpaper");
    let out = std::env::var("OUT_DIR").expect("OUT_DIR");
    // OUT_DIR = target/<profile>/build/rustpaper-workshop-<hash>/out
    let Some(build) = Path::new(&out).ancestors().nth(2) else {
        return;
    };
    let Some(profile_dir) = build.parent() else {
        return;
    };
    let found = std::fs::read_dir(build).ok().and_then(|dirs| {
        dirs.flatten()
            .filter(|d| {
                d.file_name()
                    .to_string_lossy()
                    .starts_with("steamworks-sys-")
            })
            .map(|d| d.path().join("out").join("libsteam_api.so"))
            .find(|p| p.exists())
    });
    match found {
        Some(lib) => {
            let _ = std::fs::copy(&lib, profile_dir.join("libsteam_api.so"));
        }
        None => println!(
            "cargo:warning=libsteam_api.so not found under {}",
            build.display()
        ),
    }
}
