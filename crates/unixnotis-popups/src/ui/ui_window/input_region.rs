//! Popup input-region shaping and animation tracking
//!
//! This module keeps pointer hit-region behavior independent from window layout wiring

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::cairo;
use gtk::glib::ControlFlow;
use gtk::prelude::*;

#[derive(Clone)]
pub(in super::super) struct PopupInputRegionState {
    // Runtime toggle for display-only passthrough mode
    allow_click_through: Rc<Cell<bool>>,
    // Dirty bit avoids rebuilding the region when nothing changed
    dirty: Rc<Cell<bool>>,
    // Guard prevents duplicate tick callbacks
    ticking: Rc<Cell<bool>>,
    // Last applied signature skips no-op compositor region updates
    last_signature: Rc<RefCell<Option<InputRegionSignature>>>,
}

#[derive(Clone, PartialEq, Eq)]
struct InputRegionSignature {
    // Surface dimensions are part of identity so scale/output shifts are detected
    surface_width: i32,
    surface_height: i32,
    // Ordered source rectangles represent clickable popup regions
    reactive_rects: Vec<cairo::RectangleInt>,
}

impl PopupInputRegionState {
    pub(super) fn new(allow_click_through: bool) -> Self {
        // New state starts dirty so first map applies a region immediately
        Self {
            allow_click_through: Rc::new(Cell::new(allow_click_through)),
            dirty: Rc::new(Cell::new(true)),
            ticking: Rc::new(Cell::new(false)),
            last_signature: Rc::new(RefCell::new(None)),
        }
    }

    pub(super) fn set_allow_click_through(&self, allow_click_through: bool) {
        // Only invalidate when mode actually changes
        if self.allow_click_through.replace(allow_click_through) != allow_click_through {
            self.mark_dirty();
        }
    }

    fn allow_click_through(&self) -> bool {
        self.allow_click_through.get()
    }

    fn mark_dirty(&self) {
        // Dirty marks request a full rebuild on next apply pass
        self.dirty.set(true);
    }

    fn take_dirty(&self) -> bool {
        self.dirty.replace(false)
    }
}

pub(in super::super) fn refresh_popup_input_region(
    window: &gtk::ApplicationWindow,
    stack: &gtk::Box,
    input_region: &PopupInputRegionState,
    keep_ticking: bool,
) {
    // Any caller here observed a geometry or visibility change
    input_region.mark_dirty();
    let needs_retry = apply_popup_input_region_if_dirty(window, stack, input_region);

    // Keep ticking only during animations or when geometry is still settling
    if window.is_visible() && (keep_ticking || needs_retry) {
        // Tick callback self-terminates once transitions and retries are complete
        ensure_popup_input_region_tick(window, stack, input_region);
    }
}

pub(in super::super) fn popup_stack_has_active_transitions(stack: &gtk::Box) -> bool {
    let mut child = stack.first_child();
    while let Some(widget) = child {
        let next = widget.next_sibling();

        // Revealers animate between these two states
        if let Ok(revealer) = widget.clone().downcast::<gtk::Revealer>() {
            // Transition is active while target and current child reveal states differ
            if revealer.reveals_child() != revealer.is_child_revealed() {
                return true;
            }
        }

        child = next;
    }
    false
}

fn ensure_popup_input_region_tick(
    window: &gtk::ApplicationWindow,
    stack: &gtk::Box,
    input_region: &PopupInputRegionState,
) {
    // Avoid duplicate callback loops when many updates arrive in one frame
    if input_region.ticking.replace(true) {
        return;
    }

    let stack = stack.clone();
    let input_region = input_region.clone();

    window.add_tick_callback(move |window, _| {
        // Animation-aware refresh keeps hitboxes aligned with revealer motion
        let active_transitions = popup_stack_has_active_transitions(&stack);
        if active_transitions {
            // Animated revealers shift geometry frame-by-frame
            input_region.mark_dirty();
        }

        let needs_retry = apply_popup_input_region_if_dirty(window, &stack, &input_region);
        if active_transitions || needs_retry {
            ControlFlow::Continue
        } else {
            // Reset guard so future animations can re-arm the tick callback
            input_region.ticking.set(false);
            ControlFlow::Break
        }
    });
}

