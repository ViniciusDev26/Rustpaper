// Módulo `particle`: parseia um sistema de partículas do Wallpaper Engine
// (particles/*.json) nos parâmetros que a simulação precisa. O formato tem
// campos min/max que ora são escalares, ora vec3 em string ("x y z").

use serde_json::Value;

#[derive(Debug, Clone, PartialEq)]
pub struct ParticleSystem {
    pub max_count: u32,
    pub material: String, // caminho do material (→ textura do sprite)
    // Emissão
    pub rate: f32, // partículas por segundo
    pub origin: [f32; 3],
    pub distance_min: f32,
    pub distance_max: f32,
    // Inicialização (faixas aleatórias por partícula)
    pub lifetime: (f32, f32),
    pub size: (f32, f32),
    pub velocity_min: [f32; 3],
    pub velocity_max: [f32; 3],
    // turbulentvelocityrandom: velocidade inicial de MÓDULO aleatório numa DIREÇÃO
    // aleatória (comum em bolhas/poeira). (speed_min, speed_max) — None se ausente.
    pub turbulent_speed: Option<(f32, f32)>,
    pub color_min: [f32; 3],
    pub color_max: [f32; 3],
    // Operadores
    pub gravity: [f32; 3],
    pub drag: f32,
    pub fade_in_time: f32,
    pub oscillate: Option<Oscillate>,
    pub color_change: Option<ColorChange>,
    // Metadados pro engine decidir se sabe renderizar (degradação graciosa):
    pub renderer: String,       // "sprite" | "spritetrail" | ...
    pub operators: Vec<String>, // nomes dos operadores presentes
}

// oscillateposition: desloca a partícula num seno (balanço). Faixas por partícula.
#[derive(Debug, Clone, PartialEq)]
pub struct Oscillate {
    pub mask: [f32; 3], // em quais eixos oscila
    pub frequency: (f32, f32),
    pub phase: (f32, f32),
    pub scale: (f32, f32),
}

// colorchange: interpola a cor (0..1) entre start e end, entre start_time e end_time
// (frações da vida).
#[derive(Debug, Clone, PartialEq)]
pub struct ColorChange {
    pub start_time: f32,
    pub end_time: f32,
    pub start: [f32; 3],
    pub end: [f32; 3],
}

// "x y z" -> [x, y, z]. Aceita também número puro (replica nos 3).
fn vec3(v: &Value) -> [f32; 3] {
    if let Some(s) = v.as_str() {
        let mut it = s
            .split_whitespace()
            .map(|t| t.parse::<f32>().unwrap_or(0.0));
        return [
            it.next().unwrap_or(0.0),
            it.next().unwrap_or(0.0),
            it.next().unwrap_or(0.0),
        ];
    }
    if let Some(n) = v.as_f64() {
        return [n as f32; 3];
    }
    [0.0; 3]
}

fn f32_of(v: &Value) -> f32 {
    v.as_f64().map(|n| n as f32).unwrap_or(0.0)
}

// Acha o primeiro item de um array cujo campo "name" bate.
fn find_named<'a>(arr: Option<&'a Value>, name: &str) -> Option<&'a Value> {
    arr?.as_array()?
        .iter()
        .find(|it| it.get("name").and_then(|n| n.as_str()) == Some(name))
}

