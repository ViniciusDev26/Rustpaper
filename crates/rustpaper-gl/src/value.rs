// Helpers for the loosely-typed values in scene.json.
//
// Almost any scene property can be either a plain value or a wrapper:
//   "alpha": 0.5
//   "visible": { "user": "clock", "value": true }      (bound to a user property)
//   "tinthigh": { "user": "centercolor", "value": "0.2 0.4 0.4" }
//   "visible": { "script": "...", "value": true }      (driven by a SceneScript)
// Until user properties / scripts are supported we always take `value`, which is
// what WE shows with the default settings.

use serde_json::Value;

/// Unwraps `{ "value": x, ... }` wrappers (recursively) into `x`.
pub fn plain(v: &Value) -> &Value {
    match v {
        Value::Object(m) => match m.get("value") {
            Some(inner) => plain(inner),
            None => v,
        },
        _ => v,
    }
}

/// Number / bool / "1 2 3" string -> floats.
pub fn floats(v: &Value) -> Vec<f32> {
    match plain(v) {
        Value::Number(n) => vec![n.as_f64().unwrap_or(0.0) as f32],
        Value::Bool(b) => vec![if *b { 1.0 } else { 0.0 }],
        Value::String(s) => s
            .split_whitespace()
            .filter_map(|t| t.parse().ok())
            .collect(),
        _ => Vec::new(),
    }
}

pub fn get_floats(obj: &Value, key: &str) -> Option<Vec<f32>> {
    obj.get(key).map(floats).filter(|f| !f.is_empty())
}

pub fn get_vec3(obj: &Value, key: &str, default: [f32; 3]) -> [f32; 3] {
    match get_floats(obj, key) {
        Some(f) if f.len() >= 3 => [f[0], f[1], f[2]],
        _ => default,
    }
}

pub fn get_vec2(obj: &Value, key: &str, default: [f32; 2]) -> [f32; 2] {
    match get_floats(obj, key) {
        Some(f) if f.len() >= 2 => [f[0], f[1]],
        _ => default,
    }
}

pub fn get_f32(obj: &Value, key: &str, default: f32) -> f32 {
    get_floats(obj, key).map(|f| f[0]).unwrap_or(default)
}

/// `visible`-style flags: missing means true.
pub fn get_bool(obj: &Value, key: &str, default: bool) -> bool {
    match obj.get(key).map(plain) {
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().unwrap_or(0.0) != 0.0,
        _ => default,
    }
}

/// Replaces every `{ "user": "name", "value": x }` wrapper whose property exists
/// in `props` (project.json `general.properties`) with that property's value.
/// After this pass, `plain()` returns what the user configured (or the default).
pub fn apply_user_properties(v: &mut Value, props: &serde_json::Map<String, Value>) {
    match v {
        Value::Object(m) => {
            if let Some(Value::String(name)) = m.get("user")
                && let Some(p) = props.get(name).and_then(|p| p.get("value"))
                && m.contains_key("value")
            {
                m.insert("value".into(), p.clone());
            }
            // {"user": {"name": "size", "condition": "1"}, "value": ...}:
            // true when the property's value equals the condition (combo
            // options showing/hiding objects)
            if let Some(Value::Object(u)) = m.get("user")
                && let (Some(Value::String(name)), Some(cond)) = (u.get("name"), u.get("condition"))
                && let Some(p) = props.get(name).and_then(|p| p.get("value"))
                && m.contains_key("value")
            {
                let as_text = |v: &Value| match v {
                    Value::String(s) => s.clone(),
                    Value::Bool(b) => (*b as i32).to_string(),
                    other => other.to_string(),
                };
                let eq = as_text(p) == as_text(cond)
                    || p.as_f64()
                        .zip(as_text(cond).parse::<f64>().ok())
                        .is_some_and(|(a, b)| a == b);
                m.insert("value".into(), Value::Bool(eq));
            }
            for (_, child) in m.iter_mut() {
                apply_user_properties(child, props);
            }
        }
        Value::Array(a) => a.iter_mut().for_each(|c| apply_user_properties(c, props)),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn unwraps_user_and_script_wrappers() {
        let o = json!({
            "a": 0.5,
            "b": { "user": "x", "value": "1 2 3" },
            "v": { "script": "...", "value": false },
            "n": { "user": "y", "value": { "user": "z", "value": 7 } }
        });
        assert_eq!(get_f32(&o, "a", 0.0), 0.5);
        assert_eq!(get_vec3(&o, "b", [0.0; 3]), [1.0, 2.0, 3.0]);
        assert!(!get_bool(&o, "v", true));
        assert!(get_bool(&o, "missing", true));
        assert_eq!(get_f32(&o, "n", 0.0), 7.0);
    }

    #[test]
    fn user_properties_override_scene_values() {
        let mut scene = json!({ "visible": { "user": "blood", "value": true }, "x": 1 });
        let props = json!({ "blood": { "type": "bool", "value": false } });
        apply_user_properties(&mut scene, props.as_object().unwrap());
        assert!(!get_bool(&scene, "visible", true));
    }
}
