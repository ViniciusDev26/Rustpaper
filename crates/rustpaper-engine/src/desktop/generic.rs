//! Generic adapter: any compositor with layer-shell, no IPC of its own.
//!
//! The monitors come from the compositor's `wl_output`s (names and current
//! modes, read by mirror.rs). There is no stage here (no standard way to
//! hide windows inside a compositor) and no coverage (the Playback rules
//! never pause), so this adapter only works with the private stage
//! (stage.rs), which hosts WE itself.

use super::{Coverage, Desktop, Output, Stage};

pub struct Generic {
    outputs: Vec<Output>,
}

impl Generic {
    pub fn new(outputs: Vec<(String, (u32, u32))>) -> Self {
        let outputs = outputs
            .into_iter()
            .filter(|(name, size)| !name.is_empty() && size.0 > 0 && size.1 > 0)
            .map(|(name, size)| Output { name, size })
            .collect();
        Generic { outputs }
    }
}

impl Desktop for Generic {
    fn name(&self) -> &'static str {
        "generic (layer-shell)"
    }

    fn outputs(&self) -> Vec<Output> {
        self.outputs.clone()
    }

    fn coverage(&self) -> Option<Vec<Coverage>> {
        None
    }

    fn stage(&self) -> Option<&dyn Stage> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_named_outputs_with_a_mode() {
        let g = Generic::new(vec![
            ("DP-1".into(), (1920, 1080)),
            ("".into(), (800, 600)),
            ("HDMI-A-1".into(), (0, 0)),
        ]);
        assert_eq!(
            g.outputs(),
            vec![Output {
                name: "DP-1".into(),
                size: (1920, 1080)
            }]
        );
    }
}
