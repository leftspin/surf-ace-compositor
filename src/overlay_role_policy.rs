use crate::model::{OverlayRolePolicyStatus, PaneId};
use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum OverlayRolePolicyError {
    #[error(
        "overlay role policy allows only one active overlay pane (active: {active_overlay_pane:?})"
    )]
    OverlaySlotInUse { active_overlay_pane: PaneId },
}

#[derive(Debug, Default, Clone)]
pub struct OverlayRolePolicy {
    // This encodes the production v1 shell overlay role policy (single overlay layer).
    // It is intentionally separate from provider-owned native pane hosting so the
    // shell overlay role cannot cap pane-hosted native surfaces.
    active_overlay_pane: Option<PaneId>,
}

impl OverlayRolePolicy {
    pub fn active_overlay_pane(&self) -> Option<&PaneId> {
        self.active_overlay_pane.as_ref()
    }

    pub fn reserve_for(&mut self, pane_id: &PaneId) -> Result<(), OverlayRolePolicyError> {
        match &self.active_overlay_pane {
            Some(active) if active != pane_id => Err(OverlayRolePolicyError::OverlaySlotInUse {
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

    pub fn status(&self) -> OverlayRolePolicyStatus {
        OverlayRolePolicyStatus {
            active_overlay_pane: self.active_overlay_pane.clone(),
        }
    }
}
