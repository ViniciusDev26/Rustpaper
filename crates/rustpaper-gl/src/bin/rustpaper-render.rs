// rustpaper-render: renders a WE scene offscreen to PNG (one file per time).
//
//   rustpaper-render <workshop id | folder> [--time 0,1.5,3] [--out shot.png]
//                    [--pointer 0.5,0.5] [--describe]
//
// With several times, files are named <out stem>_t<time>.png. `--describe` prints
// the layer list and every load/compile warning — the first thing to look at
// when a scene renders wrong.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use rustpaper_gl::context::GlContext;
use rustpaper_gl::scene::{FrameInput, SceneRenderer};

struct Args {
    wallpaper: PathBuf,
    times: Vec<f32>,
    out: PathBuf,
    pointer: [f32; 2],
    describe: bool,
    dump_passes: Option<PathBuf>,
    video: Option<PathBuf>,
    duration: f32,
    fps: u32,
    width: Option<u32>,
    start: f32,
    only: Option<Vec<i64>>,
}

fn parse_args() -> Result<Args, String> {
    let mut it = std::env::args().skip(1);
    let mut wallpaper = None;
    let mut times = vec![0.0];
    let mut out = PathBuf::from("render.png");
    let mut pointer = [0.5, 0.5];
    let mut describe = false;
    let mut dump_passes = None;
    let mut video = None;
    let mut duration = 10.0;
    let mut fps = 30;
    let mut width = None;
    let mut start = 0.0;
    let mut only = None;
    let floats = |s: String| -> Result<Vec<f32>, String> {
        s.split(',')
            .map(|t| t.trim().parse::<f32>().map_err(|e| format!("{t:?}: {e}")))
            .collect()
    };
    while let Some(a) = it.next() {
        match a.as_str() {
            "--time" => times = floats(it.next().ok_or("--time needs a value")?)?,
            "--out" => out = it.next().ok_or("--out needs a value")?.into(),
            "--pointer" => {
                let p = floats(it.next().ok_or("--pointer needs x,y")?)?;
                pointer = [p[0], *p.get(1).unwrap_or(&p[0])];
            }
            "--describe" => describe = true,
            "--video" => video = Some(PathBuf::from(it.next().ok_or("--video needs a file")?)),
            "--duration" => duration = floats(it.next().ok_or("--duration needs seconds")?)?[0],
            "--fps" => fps = floats(it.next().ok_or("--fps needs a value")?)?[0] as u32,
            "--only" => {
                only = Some(
                    floats(it.next().ok_or("--only needs object ids")?)?
                        .iter()
                        .map(|v| *v as i64)
                        .collect(),
                )
            }
            "--start" => start = floats(it.next().ok_or("--start needs seconds")?)?[0],
            "--width" => width = Some(floats(it.next().ok_or("--width needs pixels")?)?[0] as u32),
            "--dump-passes" => {
                dump_passes = Some(PathBuf::from(
                    it.next().ok_or("--dump-passes needs a folder")?,
                ))
            }
            "-h" | "--help" => return Err(String::new()),
            other if wallpaper.is_none() => wallpaper = Some(other.to_string()),
            other => return Err(format!("unexpected argument {other:?}")),
        }
    }
    let w = wallpaper.ok_or("missing wallpaper id or folder")?;
    let path = PathBuf::from(&w);
    let wallpaper = if path.is_dir() {
        path
    } else {
        rustpaper_core::paths::find_wallpaper(&w).ok_or_else(|| {
            format!(
                "wallpaper {w} not found in {:?}",
                rustpaper_core::paths::wallpaper_dirs()
            )
        })?
    };
    Ok(Args {
        wallpaper,
        times,
        out,
        pointer,
        describe,
        dump_passes,
        video,
        duration,
        fps,
        width,
        start,
        only,
    })
}

