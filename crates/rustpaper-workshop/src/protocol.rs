// JSON-lines protocol between the selector and `rustpaper-workshop`, and the
// pure helpers that map Steam Workshop data to it (unit-tested; the Steam
// calls themselves live in main.rs).
//
// selector -> helper, one request per line:
//   {"id": 1, "cmd": "query", "sort": "trend", "kind": "scene", "search": "", "page": 1, "mature": false}
//   {"id": 2, "cmd": "subscribe", "item": "123"}      also "unsubscribe"
//   {"id": 3, "cmd": "status", "items": ["123", ...]}
//   {"id": 4, "cmd": "subscriptions"}   -> {"123": <unix time subscribed>, ...}
// helper -> selector, one line per request, matched by `id`:
//   {"id": 1, "ok": true, "result": ...} | {"id": 1, "ok": false, "error": "..."}
// plus a first line {"ready": true} | {"ready": false, "error": "..."} once
// the Steam API is up (or not).

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Deserialize, Debug, PartialEq)]
pub struct Request {
    pub id: u64,
    #[serde(flatten)]
    pub cmd: Command,
}

#[derive(Deserialize, Debug, PartialEq)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Command {
    Query(Query),
    Subscribe { item: String },
    Unsubscribe { item: String },
    Status { items: Vec<String> },
    Subscriptions,
}

#[derive(Deserialize, Debug, PartialEq, Clone)]
pub struct Query {
    /// "trend" (this week), "top" (all time), "recent", "updated",
    /// "subscribed" (the user's own subscriptions)
    #[serde(default = "default_sort")]
    pub sort: String,
    /// "scene" | "video" | "web" | "all"
    #[serde(default = "default_kind")]
    pub kind: String,
    #[serde(default)]
    pub search: String,
    /// 1-based, 50 items per page (Steam's page size)
    #[serde(default = "default_page")]
    pub page: u32,
    /// include items rated "Questionable" / "Mature"
    #[serde(default)]
    pub mature: bool,
}

fn default_sort() -> String {
    "trend".into()
}
fn default_kind() -> String {
    "all".into()
}
fn default_page() -> u32 {
    1
}

/// Wallpaper types the gallery knows. WE also lists "Application" items
/// (executables): never offered.
pub const KINDS: [&str; 3] = ["Scene", "Video", "Web"];
/// WE's age-rating tags; everything but "Everyone" is hidden unless asked.
pub const MATURE_TAGS: [&str; 2] = ["Questionable", "Mature"];

/// Workshop tags to require for a kind filter: one tag, or any of the
/// supported kinds for "all".
pub fn required_kind_tags(kind: &str) -> Vec<&'static str> {
    match KINDS.iter().find(|k| k.eq_ignore_ascii_case(kind)) {
        Some(k) => vec![*k],
        None => KINDS.to_vec(),
    }
}

/// The wallpaper type among an item's tags, lowercase like project.json
/// ("scene"), or "" when it has none of the known ones.
pub fn kind_of(tags: &[String]) -> String {
    tags.iter()
        .find_map(|t| KINDS.iter().find(|k| k.eq_ignore_ascii_case(t.trim())))
        .map(|k| k.to_ascii_lowercase())
        .unwrap_or_default()
}

/// "everyone" | "questionable" | "mature" from the tags.
pub fn rating_of(tags: &[String]) -> String {
    let has = |name: &str| tags.iter().any(|t| t.trim().eq_ignore_ascii_case(name));
    if has("Mature") {
        "mature".into()
    } else if has("Questionable") {
        "questionable".into()
    } else {
        "everyone".into()
    }
}

/// One Workshop item as the selector shows it.
#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct Item {
    pub id: String,
    pub title: String,
    pub preview_url: String,
    /// "scene" | "video" | "web"
    pub kind: String,
    /// "everyone" | "questionable" | "mature"
    pub rating: String,
    pub tags: Vec<String>,
    pub subscriptions: u64,
    pub votes_up: u32,
    pub votes_down: u32,
    pub file_size: u32,
    pub time_updated: u32,
    pub state: State,
}

/// Local state of an item (ISteamUGC::GetItemState + download info).
#[derive(Serialize, Debug, Clone, Default, PartialEq)]
pub struct State {
    pub subscribed: bool,
    pub installed: bool,
    pub downloading: bool,
    pub needs_update: bool,
    /// bytes downloaded / total while downloading
    pub downloaded: u64,
    pub total: u64,
}

pub fn ok(id: u64, result: Value) -> String {
    serde_json::json!({ "id": id, "ok": true, "result": result }).to_string()
}

pub fn err(id: u64, error: impl std::fmt::Display) -> String {
    serde_json::json!({ "id": id, "ok": false, "error": error.to_string() }).to_string()
}

/// Workshop ids are decimal u64s; anything else is refused before it
/// reaches Steam.
pub fn parse_item(item: &str) -> Result<u64, String> {
    item.trim()
        .parse::<u64>()
        .ok()
        .filter(|&v| v > 0)
        .ok_or_else(|| format!("invalid workshop id: {item:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_parse_with_defaults() {
        let r: Request = serde_json::from_str(r#"{"id":4,"cmd":"query"}"#).unwrap();
        assert_eq!(
            r,
            Request {
                id: 4,
                cmd: Command::Query(Query {
                    sort: "trend".into(),
                    kind: "all".into(),
                    search: String::new(),
                    page: 1,
                    mature: false,
                })
            }
        );
        let r: Request =
            serde_json::from_str(r#"{"id":5,"cmd":"subscribe","item":"123"}"#).unwrap();
        assert_eq!(r.cmd, Command::Subscribe { item: "123".into() });
        assert!(serde_json::from_str::<Request>(r#"{"id":6,"cmd":"rm -rf"}"#).is_err());
        let r: Request = serde_json::from_str(r#"{"id":7,"cmd":"subscriptions"}"#).unwrap();
        assert_eq!(r.cmd, Command::Subscriptions);
    }

    #[test]
    fn kind_filters_never_include_applications() {
        assert_eq!(required_kind_tags("scene"), vec!["Scene"]);
        assert_eq!(required_kind_tags("all"), vec!["Scene", "Video", "Web"]);
        assert_eq!(
            required_kind_tags("application"),
            vec!["Scene", "Video", "Web"]
        );
    }

    #[test]
    fn kind_and_rating_from_tags() {
        let tags = |t: &[&str]| t.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(kind_of(&tags(&["Anime", "Scene", "Everyone"])), "scene");
        assert_eq!(kind_of(&tags(&["Anime"])), "");
        assert_eq!(rating_of(&tags(&["Video", "Mature"])), "mature");
        assert_eq!(rating_of(&tags(&["Video", "Questionable"])), "questionable");
        assert_eq!(rating_of(&tags(&["Video"])), "everyone");
    }

    #[test]
    fn workshop_ids_are_plain_numbers() {
        assert_eq!(parse_item("3028926404"), Ok(3028926404));
        assert!(parse_item("0").is_err());
        assert!(parse_item("../etc").is_err());
        assert!(parse_item("12a").is_err());
    }
}
