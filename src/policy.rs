use crate::model::{PaneId, PrototypePolicyStatus};
use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum PrototypePolicyError {
    #[error(
        "prototype overlay policy allows only one active overlay pane (active: {active_overlay_pane:?})"
    )]
    OverlaySlotInUse { active_overlay_pane: PaneId },
}

#[derive(Debug, Default, Clone)]
pub struct PrototypeOverlayPolicy {
    // This encodes only the temporary v1 prototype policy (single overlay layer).
    // It is intentionally separate from pane mode/state so we do not hard-code it as
    // the long-term pane-hosting contract.
    active_overlay_pane: Option<PaneId>,
}

impl PrototypeOverlayPolicy {
    pub fn active_overlay_pane(&self) -> Option<&PaneId> {
        self.active_overlay_pane.as_ref()
    }

    pub fn reserve_for(&mut self, pane_id: &PaneId) -> Result<(), PrototypePolicyError> {
        match &self.active_overlay_pane {
            Some(active) if active != pane_id => Err(PrototypePolicyError::OverlaySlotInUse {
                active_overlay_pane: active.clone(),
            }),
            _ => {
                self.active_overlay_pane = Some(pane_id.clone());
                Ok(())
            }
        }
    }

    pub fn release_if_matches(&mut self, pane_id: &PaneId) {
        if self.active_overlay_pane.as_ref() == Some(pane_id) {
            self.active_overlay_pane = None;
        }
    }

    pub fn clear_if_removed(&mut self, pane_exists: impl Fn(&PaneId) -> bool) {
        if let Some(active) = &self.active_overlay_pane {
            if !pane_exists(active) {
                self.active_overlay_pane = None;
            }
        }
    }

    pub fn status(&self) -> PrototypePolicyStatus {
        PrototypePolicyStatus {
            active_overlay_pane: self.active_overlay_pane.clone(),
        }
    }
}
