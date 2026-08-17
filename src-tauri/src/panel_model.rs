use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhysicalRect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PhysicalSize {
    pub width: i32,
    pub height: i32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PhysicalPoint {
    pub x: i32,
    pub y: i32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PetAnchor {
    pub pet_rect: PhysicalRect,
    pub work_area: PhysicalRect,
    pub dpi: u32,
    pub visible: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelSide {
    Below,
    Above,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PanelPlacement {
    pub position: PhysicalPoint,
    pub side: PanelSide,
}

pub fn place_panel(anchor: PetAnchor, panel: PhysicalSize, gap: i32) -> PanelPlacement {
    let panel = PhysicalSize {
        width: panel.width.max(1),
        height: panel.height.max(1),
    };
    let gap = gap.max(0);
    let pet_width = i64::from(anchor.pet_rect.right) - i64::from(anchor.pet_rect.left);
    let centered_x = i64::from(anchor.pet_rect.left) + (pet_width - i64::from(panel.width)) / 2;
    let below_y = anchor.pet_rect.bottom.saturating_add(gap);
    let below_fits = below_y.saturating_add(panel.height) <= anchor.work_area.bottom;
    let (candidate_y, side) = if below_fits {
        (below_y, PanelSide::Below)
    } else {
        (
            anchor
                .pet_rect
                .top
                .saturating_sub(gap)
                .saturating_sub(panel.height),
            PanelSide::Above,
        )
    };
    let max_x = anchor
        .work_area
        .right
        .saturating_sub(panel.width)
        .max(anchor.work_area.left);
    let max_y = anchor
        .work_area
        .bottom
        .saturating_sub(panel.height)
        .max(anchor.work_area.top);

    PanelPlacement {
        position: PhysicalPoint {
            x: saturating_i32(centered_x).clamp(anchor.work_area.left, max_x),
            y: candidate_y.clamp(anchor.work_area.top, max_y),
        },
        side,
    }
}

fn saturating_i32(value: i64) -> i32 {
    value.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelEffect {
    None,
    Open { generation: u64 },
    Close { generation: u64 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PendingFocusLoss {
    generation: u64,
    revision: u64,
}

#[derive(Debug, Default)]
pub struct PanelState {
    open: bool,
    generation: u64,
    focused: bool,
    focus_revision: u64,
}

impl PanelState {
    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn toggle(&mut self) -> PanelEffect {
        if self.open {
            self.close()
        } else {
            self.open = true;
            self.generation = self.generation.wrapping_add(1);
            self.focused = false;
            self.focus_revision = self.focus_revision.wrapping_add(1);
            PanelEffect::Open {
                generation: self.generation,
            }
        }
    }

    pub fn close(&mut self) -> PanelEffect {
        if !self.open {
            return PanelEffect::None;
        }
        self.open = false;
        self.generation = self.generation.wrapping_add(1);
        PanelEffect::Close {
            generation: self.generation,
        }
    }

    pub fn close_generation(&mut self, generation: u64) -> PanelEffect {
        if !self.accepts_response(generation) {
            return PanelEffect::None;
        }
        self.close()
    }

    pub fn internal_action(&mut self) -> PanelEffect {
        PanelEffect::None
    }

    pub fn record_focus_change(
        &mut self,
        generation: u64,
        focused: bool,
    ) -> Option<PendingFocusLoss> {
        if !self.accepts_response(generation) {
            return None;
        }
        self.focused = focused;
        self.focus_revision = self.focus_revision.wrapping_add(1);
        (!focused).then_some(PendingFocusLoss {
            generation,
            revision: self.focus_revision,
        })
    }

    pub fn confirm_focus_loss(&mut self, pending: PendingFocusLoss) -> PanelEffect {
        if !self.accepts_response(pending.generation)
            || self.focused
            || self.focus_revision != pending.revision
        {
            return PanelEffect::None;
        }
        self.close()
    }

    #[cfg(test)]
    pub fn drag_ended(&mut self) -> PanelEffect {
        PanelEffect::None
    }

    pub fn accepts_response(&self, generation: u64) -> bool {
        self.open && self.generation == generation
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn anchor(pet_rect: PhysicalRect, work_area: PhysicalRect) -> PetAnchor {
        PetAnchor {
            pet_rect,
            work_area,
            dpi: 96,
            visible: true,
        }
    }

    #[test]
    fn panel_prefers_below_and_centers_on_the_pet() {
        let placement = place_panel(
            anchor(
                PhysicalRect {
                    left: 800,
                    top: 300,
                    right: 1020,
                    bottom: 540,
                },
                PhysicalRect {
                    left: 0,
                    top: 0,
                    right: 1920,
                    bottom: 1040,
                },
            ),
            PhysicalSize {
                width: 360,
                height: 430,
            },
            12,
        );
        assert_eq!(placement.side, PanelSide::Below);
        assert_eq!(placement.position, PhysicalPoint { x: 730, y: 552 });
    }

    #[test]
    fn panel_flips_above_when_below_does_not_fit() {
        let placement = place_panel(
            anchor(
                PhysicalRect {
                    left: 800,
                    top: 760,
                    right: 1020,
                    bottom: 1000,
                },
                PhysicalRect {
                    left: 0,
                    top: 0,
                    right: 1920,
                    bottom: 1040,
                },
            ),
            PhysicalSize {
                width: 360,
                height: 430,
            },
            12,
        );
        assert_eq!(placement.side, PanelSide::Above);
        assert_eq!(placement.position, PhysicalPoint { x: 730, y: 318 });
    }

    #[test]
    fn panel_clamps_to_edges_negative_monitors_and_tiny_work_areas() {
        let panel = PhysicalSize {
            width: 360,
            height: 430,
        };
        let left = place_panel(
            anchor(
                PhysicalRect {
                    left: -1280,
                    top: 100,
                    right: -1100,
                    bottom: 300,
                },
                PhysicalRect {
                    left: -1280,
                    top: 0,
                    right: 0,
                    bottom: 984,
                },
            ),
            panel,
            12,
        );
        assert_eq!(left.position.x, -1280);

        let right = place_panel(
            anchor(
                PhysicalRect {
                    left: 1850,
                    top: 100,
                    right: 1920,
                    bottom: 340,
                },
                PhysicalRect {
                    left: 0,
                    top: 0,
                    right: 1920,
                    bottom: 1040,
                },
            ),
            panel,
            12,
        );
        assert_eq!(right.position.x, 1560);

        let tiny = place_panel(
            anchor(
                PhysicalRect {
                    left: -300,
                    top: -200,
                    right: -240,
                    bottom: -140,
                },
                PhysicalRect {
                    left: -320,
                    top: -240,
                    right: -120,
                    bottom: 60,
                },
            ),
            panel,
            12,
        );
        assert_eq!(tiny.position, PhysicalPoint { x: -320, y: -240 });
    }

    #[test]
    fn toggle_close_and_stale_response_transitions_are_idempotent() {
        let mut state = PanelState::default();
        assert_eq!(state.toggle(), PanelEffect::Open { generation: 1 });
        assert!(state.is_open());
        assert!(state.accepts_response(1));
        assert_eq!(state.internal_action(), PanelEffect::None);
        assert_eq!(state.toggle(), PanelEffect::Close { generation: 2 });
        assert!(!state.is_open());
        assert!(!state.accepts_response(1));
        assert_eq!(state.close(), PanelEffect::None);

        assert_eq!(state.toggle(), PanelEffect::Open { generation: 3 });
        assert!(!state.accepts_response(1));
        assert!(state.accepts_response(3));
        assert_eq!(state.close_generation(1), PanelEffect::None);
        assert!(state.is_open());
        assert_eq!(state.close(), PanelEffect::Close { generation: 4 });
        assert_eq!(state.close(), PanelEffect::None);
    }

    #[test]
    fn transient_focus_loss_during_opening_does_not_close_panel() {
        let mut state = PanelState::default();
        assert_eq!(state.toggle(), PanelEffect::Open { generation: 1 });

        assert_eq!(state.record_focus_change(1, true), None);
        let transient_loss = state
            .record_focus_change(1, false)
            .expect("focus loss should be confirmed after the event burst settles");
        assert_eq!(state.record_focus_change(1, true), None);
        assert_eq!(state.confirm_focus_loss(transient_loss), PanelEffect::None);
        assert!(state.is_open());

        let external_loss = state
            .record_focus_change(1, false)
            .expect("a sustained external focus loss should be confirmed");
        assert_eq!(
            state.confirm_focus_loss(external_loss),
            PanelEffect::Close { generation: 2 }
        );
        assert!(!state.is_open());
    }

    #[test]
    fn drag_completion_never_opens_the_panel() {
        let mut state = PanelState::default();
        assert_eq!(state.drag_ended(), PanelEffect::None);
        assert!(!state.is_open());
    }

    #[test]
    fn anchor_dto_exposes_only_physical_geometry_dpi_and_visibility() {
        let value = serde_json::to_value(anchor(
            PhysicalRect {
                left: -320,
                top: 40,
                right: -100,
                bottom: 280,
            },
            PhysicalRect {
                left: -1280,
                top: 0,
                right: 0,
                bottom: 984,
            },
        ))
        .unwrap();
        assert_eq!(value["petRect"]["left"], -320);
        assert_eq!(value["workArea"]["bottom"], 984);
        assert_eq!(value["dpi"], 96);
        assert_eq!(value["visible"], true);
        assert_eq!(value.as_object().unwrap().len(), 4);
    }
}
