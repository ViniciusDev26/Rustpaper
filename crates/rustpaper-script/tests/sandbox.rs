// The sandbox, end to end: the real worker binary, the real seccomp filter.

use std::path::Path;
use std::process::Command;

use rustpaper_script::ScriptHost;
use rustpaper_script::protocol::{Binding, Frame, Init, LayerState, ScriptDef, ValueKind};
use serde_json::{Map, json};

const WORKER: &str = env!("CARGO_BIN_EXE_rustpaper-script-worker");

fn layer(id: i64) -> LayerState {
    LayerState {
        id,
        name: format!("layer{id}"),
        kind: "image".into(),
        scale: [1.0; 3],
        color: [1.0; 3],
        alpha: 1.0,
        brightness: 1.0,
        visible: true,
        ..Default::default()
    }
}

fn script(
    id: u32,
    property: &str,
    kind: ValueKind,
    value: serde_json::Value,
    source: &str,
) -> ScriptDef {
    ScriptDef {
        id,
        layer: 1,
        property: property.into(),
        binding: Binding::Layer,
        kind,
        value,
        script_properties: Map::new(),
        source: source.into(),
    }
}

fn init(scripts: Vec<ScriptDef>) -> Init {
    Init {
        scripts,
        layers: vec![layer(1)],
        user_properties: Map::new(),
        canvas: [1920.0, 1080.0],
        screen: [1920.0, 1080.0],
    }
}

fn frame(time: f64) -> Frame {
    Frame {
        time,
        dt: 1.0 / 30.0,
        ..Default::default()
    }
}

#[test]
fn every_escape_attempt_is_denied() {
    let out = Command::new(WORKER).arg("--self-test").output().unwrap();
    let report = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "sandbox self-test failed:\n{report}");
    assert!(
        report.contains("open: denied") && report.contains("execve: denied"),
        "{report}"
    );
}

#[test]
fn the_worker_runs_under_seccomp() {
    let (host, _) = ScriptHost::spawn_worker(Path::new(WORKER), &init(vec![]), 0).unwrap();
    let status = std::fs::read_to_string(format!("/proc/{}/status", host.pid())).unwrap();
    assert!(status.contains("Seccomp:\t2"), "{status}");
    assert!(status.contains("NoNewPrivs:\t1"), "{status}");
}

#[test]
fn scripts_update_layers_and_a_runaway_one_is_cut() {
    let scripts = vec![
        script(
            0,
            "origin",
            ValueKind::Vec3,
            json!([0, 0, 0]),
            "import { mix } from 'WEMath'; export function update(v) { return new Vec3(mix(0, 10, 0.5), engine.runtime, 0); }",
        ),
        script(
            1,
            "alpha",
            ValueKind::Float,
            json!(1),
            "export function update() { for (;;) {} }",
        ),
        script(
            2,
            "visible",
            ValueKind::Bool,
            json!(true),
            "export function update() { return typeof require === 'undefined' && typeof std === 'undefined' && typeof fetch === 'undefined'; }",
        ),
    ];
    let (mut host, _) = ScriptHost::spawn_worker(Path::new(WORKER), &init(scripts), 0).unwrap();
    let r = host.frame(&frame(0.0)).unwrap();
    assert!(
        r.logs
            .iter()
            .any(|l| l.contains("exceeded the time budget")),
        "{:?}",
        r.logs
    );
    // the worker survives; the other scripts keep running
    let r = host.frame(&frame(2.0)).unwrap();
    let up = r.layers.iter().find(|u| u.id == 1).expect("layer update");
    assert_eq!(up.origin, Some([5.0, 2.0, 0.0]));
    // nothing from the host environment exists in the script's world
    assert_ne!(up.visible, Some(false));
}

#[test]
fn imports_outside_the_builtins_are_refused() {
    let scripts = vec![script(
        0,
        "alpha",
        ValueKind::Float,
        json!(1),
        "import x from '/etc/passwd'; export function update() { return 0; }",
    )];
    let (_, r) = ScriptHost::spawn_worker(Path::new(WORKER), &init(scripts), 0).unwrap();
    assert!(r.logs.iter().any(|l| l.contains("load")), "{:?}", r.logs);
}

#[test]
fn sound_play_and_stop_reach_the_host() {
    let mut s = script(
        0,
        "visible",
        ValueKind::Bool,
        json!(true),
        "export function init() { thisLayer.stop(); } export function update() { if (engine.runtime > 1) thisLayer.play(); }",
    );
    s.binding = Binding::Layer;
    let (mut host, _) = ScriptHost::spawn_worker(Path::new(WORKER), &init(vec![s]), 0).unwrap();
    let r = host.frame(&frame(0.0)).unwrap();
    assert_eq!(
        r.layers.iter().find(|u| u.id == 1).and_then(|u| u.playing),
        Some(false)
    );
    let r = host.frame(&frame(2.0)).unwrap();
    assert_eq!(
        r.layers.iter().find(|u| u.id == 1).and_then(|u| u.playing),
        Some(true)
    );
}

#[test]
fn scripts_see_angles_in_degrees() {
    let s = script(
        0,
        "angles",
        ValueKind::Vec3,
        json!([0, 0, 0]),
        "export function update(v) { return new Vec3(0, 0, v.z + 90); }",
    );
    let mut l = layer(1);
    l.angles = [0.0, 0.0, std::f32::consts::FRAC_PI_2];
    let mut i = init(vec![s]);
    i.layers = vec![l];
    let (mut host, _) = ScriptHost::spawn_worker(Path::new(WORKER), &i, 0).unwrap();
    let r = host.frame(&frame(0.0)).unwrap();
    let z = r.layers[0].angles.unwrap()[2];
    assert!((z - std::f32::consts::PI).abs() < 1e-5, "{z}");
}
