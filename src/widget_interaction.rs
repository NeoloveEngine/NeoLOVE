//! Retained interaction state shared by NeoLOVE's immediate-mode runtime widgets.
//!
//! Widgets still draw and expose their Lua API from `core`, but pointer ownership
//! lives here.  Keeping the hit regions from the previous rendered frame makes
//! the result independent of the order in which callbacks happen to query the
//! pointer: the last drawn matching region wins, popups win over normal content,
//! and a captured pointer cannot leak into another widget mid-drag.

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};

/// Normal widgets draw in the content layer. Popup regions always win a hit test.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Layer {
    #[default]
    Content,
    Popup,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Region {
    /// Unique region id. A dropdown, for example, owns separate body and menu
    /// regions while both route interaction to the same `owner`.
    pub id: String,
    /// Stable widget id used for pointer capture and keyboard focus.
    pub owner: String,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub pivot_x: f32,
    pub pivot_y: f32,
    pub rotation_degrees: f32,
    pub layer: Layer,
    pub focusable: bool,
    pub enabled: bool,
}

impl Region {
    fn contains(&self, point: [f32; 2]) -> bool {
        if !point[0].is_finite()
            || !point[1].is_finite()
            || !self.x.is_finite()
            || !self.y.is_finite()
            || !self.width.is_finite()
            || !self.height.is_finite()
            || !self.pivot_x.is_finite()
            || !self.pivot_y.is_finite()
            || !self.rotation_degrees.is_finite()
        {
            return false;
        }

        let radians = (-self.rotation_degrees).to_radians();
        let sin = radians.sin();
        let cos = radians.cos();
        let dx = point[0] - self.pivot_x;
        let dy = point[1] - self.pivot_y;
        let local_x = self.pivot_x + dx * cos - dy * sin;
        let local_y = self.pivot_y + dx * sin + dy * cos;
        let (min_x, max_x) = ordered_pair(self.x, self.x + self.width);
        let (min_y, max_y) = ordered_pair(self.y, self.y + self.height);
        local_x >= min_x && local_x <= max_x && local_y >= min_y && local_y <= max_y
    }
}

fn ordered_pair(a: f32, b: f32) -> (f32, f32) {
    if a <= b { (a, b) } else { (b, a) }
}

/// Pure interaction state. The global hub below keeps one of these per render
/// surface, preventing tests or multiple runtimes from stealing each other's
/// capture/focus state.
#[derive(Debug, Default)]
pub(crate) struct InteractionState {
    active_regions: Vec<Region>,
    next_regions: Vec<Region>,
    pointer_capture: Option<String>,
    focus_owner: Option<String>,
    tab_handled: bool,
}

impl InteractionState {
    pub(crate) fn begin_frame(&mut self) {
        self.active_regions = std::mem::take(&mut self.next_regions);
        self.tab_handled = false;

        // A removed/disabled control must not retain keyboard ownership. Do not
        // clear an empty initial frame: a programmatic focus request may happen
        // before the first set of regions is registered.
        if !self.active_regions.is_empty()
            && self.focus_owner.as_ref().is_some_and(|owner| {
                !self
                    .active_regions
                    .iter()
                    .any(|region| region.owner == *owner && region.focusable && region.enabled)
            })
        {
            self.focus_owner = None;
        }
        if self.pointer_capture.as_ref().is_some_and(|owner| {
            !self
                .active_regions
                .iter()
                .any(|region| region.owner == *owner)
        }) {
            self.pointer_capture = None;
        }
    }

    pub(crate) fn register(&mut self, region: Region) {
        upsert_region(&mut self.next_regions, region.clone());

        // Refresh current geometry without changing the prior frame's draw
        // order. A newly-created widget is provisionally appended so existing
        // one-frame click behaviour remains backwards compatible.
        if let Some(existing) = self
            .active_regions
            .iter_mut()
            .find(|existing| existing.id == region.id)
        {
            *existing = region;
        } else {
            self.active_regions.push(region);
        }
    }

    pub(crate) fn unregister_region(&mut self, id: &str) {
        self.active_regions.retain(|region| region.id != id);
        self.next_regions.retain(|region| region.id != id);
    }

