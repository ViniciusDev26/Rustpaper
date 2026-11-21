// rustpaper-script-worker: runs a scene's SceneScripts, sandboxed.
//
// Started by `rustpaper_script::ScriptHost` with three pipes and an empty
// environment. Before it reads a single byte of script it locks itself down
// (`sandbox::lock_down`: seccomp allowlist + rlimits), so even a script that
// broke out of QuickJS could only allocate memory and talk on its pipes.
//
// Protocol (one JSON document per line, see `protocol.rs`):
//   stdin  `Init`  -> stdout `Reply` (load errors)
//   stdin  `Frame` -> stdout `Reply` (the first frame runs init() and
//                     applyUserProperties() before the first update())

use std::cell::Cell;
use std::io::{BufRead, Write};
use std::rc::Rc;
use std::time::{Duration, Instant};

use rquickjs::loader::{BuiltinLoader, BuiltinResolver};
use rquickjs::{CatchResultExt, Context, Ctx, Function, Module, Object, Runtime};
use rustpaper_script::protocol::Init;
use rustpaper_script::sandbox;

const PRELUDE: &str = include_str!("../prelude.js");

/// JS heap limit (QuickJS's own accounting).
const HEAP_LIMIT: usize = 256 << 20;
/// Address-space rlimit of the whole worker process.
const ADDRESS_SPACE: u64 = 2 << 30;
/// Time budget of one module's top-level evaluation.
const LOAD_BUDGET: Duration = Duration::from_millis(500);
/// Time budget of one frame (every script's update()).
const FRAME_BUDGET: Duration = Duration::from_millis(40);
/// The first frame also runs init() and applyUserProperties().
const START_BUDGET: Duration = Duration::from_millis(1000);

/// WE's built-in modules re-export the prelude's globals.
const MODULES: [(&str, &str); 3] = [
    (
        "WEMath",
        "const M = globalThis.WEMath; export default M;
         export const { PI, deg2rad, rad2deg, mix, lerp, clamp, smoothStep, smoothstep, fract, sign, step } = M;",
    ),
    (
        "WEColor",
        "const M = globalThis.WEColor; export default M;
         export const { rgb2hsv, hsv2rgb, normalizeColor, expandColor } = M;",
    ),
    (
        "WEVector",
        "const M = globalThis.WEVector; export default M;
         export const { angleVector2, vectorAngle2 } = M;",
    ),
];