fn apply_popup_input_region_if_dirty(
    window: &gtk::ApplicationWindow,
    stack: &gtk::Box,
    input_region: &PopupInputRegionState,
) -> bool {
    // Skip work when no geometry or visibility changes have been observed
    if !input_region.take_dirty() {
        return false;
    }

    let Some(surface) = window.surface() else {
        // Surface can be unavailable very early in lifecycle
        input_region.mark_dirty();
        return true;
    };

    let surface_width = surface.width().max(0);
    let surface_height = surface.height().max(0);

    // Signature includes surface size so monitor/scale changes are detected
    let (region, signature, visible_widgets) = if input_region.allow_click_through() {
        (
            cairo::Region::create(),
            InputRegionSignature {
                surface_width,
                surface_height,
                reactive_rects: Vec::new(),
            },
            0,
        )
    } else {
        build_popup_input_region(window, stack, surface_width, surface_height)
    };

    // Visible children with no rectangles usually means allocation is still in flight
    let needs_layout_retry = !input_region.allow_click_through()
        && visible_widgets > 0
        && signature.reactive_rects.is_empty();
    if needs_layout_retry {
        // Retry next frame when widgets are visible but bounds have not landed yet
        input_region.mark_dirty();
    }

    let unchanged = input_region
        .last_signature
        .borrow()
        .as_ref()
        .is_some_and(|prev| *prev == signature);
    if unchanged {
        // No compositor call is needed when geometry signature did not move
        return needs_layout_retry;
    }

    // Apply the new union region so empty overlay areas stay click-through
    surface.set_input_region(&region);
    *input_region.last_signature.borrow_mut() = Some(signature);
    needs_layout_retry
}

fn build_popup_input_region(
    window: &gtk::ApplicationWindow,
    stack: &gtk::Box,
    surface_width: i32,
    surface_height: i32,
) -> (cairo::Region, InputRegionSignature, usize) {
    // Region starts empty and is expanded by visible child rectangles
    let region = cairo::Region::create();
    let mut reactive_rects = Vec::new();
    let mut visible_widgets = 0usize;

    let mut child = stack.first_child();
    while let Some(widget) = child {
        let next = widget.next_sibling();
        if widget.is_visible() {
            visible_widgets += 1;
            // Only currently visible popup widgets should capture input
            union_widget_bounds(&region, &widget, window, &mut reactive_rects);
        }
        // Capture next first to stay robust if current node gets detached
        child = next;
    }

    // Keep controls interactive while first-frame allocations are still settling
    if visible_widgets > 0 && reactive_rects.is_empty() {
        if let Some(rect) = widget_rect_in_window(stack.upcast_ref(), window) {
            let _ = region.union_rectangle(&rect);
            reactive_rects.push(rect);
        }
    }

    (
        region,
        InputRegionSignature {
            surface_width,
            surface_height,
            reactive_rects,
        },
        visible_widgets,
    )
}

fn union_widget_bounds(
    region: &cairo::Region,
    widget: &gtk::Widget,
    window: &gtk::ApplicationWindow,
    reactive_rects: &mut Vec<cairo::RectangleInt>,
) {
    let Some(rect) = widget_rect_in_window(widget, window) else {
        // Geometry may be temporarily unavailable during lifecycle transitions
        return;
    };

    // Unioned rectangles produce a single compositor input region
    let _ = region.union_rectangle(&rect);
    // Raw rectangles are retained for stable signature comparisons
    reactive_rects.push(rect);
}

fn widget_rect_in_window(
    widget: &gtk::Widget,
    window: &gtk::ApplicationWindow,
) -> Option<cairo::RectangleInt> {
    // Allocation provides stable logical size once widget measurement is done
    let alloc = widget.allocation();
    let width = alloc.width();
    let height = alloc.height();
    if width <= 0 || height <= 0 {
        // Hidden or not-yet-sized widgets cannot contribute valid hit boxes
        return None;
    }

    // Prefer translate_coordinates because it directly maps widget origin to window space
    let translated = widget
        .translate_coordinates(window, 0.0, 0.0)
        .map(|(x, y)| {
            (
                clamp_floor_nonneg(x),
                clamp_floor_nonneg(y),
                clamp_ceil_nonneg(x + f64::from(width)),
                clamp_ceil_nonneg(y + f64::from(height)),
            )
        });

    let (x0, y0, x1, y1) = if let Some(values) = translated {
        values
    } else {
        // Fallback covers transient coordinate-mapping failures
        let bounds = widget.compute_bounds(window)?;
        (
            clamp_floor_nonneg(f64::from(bounds.x())),
            clamp_floor_nonneg(f64::from(bounds.y())),
            clamp_ceil_nonneg(f64::from(bounds.x() + bounds.width())),
            clamp_ceil_nonneg(f64::from(bounds.y() + bounds.height())),
        )
    };

    if x1 <= x0 || y1 <= y0 {
        // Guard against degenerate coordinates from transient layout states
        return None;
    }

    Some(cairo::RectangleInt::new(x0, y0, x1 - x0, y1 - y0))
}

fn clamp_floor_nonneg(value: f64) -> i32 {
    if !value.is_finite() {
        // Defensive clamp for NaN and infinities
        return 0;
    }
    // Floor keeps origin inside widget bounds while clamping negative drift
    value.floor().clamp(0.0, f64::from(i32::MAX)) as i32
}

fn clamp_ceil_nonneg(value: f64) -> i32 {
    if !value.is_finite() {
        // Defensive clamp for NaN and infinities
        return 0;
    }
    // Ceil keeps width and height inclusive of fractional trailing edges
    value.ceil().clamp(0.0, f64::from(i32::MAX)) as i32
}