    pub(crate) fn unregister_owner(&mut self, owner: &str) {
        self.active_regions.retain(|region| region.owner != owner);
        self.next_regions.retain(|region| region.owner != owner);
        if self.pointer_capture.as_deref() == Some(owner) {
            self.pointer_capture = None;
        }
        if self.focus_owner.as_deref() == Some(owner) {
            self.focus_owner = None;
        }
    }

    fn top_region_at(&self, point: [f32; 2]) -> Option<&Region> {
        let top_layer = self
            .active_regions
            .iter()
            .filter(|region| region.contains(point))
            .map(|region| region.layer)
            .max()?;
        self.active_regions
            .iter()
            .rev()
            .find(|region| region.layer == top_layer && region.contains(point))
    }

    pub(crate) fn region_hovered(&self, id: &str, point: [f32; 2]) -> bool {
        if let Some(captured) = self.pointer_capture.as_deref() {
            return self
                .active_regions
                .iter()
                .find(|region| region.id == id && region.owner == captured)
                .is_some_and(|region| region.contains(point));
        }
        self.top_region_at(point)
            .is_some_and(|region| region.id == id)
    }

    pub(crate) fn press_region(
        &mut self,
        id: &str,
        point: [f32; 2],
        pressed: bool,
        pointer_down: bool,
    ) -> bool {
        if !pressed || self.pointer_capture.is_some() {
            return false;
        }
        let Some(region) = self.top_region_at(point) else {
            return false;
        };
        if region.id != id || !region.enabled {
            return false;
        }
        let owner = region.owner.clone();
        if pointer_down {
            self.pointer_capture = Some(owner);
        }
        true
    }

    pub(crate) fn pointer_owned(&self, owner: &str) -> bool {
        self.pointer_capture.as_deref() == Some(owner)
    }

    /// Releases capture and reports whether `owner` held it. Callers can decide
    /// whether a release outside the widget cancels a click.
    pub(crate) fn release_pointer(&mut self, owner: &str, released: bool) -> bool {
        if !released || self.pointer_capture.as_deref() != Some(owner) {
            return false;
        }
        self.pointer_capture = None;
        true
    }

    pub(crate) fn request_focus(&mut self, owner: &str) {
        self.focus_owner = Some(owner.to_string());
    }

    pub(crate) fn adopt_focus_if_unowned(&mut self, owner: &str) {
        if self.focus_owner.is_none() {
            self.request_focus(owner);
        }
    }

    pub(crate) fn clear_focus(&mut self, owner: &str) {
        if self.focus_owner.as_deref() == Some(owner) {
            self.focus_owner = None;
        }
    }

    pub(crate) fn is_focused(&self, owner: &str) -> bool {
        self.focus_owner.as_deref() == Some(owner)
    }

    /// Moves focus once per frame through enabled regions in rendered order.
    /// Returns the new owner when Tab was handled.
    pub(crate) fn advance_focus(&mut self, tab_pressed: bool, reverse: bool) -> Option<String> {
        if !tab_pressed || self.tab_handled {
            return None;
        }
        self.tab_handled = true;

        let mut seen = HashSet::new();
        let owners = self
            .active_regions
            .iter()
            .filter(|region| region.focusable && region.enabled)
            .filter_map(|region| {
                seen.insert(region.owner.clone())
                    .then(|| region.owner.clone())
            })
            .collect::<Vec<_>>();
        if owners.is_empty() {
            self.focus_owner = None;
            return None;
        }

        let current = self
            .focus_owner
            .as_ref()
            .and_then(|owner| owners.iter().position(|candidate| candidate == owner));
        let next = if reverse {
            current
                .map(|index| index.checked_sub(1).unwrap_or(owners.len() - 1))
                .unwrap_or(owners.len() - 1)
        } else {
            current.map(|index| (index + 1) % owners.len()).unwrap_or(0)
        };
        let owner = owners[next].clone();
        self.focus_owner = Some(owner.clone());
        Some(owner)
    }
}

