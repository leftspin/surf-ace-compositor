//! Ephemeral presentation policy. Callers validate the bound main-app source;
//! this module fences its generation and keeps tiled geometry out of the ledger.
use crate::model::{NativePanePresentationGeneration, OverlayRect, PaneGeometry, StatusSnapshot};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const CAPABILITY_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "host", rename_all = "snake_case", deny_unknown_fields)]
pub enum SelectedPane {
    Native {
        renderer_pane_id: u64,
        pane_lineage_id: String,
        native_pane_id: String,
        native_pane_instance_id: String,
    },
    Renderer {
        renderer_pane_id: u64,
        pane_lineage_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PresentationRequest {
    pub version: u32,
    pub request_revision: u64,
    pub presentation_generation: NativePanePresentationGeneration,
    pub selected: Option<SelectedPane>,
    pub overlay_rect: Option<OverlayRect>,
    pub content_rect: Option<OverlayRect>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PresentationAck {
    pub version: u32,
    pub request_revision: u64,
    pub presentation_generation: NativePanePresentationGeneration,
    pub selected: Option<SelectedPane>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PresentationInput {
    Normal,
    Renderer,
    Native(String),
}

#[derive(Debug, Clone, Default)]
pub struct PresentationLedger {
    // Includes Restore tombstones so a delayed selection cannot undo Restore.
    entries: BTreeMap<String, PresentationRequest>,
}

impl PresentationLedger {
    pub fn accept(
        &mut self,
        request: PresentationRequest,
        current: &NativePanePresentationGeneration,
        surface_size: (f64, f64),
    ) -> Result<PresentationAck, String> {
        let generation = &request.presentation_generation;
        if request.version != CAPABILITY_VERSION || request.request_revision == 0 {
            return Err("unsupported presentation version or revision".into());
        }
        // A caller can retire its exact acknowledged selection after local
        // geometry/focus/topology advanced. This cannot select stale content,
        // replace a newer generation, or remove the monotonic replay fence.
        if request.selected.is_none()
            && request.overlay_rect.is_none()
            && request.content_rect.is_none()
            && self
                .entries
                .get(&generation.surface_id)
                .is_some_and(|previous| {
                    previous.presentation_generation == *generation
                        && request.request_revision > previous.request_revision
                })
        {
            let accepted = ack(&request);
            self.entries.insert(generation.surface_id.clone(), request);
            return Ok(accepted);
        }
        if generation.surface_id.is_empty()
            || generation.surface_epoch.is_empty()
            || !same_generation(generation, current)
        {
            return Err("presentation generation does not match routed surface".into());
        }
        for (id, instance) in &generation.pane_instances {
            if current.pane_instances.get(id) != Some(instance) {
                return Err("presentation contains a foreign native instance".into());
            }
        }
        match &request.selected {
            Some(selected) => {
                let (pane_id, lineage) = match selected {
                    SelectedPane::Native {
                        renderer_pane_id,
                        pane_lineage_id,
                        native_pane_id,
                        native_pane_instance_id: _,
                    } => {
                        if current.pane_instances.get(native_pane_id) != Some(pane_lineage_id) {
                            return Err("selected native lineage is not current".into());
                        }
                        (*renderer_pane_id, pane_lineage_id)
                    }
                    SelectedPane::Renderer {
                        renderer_pane_id,
                        pane_lineage_id,
                    } => (*renderer_pane_id, pane_lineage_id),
                };
                if pane_id == 0 || lineage.is_empty() {
                    return Err("selected pane identity is missing".into());
                }
                let overlay = request.overlay_rect.ok_or("missing overlay rectangle")?;
                let content = request.content_rect.ok_or("missing content rectangle")?;
                if !valid_rect(overlay, surface_size)
                    || !valid_rect(content, surface_size)
                    || !contains_rect(overlay, content)
                {
                    return Err("presentation rectangle is outside its surface or overlay".into());
                }
            }
            None if request.overlay_rect.is_some() || request.content_rect.is_some() => {
                return Err("Restore cannot carry display rectangles".into());
            }
            None => {}
        }
        if let Some(previous) = self.entries.get(&generation.surface_id) {
            if previous.presentation_generation.surface_epoch == generation.surface_epoch {
                if request.request_revision < previous.request_revision {
                    return Err("stale presentation request".into());
                }
                if request.request_revision == previous.request_revision {
                    if previous != &request {
                        return Err("conflicting presentation replay".into());
                    }
                    return Ok(ack(previous));
                }
            }
        }
        let accepted = ack(&request);
        self.entries.insert(generation.surface_id.clone(), request);
        Ok(accepted)
    }

    pub fn current(
        &self,
        generation: &NativePanePresentationGeneration,
    ) -> Option<&PresentationRequest> {
        self.entries
            .get(&generation.surface_id)
            .filter(|entry| same_generation(&entry.presentation_generation, generation))
    }

    pub fn clear_surface(&mut self, surface_id: &str) {
        self.entries.remove(surface_id);
    }

    pub fn active_requests(
        &self,
        generations: &BTreeMap<String, NativePanePresentationGeneration>,
    ) -> Vec<PresentationRequest> {
        generations
            .values()
            .filter_map(|generation| {
                let entry = self.entries.get(&generation.surface_id)?;
                if entry.selected.is_none()
                    || entry.presentation_generation.surface_epoch != generation.surface_epoch
                    || entry.presentation_generation.topology_epoch != generation.topology_epoch
                {
                    return None;
                }
                if let Some(SelectedPane::Native {
                    native_pane_id,
                    pane_lineage_id,
                    ..
                }) = &entry.selected
                {
                    if generation.pane_instances.get(native_pane_id) != Some(pane_lineage_id) {
                        return None;
                    }
                }
                let mut projected = entry.clone();
                // A retained sibling can acquire native content without changing topology.
                // Visibility must cover the current cohort, not only the initial cohort.
                projected.presentation_generation.pane_instances =
                    generation.pane_instances.clone();
                Some(projected)
            })
            .collect()
    }
}

impl PresentationRequest {
    pub fn displayed_geometry(&self, pane_id: &str, tiled: PaneGeometry) -> PaneGeometry {
        match (&self.selected, self.content_rect) {
            (Some(SelectedPane::Native { native_pane_id, .. }), Some(rect))
                if native_pane_id == pane_id =>
            {
                PaneGeometry {
                    x: rect.x,
                    y: rect.y,
                    width: rect.width,
                    height: rect.height,
                    coordinate_space: tiled.coordinate_space,
                }
            }
            _ => tiled,
        }
    }
    pub fn native_visible(&self, pane_id: &str) -> bool {
        if !self
            .presentation_generation
            .pane_instances
            .contains_key(pane_id)
        {
            return true;
        }
        match &self.selected {
            None => true,
            Some(SelectedPane::Native { native_pane_id, .. }) => native_pane_id == pane_id,
            Some(SelectedPane::Renderer { .. }) => false,
        }
    }

    pub fn input_at(&self, x: f64, y: f64) -> PresentationInput {
        match &self.selected {
            None => PresentationInput::Normal,
            Some(SelectedPane::Native { native_pane_id, .. })
                if self
                    .content_rect
                    .is_some_and(|rect| contains_point(rect, x, y)) =>
            {
                PresentationInput::Native(native_pane_id.clone())
            }
            Some(_) => PresentationInput::Renderer,
        }
    }
}

impl StatusSnapshot {
    pub fn presentation_input_at(&self, x: f64, y: f64) -> PresentationInput {
        match self.pane_presentations.as_slice() {
            [] => PresentationInput::Normal,
            [request] => request.input_at(x, y),
            // A single bound main-app host cannot disambiguate multiple renderer
            // window-local coordinates. Fail closed until the caller routes them.
            _ => PresentationInput::Renderer,
        }
    }
    pub fn presentation_native_visible(&self, pane_id: &str) -> bool {
        self.pane_presentations
            .iter()
            .all(|request| request.native_visible(pane_id))
    }

    pub fn presentation_revisions(&self) -> Vec<(String, String, u64)> {
        self.pane_presentations
            .iter()
            .map(|request| {
                (
                    request.presentation_generation.surface_id.clone(),
                    request.presentation_generation.surface_epoch.clone(),
                    request.request_revision,
                )
            })
            .collect()
    }
}

fn same_generation(
    a: &NativePanePresentationGeneration,
    b: &NativePanePresentationGeneration,
) -> bool {
    a.surface_id == b.surface_id
        && a.surface_epoch == b.surface_epoch
        && a.topology_epoch == b.topology_epoch
        && a.geometry_revision == b.geometry_revision
        && a.focus_revision == b.focus_revision
}

fn ack(request: &PresentationRequest) -> PresentationAck {
    PresentationAck {
        version: CAPABILITY_VERSION,
        request_revision: request.request_revision,
        presentation_generation: request.presentation_generation.clone(),
        selected: request.selected.clone(),
    }
}

fn valid_rect(rect: OverlayRect, size: (f64, f64)) -> bool {
    [rect.x, rect.y, rect.width, rect.height, size.0, size.1]
        .iter()
        .all(|n| n.is_finite())
        && rect.x >= 0.0
        && rect.y >= 0.0
        && rect.width > 0.0
        && rect.height > 0.0
        && rect.x + rect.width <= size.0
        && rect.y + rect.height <= size.1
}
fn contains_rect(outer: OverlayRect, inner: OverlayRect) -> bool {
    inner.x >= outer.x
        && inner.y >= outer.y
        && inner.x + inner.width <= outer.x + outer.width
        && inner.y + inner.height <= outer.y + outer.height
}
fn contains_point(rect: OverlayRect, x: f64, y: f64) -> bool {
    x >= rect.x && y >= rect.y && x < rect.x + rect.width && y < rect.y + rect.height
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request(revision: u64) -> PresentationRequest {
        PresentationRequest {
            version: 1,
            request_revision: revision,
            presentation_generation: NativePanePresentationGeneration {
                surface_id: "s".into(),
                surface_epoch: "s:1".into(),
                topology_epoch: 2,
                geometry_revision: 3,
                focus_revision: 4,
                pane_instances: BTreeMap::from([
                    ("s:1".into(), "lineage-1".into()),
                    ("s:9".into(), "lineage-9".into()),
                ]),
            },
            selected: Some(SelectedPane::Native {
                renderer_pane_id: 1,
                pane_lineage_id: "lineage-1".into(),
                native_pane_id: "s:1".into(),
                native_pane_instance_id: "instance-1".into(),
            }),
            overlay_rect: Some(OverlayRect {
                x: 10.0,
                y: 10.0,
                width: 980.0,
                height: 680.0,
            }),
            content_rect: Some(OverlayRect {
                x: 12.0,
                y: 12.0,
                width: 976.0,
                height: 676.0,
            }),
        }
    }
    #[test]
    fn native_selection_occludes_later_id_and_backdrop_cannot_hit_sibling() {
        let req = request(1);
        assert!(req.native_visible("s:1"));
        assert!(!req.native_visible("s:9"));
        assert_eq!(
            req.input_at(400.0, 300.0),
            PresentationInput::Native("s:1".into())
        );
        assert_eq!(req.input_at(2.0, 2.0), PresentationInput::Renderer);
    }
    #[test]
    fn displayed_geometry_expands_only_selected_host_and_restore_uses_original_tile() {
        let selected = request(1);
        let tiled = PaneGeometry {
            x: 0.0,
            y: 0.0,
            width: 200.0,
            height: 400.0,
            coordinate_space: crate::model::PaneGeometryCoordinateSpace::CompositorLogical,
        };
        let displayed = selected.displayed_geometry("s:1", tiled);
        assert_eq!(displayed.width, 976.0);
        assert_eq!(displayed.x, 12.0);
        assert_eq!(selected.displayed_geometry("s:9", tiled), tiled);
        assert_eq!(selected.displayed_geometry("other:1", tiled), tiled);
        let mut restored = selected;
        restored.selected = None;
        restored.content_rect = None;
        restored.overlay_rect = None;
        assert_eq!(restored.displayed_geometry("s:1", tiled), tiled);
        assert_eq!(tiled.width, 200.0);
    }
    #[test]
    fn renderer_selection_masks_native_visuals_and_all_native_input() {
        let mut req = request(1);
        req.selected = Some(SelectedPane::Renderer {
            renderer_pane_id: 3,
            pane_lineage_id: "lineage-3".into(),
        });
        assert!(!req.native_visible("s:1") && !req.native_visible("s:9"));
        assert_eq!(req.input_at(400.0, 300.0), PresentationInput::Renderer);
        assert_eq!(req.input_at(2.0, 2.0), PresentationInput::Renderer);
    }
    #[test]
    fn retained_selection_covers_new_native_sibling_without_affecting_other_window() {
        let req = request(1);
        let mut current = req.presentation_generation.clone();
        let mut ledger = PresentationLedger::default();
        ledger.accept(req, &current, (1000.0, 700.0)).unwrap();
        current.geometry_revision += 1;
        current.focus_revision += 1;
        current
            .pane_instances
            .insert("s:3".into(), "new-instance".into());
        let active = ledger.active_requests(&BTreeMap::from([("s".into(), current)]));
        assert_eq!(active.len(), 1);
        assert!(active[0].native_visible("s:1"));
        assert!(!active[0].native_visible("s:3"));
        assert!(active[0].native_visible("other-window:3"));
    }
    #[test]
    fn topology_or_selected_instance_replacement_clears_display_projection() {
        let req = request(1);
        let current = req.presentation_generation.clone();
        let mut ledger = PresentationLedger::default();
        ledger.accept(req, &current, (1000.0, 700.0)).unwrap();
        let mut changed = current.clone();
        changed.topology_epoch += 1;
        assert!(
            ledger
                .active_requests(&BTreeMap::from([("s".into(), changed)]))
                .is_empty()
        );
        let mut changed = current;
        changed
            .pane_instances
            .insert("s:1".into(), "replacement-instance".into());
        assert!(
            ledger
                .active_requests(&BTreeMap::from([("s".into(), changed)]))
                .is_empty()
        );
        ledger.clear_surface("s");
        assert!(ledger.entries.is_empty());
    }
    #[test]
    fn restore_tombstone_rejects_delayed_selection_and_conflicting_replay() {
        let req = request(1);
        let generation = req.presentation_generation.clone();
        let mut ledger = PresentationLedger::default();
        let first = ledger
            .accept(req.clone(), &generation, (1000.0, 700.0))
            .unwrap();
        assert_eq!(
            ledger
                .accept(req.clone(), &generation, (1000.0, 700.0))
                .unwrap(),
            first
        );
        let mut restore = request(2);
        restore.selected = None;
        restore.overlay_rect = None;
        restore.content_rect = None;
        ledger
            .accept(restore.clone(), &generation, (1000.0, 700.0))
            .unwrap();
        assert!(ledger.accept(req, &generation, (1000.0, 700.0)).is_err());
        assert!(
            ledger
                .accept(request(2), &generation, (1000.0, 700.0))
                .is_err()
        );
        assert_eq!(ledger.current(&generation), Some(&restore));
    }
    #[test]
    fn exact_prior_generation_restore_retires_after_authority_advance_but_not_newer_selection() {
        let req = request(1);
        let generation = req.presentation_generation.clone();
        let mut ledger = PresentationLedger::default();
        ledger
            .accept(req.clone(), &generation, (1000.0, 700.0))
            .unwrap();
        let mut advanced = generation.clone();
        advanced.geometry_revision += 1;
        advanced.focus_revision += 1;
        let mut restore = req.clone();
        restore.request_revision = 2;
        restore.selected = None;
        restore.overlay_rect = None;
        restore.content_rect = None;
        assert!(
            ledger
                .accept(restore.clone(), &advanced, (900.0, 600.0))
                .is_ok()
        );
        assert!(
            ledger
                .active_requests(&BTreeMap::from([("s".into(), advanced.clone())]))
                .is_empty()
        );
        assert!(ledger.accept(req, &advanced, (1000.0, 700.0)).is_err());
        let mut newer = request(3);
        newer.presentation_generation = advanced.clone();
        ledger
            .accept(newer.clone(), &advanced, (1000.0, 700.0))
            .unwrap();
        restore.request_revision = 4;
        assert!(ledger.accept(restore, &advanced, (1000.0, 700.0)).is_err());
        assert_eq!(ledger.current(&advanced), Some(&newer));
    }
    #[test]
    fn stale_identity_or_bad_geometry_cannot_replace_accepted_presentation() {
        let req = request(1);
        let generation = req.presentation_generation.clone();
        let mut ledger = PresentationLedger::default();
        ledger
            .accept(req.clone(), &generation, (1000.0, 700.0))
            .unwrap();
        let mut foreign = request(2);
        foreign.presentation_generation.surface_epoch = "other".into();
        assert!(
            ledger
                .accept(foreign, &generation, (1000.0, 700.0))
                .is_err()
        );
        let mut invalid = request(2);
        invalid.content_rect.as_mut().unwrap().width = f64::NAN;
        assert!(
            ledger
                .accept(invalid, &generation, (1000.0, 700.0))
                .is_err()
        );
        assert_eq!(ledger.current(&generation), Some(&req));
        let mut new_topology = generation.clone();
        new_topology.topology_epoch += 1;
        assert!(ledger.current(&new_topology).is_none());
    }
}
