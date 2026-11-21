//! SceneScript (WE's JavaScript) support, always sandboxed.
//!
//! Workshop scripts are untrusted code. They never run in the renderer's
//! process: `ScriptHost` starts `rustpaper-script-worker`, which applies a
//! seccomp allowlist and rlimits to itself before it reads any script and
//! runs them in QuickJS with heap, stack and time limits. The two sides only
//! exchange plain JSON (`protocol`).

mod host;
pub mod protocol;
pub mod sandbox;

pub use host::ScriptHost;