fn main() {
    if let Err(e) = sandbox::lock_down(ADDRESS_SPACE) {
        eprintln!("rustpaper-script-worker: sandbox: {e}");
        std::process::exit(2);
    }
    // `--self-test`: prove the jail holds (tests/sandbox.rs)
    if std::env::args().nth(1).as_deref() == Some("--self-test") {
        std::process::exit(sandbox::self_test());
    }
    if let Err(e) = run() {
        eprintln!("rustpaper-script-worker: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let rt = Runtime::new().map_err(|e| e.to_string())?;
    rt.set_memory_limit(HEAP_LIMIT);
    rt.set_max_stack_size(1 << 20);
    let deadline: Rc<Cell<Option<Instant>>> = Rc::new(Cell::new(None));
    let d = deadline.clone();
    rt.set_interrupt_handler(Some(Box::new(move || {
        d.get().is_some_and(|t| Instant::now() > t)
    })));
    let mut resolver = BuiltinResolver::default();
    let mut loader = BuiltinLoader::default();
    for (name, src) in MODULES {
        resolver.add_module(name);
        loader.add_module(name, src);
    }
    rt.set_loader(resolver, loader);
    let ctx = Context::full(&rt).map_err(|e| e.to_string())?;

    let stdin = std::io::stdin();
    let mut lines = stdin.lock().lines();
    let mut stdout = std::io::stdout().lock();
    let mut send = |s: String| -> Result<(), String> {
        stdout
            .write_all(s.as_bytes())
            .and_then(|_| stdout.write_all(b"\n"))
            .and_then(|_| stdout.flush())
            .map_err(|e| e.to_string())
    };

    let init_line = lines
        .next()
        .ok_or("no init message")?
        .map_err(|e| e.to_string())?;
    let init: Init = serde_json::from_str(&init_line).map_err(|e| e.to_string())?;

    let reply = ctx.with(|ctx| -> Result<String, String> {
        deadline.set(Some(Instant::now() + LOAD_BUDGET));
        ctx.eval::<(), _>(PRELUDE)
            .catch(&ctx)
            .map_err(|e| format!("prelude: {e}"))?;
        let rp: Object = ctx.globals().get("__rp").map_err(|e| e.to_string())?;
        let reply = call_str(&ctx, &rp, "init", &init_line)?;
        for s in &init.scripts {
            deadline.set(Some(Instant::now() + LOAD_BUDGET));
            if let Err(e) = load_module(&ctx, &rp, s.id, &s.source) {
                let f: Function = rp.get("moduleFailed").map_err(|e| e.to_string())?;
                let _ = f.call::<_, ()>((s.id, e));
            }
        }
        deadline.set(None);
        // load errors were logged after init's own reply: flush them now
        let _ = reply;
        call_str(&ctx, &rp, "frameLogs", "")
    })?;
    send(reply)?;

    let mut first = true;
    for line in lines {
        let line = line.map_err(|e| e.to_string())?;
        let (method, budget) = if first {
            ("start", START_BUDGET)
        } else {
            ("frame", FRAME_BUDGET)
        };
        first = false;
        let reply = ctx.with(|ctx| -> Result<String, String> {
            let rp: Object = ctx.globals().get("__rp").map_err(|e| e.to_string())?;
            deadline.set(Some(Instant::now() + budget));
            let r = call_str(&ctx, &rp, method, &line);
            deadline.set(None);
            match r {
                Ok(s) => Ok(s),
                // ran out of time (or memory): disable the script that was
                // running and answer with just the logs
                Err(e) => {
                    let id: i32 = rp
                        .get::<_, Function>("running")
                        .and_then(|f| f.call(()))
                        .unwrap_or(-1);
                    if let Ok(f) = rp.get::<_, Function>("disable") {
                        let _ = f.call::<_, ()>((id,));
                    }
                    if let Ok(f) = rp.get::<_, Function>("log") {
                        let _ = f.call::<_, ()>((format!("frame aborted: {e}"),));
                    }
                    call_str(&ctx, &rp, "frameLogs", "")
                }
            }
        })?;
        send(reply)?;
    }
    Ok(())
}

/// Calls `__rp[method](arg)`, which returns a JSON string.
fn call_str<'js>(
    ctx: &Ctx<'js>,
    rp: &Object<'js>,
    method: &str,
    arg: &str,
) -> Result<String, String> {
    let f: Function = rp.get(method).map_err(|e| e.to_string())?;
    f.call::<_, String>((arg,))
        .catch(ctx)
        .map_err(|e| format!("{method}: {e}"))
}

/// Compiles and evaluates one Workshop script as an ES module, then hands its
/// exports to the prelude. The bindings line goes on the module's first line
/// so the script's own line numbers stay right in error messages.
fn load_module<'js>(ctx: &Ctx<'js>, rp: &Object<'js>, id: u32, source: &str) -> Result<(), String> {
    let src = format!(
        "const {{ thisLayer, thisObject, thisScene, console, createScriptProperties }} = __rp.ctx({id}); {source}"
    );
    let m = Module::declare(ctx.clone(), format!("script_{id}"), src)
        .catch(ctx)
        .map_err(|e| e.to_string())?;
    let (m, promise) = m.eval().catch(ctx).map_err(|e| e.to_string())?;
    promise
        .finish::<()>()
        .catch(ctx)
        .map_err(|e| e.to_string())?;
    let ns = m.namespace().catch(ctx).map_err(|e| e.to_string())?;
    let f: Function = rp.get("attach").map_err(|e| e.to_string())?;
    f.call::<_, ()>((id, ns))
        .catch(ctx)
        .map_err(|e| e.to_string())
}