fn upsert_region(regions: &mut Vec<Region>, region: Region) {
    if let Some(existing) = regions.iter_mut().find(|existing| existing.id == region.id) {
        *existing = region;
    } else {
        regions.push(region);
    }
}

#[derive(Default)]
struct InteractionHub {
    surfaces: HashMap<usize, InteractionState>,
}

static HUB: OnceLock<Mutex<InteractionHub>> = OnceLock::new();

fn hub() -> &'static Mutex<InteractionHub> {
    HUB.get_or_init(|| Mutex::new(InteractionHub::default()))
}

fn with_surface<R>(surface: usize, fallback: R, run: impl FnOnce(&mut InteractionState) -> R) -> R {
    hub()
        .lock()
        .map(|mut hub| run(hub.surfaces.entry(surface).or_default()))
        .unwrap_or(fallback)
}

pub(crate) fn begin_frame(surface: usize) {
    with_surface(surface, (), InteractionState::begin_frame);
}

/// Remove retained focus, capture, and hit regions when a render surface dies.
pub(crate) fn remove_surface(surface: usize) {
    let Some(hub) = HUB.get() else {
        return;
    };
    let mut hub = match hub.lock() {
        Ok(hub) => hub,
        Err(poisoned) => poisoned.into_inner(),
    };
    hub.surfaces.remove(&surface);
}

pub(crate) fn register(surface: usize, region: Region) {
    with_surface(surface, (), |state| state.register(region));
}

pub(crate) fn unregister_region(surface: usize, id: &str) {
    with_surface(surface, (), |state| state.unregister_region(id));
}

pub(crate) fn unregister_owner(surface: usize, owner: &str) {
    with_surface(surface, (), |state| state.unregister_owner(owner));
}

pub(crate) fn region_hovered(surface: usize, id: &str, point: [f32; 2]) -> bool {
    with_surface(surface, false, |state| state.region_hovered(id, point))
}

pub(crate) fn press_region(
    surface: usize,
    id: &str,
    point: [f32; 2],
    pressed: bool,
    pointer_down: bool,
) -> bool {
    with_surface(surface, false, |state| {
        state.press_region(id, point, pressed, pointer_down)
    })
}

pub(crate) fn pointer_owned(surface: usize, owner: &str) -> bool {
    with_surface(surface, false, |state| state.pointer_owned(owner))
}

pub(crate) fn release_pointer(surface: usize, owner: &str, released: bool) -> bool {
    with_surface(surface, false, |state| {
        state.release_pointer(owner, released)
    })
}

pub(crate) fn request_focus(surface: usize, owner: &str) {
    with_surface(surface, (), |state| state.request_focus(owner));
}

pub(crate) fn adopt_focus_if_unowned(surface: usize, owner: &str) {
    with_surface(surface, (), |state| state.adopt_focus_if_unowned(owner));
}

pub(crate) fn clear_focus(surface: usize, owner: &str) {
    with_surface(surface, (), |state| state.clear_focus(owner));
}

pub(crate) fn is_focused(surface: usize, owner: &str) -> bool {
    with_surface(surface, false, |state| state.is_focused(owner))
}

pub(crate) fn advance_focus(surface: usize, tab_pressed: bool, reverse: bool) -> Option<String> {
    with_surface(surface, None, |state| {
        state.advance_focus(tab_pressed, reverse)
    })
}

