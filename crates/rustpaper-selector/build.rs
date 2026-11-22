// Builds the React UI (ui/, Vite) before tauri-build: the Tauri context
// embeds the frontend into the binary at compile time, so a plain
// `cargo build` must produce it first.
//
// The bundle goes to $OUT_DIR/ui (a build output, not ui/dist): watching a
// folder this script writes would make cargo rerun it on every build.
// TAURI_CONFIG (merged over tauri.conf.json by tauri-build and by
// generate_context!) points frontendDist there.
//
// Needs Node.js + npm. `npm ci` runs only when ui/node_modules is missing
// (it needs the network: packaging runs it in prepare()).
// RUSTPAPER_UI_DIST=<dir> uses an already built bundle instead.
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    for f in [
        "src",
        "index.html",
        "package.json",
        "package-lock.json",
        "vite.config.ts",
        "tsconfig.json",
    ] {
        println!("cargo:rerun-if-changed=ui/{f}");
    }
    println!("cargo:rerun-if-env-changed=RUSTPAPER_UI_DIST");

    let dist = match std::env::var_os("RUSTPAPER_UI_DIST") {
        Some(d) => PathBuf::from(d),
        None => {
            let out = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR")).join("ui");
            let ui = Path::new("ui");
            if !ui.join("node_modules").exists() {
                npm(ui, &["ci", "--no-audit", "--no-fund"]);
            }
            npm(ui, &["run", "typecheck"]);
            let out_arg = out.to_str().expect("OUT_DIR is not UTF-8");
            npm(
                ui,
                &[
                    "exec",
                    "--",
                    "vite",
                    "build",
                    "--emptyOutDir",
                    "--outDir",
                    out_arg,
                ],
            );
            out
        }
    };
    assert!(
        dist.join("index.html").exists(),
        "no index.html in the UI bundle {}",
        dist.display()
    );
    let config = serde_json::json!({ "build": { "frontendDist": dist } }).to_string();
    // tauri-build reads it from this process, generate_context! from rustc's
    // environment
    // SAFETY: build scripts are single-threaded; nothing reads the
    // environment concurrently.
    unsafe { std::env::set_var("TAURI_CONFIG", &config) };
    println!("cargo:rustc-env=TAURI_CONFIG={config}");
    tauri_build::build()
}

fn npm(dir: &Path, args: &[&str]) {
    let status = Command::new("npm").args(args).current_dir(dir).status();
    match status {
        Ok(s) if s.success() => {}
        Ok(s) => panic!("`npm {}` in ui/ failed ({s})", args.join(" ")),
        Err(e) => panic!(
            "the selector's UI needs Node.js + npm to build (`npm {}`): {e}",
            args.join(" ")
        ),
    }
}