fn out_path(base: &Path, time: f32, many: bool) -> PathBuf {
    if !many {
        return base.to_path_buf();
    }
    let stem = base
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("render");
    base.with_file_name(format!("{stem}_t{time}.png"))
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            if !e.is_empty() {
                eprintln!("error: {e}");
            }
            eprintln!(
                "usage: rustpaper-render <workshop id | folder> [--time 0,1.5] [--out x.png] [--pointer x,y] [--describe] [--dump-passes dir] [--only id,id]\n       [--video out.mp4 --duration 10 --fps 30 --start 0] [--width 1920]"
            );
            return ExitCode::from(2);
        }
    };
    let Some(base) = rustpaper_core::paths::assets_dir() else {
        eprintln!("error: WE base assets not found (set WE_ASSETS_DIR)");
        return ExitCode::FAILURE;
    };
    let ctx = match GlContext::new_headless() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: no OpenGL: {e}");
            return ExitCode::FAILURE;
        }
    };
    let gl = &ctx.gl;
    let t0 = std::time::Instant::now();
    let mut scene = match SceneRenderer::load(gl, &args.wallpaper, &base) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: {}: {e}", args.wallpaper.display());
            return ExitCode::FAILURE;
        }
    };
    let load_ms = t0.elapsed().as_millis();
    if args.describe {
        println!("GL: {}", ctx.describe());
        println!(
            "scene {}x{} loaded in {load_ms} ms",
            scene.width, scene.height
        );
        for l in scene.describe() {
            println!("  {l}");
        }
    }

    if let Some(ids) = &args.only {
        scene.only = Some(ids.iter().copied().collect());
    }
    if let Some(d) = &args.dump_passes {
        let _ = std::fs::create_dir_all(d);
        scene.dump_dir = Some(d.clone());
    }
    if let Some(v) = &args.video {
        return match record_video(gl, &mut scene, &args, v) {
            Ok(()) => {
                for w in &scene.warnings {
                    eprintln!("warning: {w}");
                }
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("error: video: {e}");
                ExitCode::FAILURE
            }
        };
    }
    for &time in &args.times {
        let t = std::time::Instant::now();
        scene.render(
            gl,
            FrameInput {
                time,
                pointer: args.pointer,
                ..Default::default()
            },
        );
        let px = scene.output().read_rgba(gl);
        let path = out_path(&args.out, time, args.times.len() > 1);
        if let Err(e) = image::save_buffer(
            &path,
            &px,
            scene.width,
            scene.height,
            image::ExtendedColorType::Rgba8,
        ) {
            eprintln!("error: writing {}: {e}", path.display());
            return ExitCode::FAILURE;
        }
        if args.describe {
            println!(
                "t={time}: {} ({} ms)",
                path.display(),
                t.elapsed().as_millis()
            );
        }
    }
    for w in &scene.warnings {
        eprintln!("warning: {w}");
    }
    ExitCode::SUCCESS
}

/// Renders `duration` seconds at `fps` and pipes raw RGBA frames into ffmpeg.
/// Times are exact (frame n is t = n / fps), so two runs give the same video.
fn record_video(
    gl: &glow::Context,
    scene: &mut SceneRenderer,
    args: &Args,
    out: &Path,
) -> Result<(), String> {
    use std::io::Write;
    let (w, h) = (scene.width, scene.height);
    let scale = match args.width {
        Some(tw) => format!("scale={tw}:-2:flags=lanczos,"),
        None => String::new(),
    };
    let mut ff = std::process::Command::new("ffmpeg")
        .args([
            "-loglevel",
            "error",
            "-y",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgba",
        ])
        .args([
            "-s",
            &format!("{w}x{h}"),
            "-r",
            &args.fps.to_string(),
            "-i",
            "-",
        ])
        .args(["-vf", &format!("{scale}format=yuv420p")])
        .args(["-c:v", "libx264", "-crf", "16", "-preset", "fast"])
        .arg(out)
        .stdin(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("starting ffmpeg: {e}"))?;
    let mut stdin = ff.stdin.take().ok_or("no ffmpeg stdin")?;
    let frames = (args.duration * args.fps as f32).round() as u32;
    let t0 = std::time::Instant::now();
    for n in 0..frames {
        scene.render(
            gl,
            FrameInput {
                time: args.start + n as f32 / args.fps as f32,
                pointer: args.pointer,
                ..Default::default()
            },
        );
        let px = scene.output().read_rgba(gl);
        stdin
            .write_all(&px)
            .map_err(|e| format!("writing frame {n}: {e}"))?;
    }
    drop(stdin);
    let status = ff.wait().map_err(|e| e.to_string())?;
    if !status.success() {
        return Err(format!("ffmpeg exited with {status}"));
    }
    if args.describe {
        let secs = t0.elapsed().as_secs_f32();
        println!(
            "{frames} frames in {secs:.1}s ({:.0} fps incl. readback+encode) -> {}",
            frames as f32 / secs,
            out.display()
        );
    }
    Ok(())
}