#[cfg(test)]
fn has_surface(surface: usize) -> bool {
    HUB.get().is_some_and(|hub| {
        let hub = match hub.lock() {
            Ok(hub) => hub,
            Err(poisoned) => poisoned.into_inner(),
        };
        hub.surfaces.contains_key(&surface)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn region(id: &str, owner: &str, x: f32, layer: Layer) -> Region {
        Region {
            id: id.to_string(),
            owner: owner.to_string(),
            x,
            y: 0.0,
            width: 20.0,
            height: 20.0,
            pivot_x: x,
            pivot_y: 0.0,
            rotation_degrees: 0.0,
            layer,
            focusable: true,
            enabled: true,
        }
    }

    #[test]
    fn last_drawn_overlapping_region_is_the_only_hit_target() {
        let mut state = InteractionState::default();
        state.register(region("back", "back", 0.0, Layer::Content));
        state.register(region("front", "front", 0.0, Layer::Content));
        state.begin_frame();

        assert!(!state.region_hovered("back", [10.0, 10.0]));
        assert!(state.region_hovered("front", [10.0, 10.0]));
        assert!(!state.press_region("back", [10.0, 10.0], true, true));
        assert!(state.press_region("front", [10.0, 10.0], true, true));
    }

    #[test]
    fn popup_layer_wins_even_when_content_was_drawn_later() {
        let mut state = InteractionState::default();
        state.register(region("menu", "dropdown", 0.0, Layer::Popup));
        state.register(region("late-content", "button", 0.0, Layer::Content));
        state.begin_frame();

        assert!(state.region_hovered("menu", [5.0, 5.0]));
        assert!(!state.region_hovered("late-content", [5.0, 5.0]));
    }

    #[test]
    fn pointer_capture_survives_leaving_bounds_and_blocks_other_widgets() {
        let mut state = InteractionState::default();
        state.register(region("slider", "slider", 0.0, Layer::Content));
        state.register(region("other", "other", 40.0, Layer::Content));
        state.begin_frame();

        assert!(state.press_region("slider", [10.0, 10.0], true, true));
        assert!(state.pointer_owned("slider"));
        assert!(!state.region_hovered("other", [45.0, 10.0]));
        assert!(state.release_pointer("slider", true));
        assert!(state.region_hovered("other", [45.0, 10.0]));
    }

    #[test]
    fn focus_is_exclusive_and_tab_wraps_in_render_order() {
        let mut state = InteractionState::default();
        state.register(region("a-body", "a", 0.0, Layer::Content));
        state.register(region("b-body", "b", 30.0, Layer::Content));
        state.begin_frame();

        state.request_focus("a");
        assert!(state.is_focused("a"));
        state.request_focus("b");
        assert!(!state.is_focused("a"));
        assert!(state.is_focused("b"));
        assert_eq!(state.advance_focus(true, false).as_deref(), Some("a"));
        assert_eq!(
            state.advance_focus(true, false),
            None,
            "Tab is consumed once"
        );

        state.register(region("a-body", "a", 0.0, Layer::Content));
        state.register(region("b-body", "b", 30.0, Layer::Content));
        state.begin_frame();
        assert_eq!(state.advance_focus(true, true).as_deref(), Some("b"));
    }

    #[test]
    fn rotated_and_negative_extent_regions_hit_correctly() {
        let mut state = InteractionState::default();
        let mut rotated = region("rotated", "rotated", 0.0, Layer::Content);
        rotated.width = -20.0;
        rotated.pivot_x = 0.0;
        rotated.rotation_degrees = 90.0;
        state.register(rotated);
        state.begin_frame();

        assert!(state.region_hovered("rotated", [-10.0, -10.0]));
        assert!(!state.region_hovered("rotated", [10.0, 10.0]));
    }

    #[test]
    fn beginning_one_surface_does_not_advance_another_surface() {
        let first_state = crate::renderer::new_shared_render_state();
        let second_state = crate::renderer::new_shared_render_state();
        let first = crate::renderer::interaction_surface_id(&first_state);
        let second = crate::renderer::interaction_surface_id(&second_state);

        register(first, region("first", "first", 0.0, Layer::Content));
        register(second, region("second", "second", 0.0, Layer::Content));

        begin_frame(first);
        begin_frame(second);

        assert!(region_hovered(first, "first", [10.0, 10.0]));
        assert!(region_hovered(second, "second", [10.0, 10.0]));
    }

    #[test]
    fn dropping_render_state_removes_its_surface() {
        let render_state = crate::renderer::new_shared_render_state();
        let surface = crate::renderer::interaction_surface_id(&render_state);
        register(
            surface,
            region("temporary", "temporary", 0.0, Layer::Content),
        );
        assert!(has_surface(surface));

        drop(render_state);

        assert!(!has_surface(surface));
    }
}
