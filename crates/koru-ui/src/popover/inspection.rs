// SPDX-License-Identifier: MPL-2.0

//! Shared shell evidence. Host-declared placement is distinct from measuring
//! the current rectangle, and opacity alone never supplies lifecycle state.

use super::*;
use koru_draw::inspect::{self, Record, Scope};

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum KoruPopoverState {
    #[default]
    Unknown,
    Opening,
    Open,
    Closing,
}

#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub enum KoruPopoverPlacement {
    /// The caller supplies a rectangle without declaring anchored placement.
    #[default]
    CallerRect,
    /// The host used PopoverShell::anchored for this cached rectangle.
    Anchored { anchor: Rect, align: PopoverAlign },
}

/// Host-local identity evidence, not an accessibility node or a registry.
/// Geometry and lifecycle come from the host's existing state; this structure
/// does not mint missing occurrences or retain private content.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct KoruPopoverInspection {
    pub occurrence: Option<u64>,
    pub owner: Option<u64>,
    pub trigger: Option<u64>,
    pub state: KoruPopoverState,
    pub placement: KoruPopoverPlacement,
}

fn identity(r: &mut Record, key: &'static str, id: Option<u64>) {
    match id {
        Some(id) => r.property(
            key,
            id,
            "host-local occurrence namespace; existing host identity",
        ),
        None => r.property(key, "unavailable", "host does not supply this occurrence"),
    }
}

pub(super) fn scope(
    shell: &PopoverShell,
    root: Affine,
    association: KoruPopoverInspection,
) -> Option<Scope> {
    if !inspect::enabled() {
        return None;
    }
    Some(inspect::scope(
        "KoruPopover",
        "component",
        root,
        scene::rect(shell.rect.x, shell.rect.y, shell.rect.w, shell.rect.h),
        |r| {
            let recipe = theme::overlay::popover();
            r.property(
                "component family",
                "C-OVERLAY",
                "docs/features/koruui/COMPONENTS.md",
            );
            identity(r, "occurrence", association.occurrence);
            identity(r, "logical owner occurrence", association.owner);
            identity(r, "trigger occurrence", association.trigger);
            r.property(
                "state",
                format!("{:?}", association.state),
                "explicit host lifecycle; never inferred from opacity",
            );
            r.property(
                "radius",
                shell.radius,
                "PopoverShell::radius; actual caller-owned value",
            );
            r.property(
                "recipe radius",
                recipe.radius,
                "theme::overlay::popover().radius; anchored default",
            );
            r.property(
                "recipe viewport inset",
                recipe.viewport_inset,
                "theme::overlay::popover().viewport_inset; anchored default, not measured padding",
            );
            r.property(
                "recipe anchor gap",
                recipe.anchor_gap,
                "theme::overlay::popover().anchor_gap; anchored default, not measured spacing",
            );
            match association.placement {
                KoruPopoverPlacement::CallerRect => r.property(
                    "placement",
                    "caller rectangle; anchored origin not supplied",
                    "KoruPopoverPlacement::CallerRect",
                ),
                KoruPopoverPlacement::Anchored { anchor, align } => {
                    r.property(
                        "placement",
                        format!("anchored {align:?}"),
                        "host declares PopoverShell::anchored supplied cached geometry",
                    );
                    r.property(
                        "anchor bounds (x y w h)",
                        format!("{}, {}, {}, {}", anchor.x, anchor.y, anchor.w, anchor.h),
                        "host anchor in local logical units; separate from trigger occurrence",
                    );
                }
            }
            r.property(
                "content ownership",
                "callback spans shell and content; local content needs unregistered boundaries",
                "PopoverShell::draw_with_content",
            );
            r.property(
                "native semantics",
                "host-owned; a popover does not imply menu or modal behavior",
                "paint evidence only",
            );
            r.property("docs", "docs/features/koruui/COMPONENTS.md", "catalog");
        },
    ))
}