impl ParticleSystem {
    pub fn parse(json: &str) -> Result<ParticleSystem, serde_json::Error> {
        let root: Value = serde_json::from_str(json)?;

        let max_count = root.get("maxcount").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
        let material = root
            .get("material")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        // Emitter (usamos o primeiro).
        let emitter = root
            .get("emitter")
            .and_then(|e| e.as_array())
            .and_then(|a| a.first());
        let rate = emitter
            .and_then(|e| e.get("rate"))
            .map(f32_of)
            .unwrap_or(0.0);
        let origin = emitter
            .and_then(|e| e.get("origin"))
            .map(vec3)
            .unwrap_or([0.0; 3]);
        let distance_min = emitter
            .and_then(|e| e.get("distancemin"))
            .map(f32_of)
            .unwrap_or(0.0);
        let distance_max = emitter
            .and_then(|e| e.get("distancemax"))
            .map(f32_of)
            .unwrap_or(0.0);

        // Initializers (por nome).
        let init = root.get("initializer");
        let (lt_min, lt_max) = min_max_scalar(find_named(init, "lifetimerandom"));
        let (sz_min, sz_max) = min_max_scalar(find_named(init, "sizerandom"));
        let (vel_min, vel_max) = min_max_vec3(find_named(init, "velocityrandom"));
        let (col_min, col_max) = min_max_vec3(find_named(init, "colorrandom"));
        let turbulent_speed = find_named(init, "turbulentvelocityrandom").map(|t| {
            let smin = t.get("speedmin").map(f32_of).unwrap_or(0.0);
            let smax = t.get("speedmax").map(f32_of).unwrap_or(smin);
            (smin, smax)
        });

        // Operators.
        let op = root.get("operator");
        let movement = find_named(op, "movement");
        let gravity = movement
            .and_then(|m| m.get("gravity"))
            .map(vec3)
            .unwrap_or([0.0; 3]);
        let drag = movement
            .and_then(|m| m.get("drag"))
            .map(f32_of)
            .unwrap_or(0.0);
        let fade_in_time = find_named(op, "alphafade")
            .and_then(|a| a.get("fadeintime"))
            .map(f32_of)
            .unwrap_or(0.0);

        let oscillate = find_named(op, "oscillateposition").map(|o| Oscillate {
            mask: o.get("mask").map(vec3).unwrap_or([1.0, 1.0, 1.0]),
            frequency: (
                o.get("frequencymin").map(f32_of).unwrap_or(1.0),
                o.get("frequencymax").map(f32_of).unwrap_or(1.0),
            ),
            phase: (
                o.get("phasemin").map(f32_of).unwrap_or(0.0),
                o.get("phasemax").map(f32_of).unwrap_or(0.0),
            ),
            scale: (
                o.get("scalemin").map(f32_of).unwrap_or(0.0),
                o.get("scalemax").map(f32_of).unwrap_or(0.0),
            ),
        });

        let color_change = find_named(op, "colorchange").map(|c| ColorChange {
            start_time: c.get("starttime").map(f32_of).unwrap_or(0.0),
            end_time: c.get("endtime").map(f32_of).unwrap_or(1.0),
            start: c.get("startvalue").map(vec3).unwrap_or([1.0; 3]),
            end: c.get("endvalue").map(vec3).unwrap_or([1.0; 3]),
        });

        // Renderer (primeiro) + nomes dos operadores.
        let renderer = root
            .get("renderer")
            .and_then(|r| r.as_array())
            .and_then(|a| a.first())
            .and_then(|r| r.get("name"))
            .and_then(|n| n.as_str())
            .unwrap_or("")
            .to_string();
        let operators = op
            .and_then(|o| o.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|it| it.get("name").and_then(|n| n.as_str()).map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        Ok(ParticleSystem {
            max_count,
            material,
            rate,
            origin,
            distance_min,
            distance_max,
            lifetime: (lt_min, lt_max),
            size: (sz_min, sz_max),
            velocity_min: vel_min,
            velocity_max: vel_max,
            turbulent_speed,
            color_min: col_min,
            color_max: col_max,
            gravity,
            drag,
            fade_in_time,
            oscillate,
            color_change,
            renderer,
            operators,
        })
    }
}

fn min_max_scalar(item: Option<&Value>) -> (f32, f32) {
    match item {
        Some(it) => (
            it.get("min").map(f32_of).unwrap_or(0.0),
            it.get("max").map(f32_of).unwrap_or(0.0),
        ),
        None => (0.0, 0.0),
    }
}

fn min_max_vec3(item: Option<&Value>) -> ([f32; 3], [f32; 3]) {
    match item {
        Some(it) => (
            it.get("min").map(vec3).unwrap_or([0.0; 3]),
            it.get("max").map(vec3).unwrap_or([0.0; 3]),
        ),
        None => ([0.0; 3], [0.0; 3]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Fixture baseada na neve real do Wallpaper Engine (snowparticles.json).
    const SNOW: &str = r#"{
        "maxcount": 300,
        "material": "materials/presets/snowparticles.json",
        "emitter": [{ "name": "sphererandom", "rate": 10, "origin": "0 650 0",
                      "distancemin": 10, "distancemax": 1024 }],
        "initializer": [
            { "name": "lifetimerandom", "min": 8, "max": 20 },
            { "name": "sizerandom", "min": 2, "max": 30 },
            { "name": "velocityrandom", "min": "-10 -50 0", "max": "-37 -90 0" },
            { "name": "colorrandom", "min": "90 92 95", "max": "0 0 0" }
        ],
        "operator": [
            { "name": "movement", "gravity": "0 0 0" },
            { "name": "oscillateposition", "mask": "1 0.5 0",
              "frequencymin": 0.8, "frequencymax": 1, "phasemin": 0, "phasemax": 1,
              "scalemin": 20, "scalemax": 35 },
            { "name": "alphafade", "fadeintime": 0.1 }
        ],
        "renderer": [{ "name": "sprite" }]
    }"#;

    #[test]
    fn parses_snow() {
        let p = ParticleSystem::parse(SNOW).unwrap();
        assert_eq!(p.max_count, 300);
        assert_eq!(p.material, "materials/presets/snowparticles.json");
        assert_eq!(p.rate, 10.0);
        assert_eq!(p.origin, [0.0, 650.0, 0.0]);
        assert_eq!(p.lifetime, (8.0, 20.0));
        assert_eq!(p.size, (2.0, 30.0));
        assert_eq!(p.velocity_min, [-10.0, -50.0, 0.0]);
        assert_eq!(p.velocity_max, [-37.0, -90.0, 0.0]);
        assert_eq!(p.fade_in_time, 0.1);
        assert_eq!(p.renderer, "sprite");
        assert_eq!(
            p.operators,
            vec!["movement", "oscillateposition", "alphafade"]
        );
        let osc = p.oscillate.expect("deveria ter oscillate");
        assert_eq!(osc.mask, [1.0, 0.5, 0.0]);
        assert_eq!(osc.scale, (20.0, 35.0));
    }

    #[test]
    fn missing_fields_default() {
        let p = ParticleSystem::parse("{}").unwrap();
        assert_eq!(p.max_count, 0);
        assert_eq!(p.rate, 0.0);
        assert_eq!(p.lifetime, (0.0, 0.0));
    }

    #[test]
    fn vec3_parsing() {
        assert_eq!(vec3(&serde_json::json!("1 2 3")), [1.0, 2.0, 3.0]);
        assert_eq!(vec3(&serde_json::json!("0.5 -1 0")), [0.5, -1.0, 0.0]);
        assert_eq!(vec3(&serde_json::json!(5.0)), [5.0, 5.0, 5.0]);
    }
}
