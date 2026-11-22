//! Whether the computer runs on battery (the "Laptop on battery" Playback
//! setting).

/// True when the machine has a mains adapter (`type` = Mains in
/// /sys/class/power_supply) and none of them is online. Desktops (no
/// adapter listed) are never on battery.
pub fn on_battery() -> bool {
    let Ok(rd) = std::fs::read_dir("/sys/class/power_supply") else {
        return false;
    };
    let mut mains = rd.flatten().filter(|e| {
        std::fs::read_to_string(e.path().join("type")).is_ok_and(|t| t.trim() == "Mains")
    });
    let mut any = false;
    let online = mains.any(|e| {
        any = true;
        std::fs::read_to_string(e.path().join("online")).is_ok_and(|o| o.trim() == "1")
    });
    any && !online
}
