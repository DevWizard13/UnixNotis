//! Window construction and layout helpers for the popup surface
//!
//! Keeps layout configuration isolated from popup state logic

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::cairo;
use gtk::glib::ControlFlow;
use gtk::prelude::*;
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use unixnotis_core::{Anchor, Config, Margins};

#[derive(Clone)]
pub(super) struct PopupInputRegionState {
    // Runtime toggle for click passthrough mode
    allow_click_through: Rc<Cell<bool>>,
    // Dirty bit avoids recomputing region when nothing changed
    dirty: Rc<Cell<bool>>,
    // Guard to keep only one tick callback alive
    ticking: Rc<Cell<bool>>,
    // Cache of last applied shape to skip no-op surface updates
    last_signature: Rc<RefCell<Option<InputRegionSignature>>>,
}

#[derive(Clone, PartialEq, Eq)]
struct InputRegionSignature {
    surface_width: i32,
    surface_height: i32,
    reactive_rects: Vec<cairo::RectangleInt>,
}

impl PopupInputRegionState {
    fn new(allow_click_through: bool) -> Self {
        Self {
            allow_click_through: Rc::new(Cell::new(allow_click_through)),
            dirty: Rc::new(Cell::new(true)),
            ticking: Rc::new(Cell::new(false)),
            last_signature: Rc::new(RefCell::new(None)),
        }
    }

    pub(super) fn set_allow_click_through(&self, allow_click_through: bool) {
        // Only mark dirty when the mode actually flips
        if self.allow_click_through.replace(allow_click_through) != allow_click_through {
            self.mark_dirty();
        }
    }

    fn allow_click_through(&self) -> bool {
        self.allow_click_through.get()
    }

    pub(super) fn mark_dirty(&self) {
        self.dirty.set(true);
    }

    fn take_dirty(&self) -> bool {
        self.dirty.replace(false)
    }
}

pub(super) fn build_popup_window(
    app: &gtk::Application,
    config: &Config,
) -> (gtk::ApplicationWindow, gtk::Box, PopupInputRegionState) {
    let window = gtk::ApplicationWindow::new(app);
    window.set_decorated(false);
    window.set_resizable(false);
    window.set_title(Some("UnixNotis Popups"));
    window.add_css_class("unixnotis-popup-window");

    window.init_layer_shell();
    window.set_namespace(Some("unixnotis-popups"));
    window.set_layer(Layer::Overlay);

    let stack = gtk::Box::new(gtk::Orientation::Vertical, config.popups.spacing);
    stack.add_css_class("unixnotis-popup-stack");
    window.set_child(Some(&stack));
    window.set_visible(false);

    // Shared region state is reused by layout, popup updates, and config reload
    let input_region = PopupInputRegionState::new(config.popups.allow_click_through);
    apply_popup_config(&window, &stack, config, &input_region);

    window.connect_realize({
        let stack = stack.clone();
        let input_region = input_region.clone();
        move |window| {
            // Realize is the first safe time to touch the surface region
            refresh_popup_input_region(
                window,
                &stack,
                &input_region,
                popup_stack_has_active_transitions(&stack),
            );
        }
    });
    window.connect_map({
        let stack = stack.clone();
        let input_region = input_region.clone();
        move |window| {
            // Map can change surface geometry after realize
            refresh_popup_input_region(
                window,
                &stack,
                &input_region,
                popup_stack_has_active_transitions(&stack),
            );
        }
    });
    window.connect_scale_factor_notify({
        let stack = stack.clone();
        let input_region = input_region.clone();
        move |window| {
            // Scale changes shift pixel geometry so region must be rebuilt
            refresh_popup_input_region(
                window,
                &stack,
                &input_region,
                popup_stack_has_active_transitions(&stack),
            );
        }
    });

    (window, stack, input_region)
}

pub(super) fn apply_popup_config(
    window: &gtk::ApplicationWindow,
    stack: &gtk::Box,
    config: &Config,
    input_region: &PopupInputRegionState,
) {
    window.set_default_size(config.popups.width, 1);
    window.set_size_request(config.popups.width, -1);
    stack.set_spacing(config.popups.spacing);

    apply_anchor(window, config.popups.anchor, config.popups.margin);
    window.set_exclusive_zone(0);
    window.set_keyboard_mode(KeyboardMode::None);

    let monitor = if let Some(output) = config.popups.output.as_ref() {
        find_monitor(output).or_else(default_monitor)
    } else {
        default_monitor()
    };
    if let Some(monitor) = monitor.as_ref() {
        window.set_monitor(Some(monitor));
    } else {
        window.set_monitor(None);
    }

    // Keep runtime mode in sync with latest config value
    input_region.set_allow_click_through(config.popups.allow_click_through);
    refresh_popup_input_region(
        window,
        stack,
        input_region,
        popup_stack_has_active_transitions(stack),
    );
}

pub(super) fn refresh_popup_input_region(
    window: &gtk::ApplicationWindow,
    stack: &gtk::Box,
    input_region: &PopupInputRegionState,
    keep_ticking: bool,
) {
    // Any caller here has observed a state or layout change
    input_region.mark_dirty();
    apply_popup_input_region_if_dirty(window, stack, input_region);
    // Tick only while transitions are active to avoid steady frame work
    if keep_ticking && window.is_visible() {
        ensure_popup_input_region_tick(window, stack, input_region);
    }
}

pub(super) fn popup_stack_has_active_transitions(stack: &gtk::Box) -> bool {
    let mut child = stack.first_child();
    while let Some(widget) = child {
        let next = widget.next_sibling();
        if let Ok(revealer) = widget.clone().downcast::<gtk::Revealer>() {
            // During reveal animation these values differ
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
    // Prevent duplicate tick loops when many events fire together
    if input_region.ticking.replace(true) {
        return;
    }

    let stack = stack.clone();
    let input_region = input_region.clone();
    window.add_tick_callback(move |window, _| {
        // Region may drift each frame while revealers animate
        let active_transitions = popup_stack_has_active_transitions(&stack);
        if active_transitions {
            input_region.mark_dirty();
        }
        apply_popup_input_region_if_dirty(window, &stack, &input_region);
        if active_transitions {
            ControlFlow::Continue
        } else {
            // Clear guard so future animations can start ticking again
            input_region.ticking.set(false);
            ControlFlow::Break
        }
    });
}

fn apply_popup_input_region_if_dirty(
    window: &gtk::ApplicationWindow,
    stack: &gtk::Box,
    input_region: &PopupInputRegionState,
) {
    // Fast exit when no invalidation was requested
    if !input_region.take_dirty() {
        return;
    }

    let Some(surface) = window.surface() else {
        // Surface might not exist yet during early startup
        // Keep dirty set so next map/realize pass retries
        input_region.mark_dirty();
        return;
    };

    let surface_width = surface.width().max(0);
    let surface_height = surface.height().max(0);
    let (region, signature) = if input_region.allow_click_through() {
        (
            cairo::Region::create(),
            InputRegionSignature {
                surface_width,
                surface_height,
                reactive_rects: Vec::new(),
            },
        )
    } else {
        build_popup_input_region(window, stack, surface_width, surface_height)
    };

    let unchanged = input_region
        .last_signature
        .borrow()
        .as_ref()
        .is_some_and(|prev| *prev == signature);
    if unchanged {
        // Skip compositor call when region shape is identical
        return;
    }

    surface.set_input_region(&region);
    *input_region.last_signature.borrow_mut() = Some(signature);
}

fn build_popup_input_region(
    window: &gtk::ApplicationWindow,
    stack: &gtk::Box,
    surface_width: i32,
    surface_height: i32,
) -> (cairo::Region, InputRegionSignature) {
    let region = cairo::Region::create();
    // Store source rects so signature can detect no-op rebuilds
    let mut reactive_rects = Vec::new();
    let mut child = stack.first_child();
    while let Some(widget) = child {
        let next = widget.next_sibling();
        if widget.is_visible() {
            // Only visible popup widgets should capture pointer input
            union_widget_bounds(
                &region,
                &widget,
                window,
                surface_width,
                surface_height,
                &mut reactive_rects,
            );
        }
        child = next;
    }

    (
        region,
        InputRegionSignature {
            surface_width,
            surface_height,
            reactive_rects,
        },
    )
}

fn union_widget_bounds(
    region: &cairo::Region,
    widget: &gtk::Widget,
    window: &gtk::ApplicationWindow,
    surface_width: i32,
    surface_height: i32,
    reactive_rects: &mut Vec<cairo::RectangleInt>,
) {
    let Some(bounds) = widget.compute_bounds(window) else {
        // Bounds can be unavailable briefly during widget lifecycle changes
        return;
    };

    let x0 = clamp_floor(bounds.x(), 0, surface_width);
    let y0 = clamp_floor(bounds.y(), 0, surface_height);
    let x1 = clamp_ceil(bounds.x() + bounds.width(), 0, surface_width);
    let y1 = clamp_ceil(bounds.y() + bounds.height(), 0, surface_height);
    if x1 <= x0 || y1 <= y0 {
        return;
    }

    let rect = cairo::RectangleInt::new(x0, y0, x1 - x0, y1 - y0);
    // Union keeps a single region for all interactive popup areas
    let _ = region.union_rectangle(&rect);
    // Cache raw rectangles for cheap signature comparison
    reactive_rects.push(rect);
}

fn clamp_floor(value: f32, min: i32, max: i32) -> i32 {
    if !value.is_finite() {
        // Defensive clamp for invalid geometry input
        return min;
    }
    let clamped = f64::from(value)
        .floor()
        .clamp(f64::from(min), f64::from(max));
    clamped as i32
}

fn clamp_ceil(value: f32, min: i32, max: i32) -> i32 {
    if !value.is_finite() {
        // Defensive clamp for invalid geometry input
        return min;
    }
    let clamped = f64::from(value)
        .ceil()
        .clamp(f64::from(min), f64::from(max));
    clamped as i32
}

fn apply_anchor(window: &impl IsA<gtk::Window>, anchor: Anchor, margin: Margins) {
    // Reset first so anchor switches do not leave stale edges enabled
    for edge in [Edge::Top, Edge::Right, Edge::Bottom, Edge::Left] {
        window.set_anchor(edge, false);
    }
    match anchor {
        Anchor::TopRight => {
            window.set_anchor(Edge::Top, true);
            window.set_anchor(Edge::Right, true);
        }
        Anchor::TopLeft => {
            window.set_anchor(Edge::Top, true);
            window.set_anchor(Edge::Left, true);
        }
        Anchor::BottomRight => {
            window.set_anchor(Edge::Bottom, true);
            window.set_anchor(Edge::Right, true);
        }
        Anchor::BottomLeft => {
            window.set_anchor(Edge::Bottom, true);
            window.set_anchor(Edge::Left, true);
        }
        Anchor::Top => {
            window.set_anchor(Edge::Top, true);
            window.set_anchor(Edge::Left, true);
            window.set_anchor(Edge::Right, true);
        }
        Anchor::Bottom => {
            window.set_anchor(Edge::Bottom, true);
            window.set_anchor(Edge::Left, true);
            window.set_anchor(Edge::Right, true);
        }
        Anchor::Left => {
            window.set_anchor(Edge::Left, true);
            window.set_anchor(Edge::Top, true);
            window.set_anchor(Edge::Bottom, true);
        }
        Anchor::Right => {
            window.set_anchor(Edge::Right, true);
            window.set_anchor(Edge::Top, true);
            window.set_anchor(Edge::Bottom, true);
        }
    }

    window.set_margin(Edge::Top, margin.top);
    window.set_margin(Edge::Right, margin.right);
    window.set_margin(Edge::Bottom, margin.bottom);
    window.set_margin(Edge::Left, margin.left);
}

fn find_monitor(output: &str) -> Option<gtk::gdk::Monitor> {
    let display = gtk::gdk::Display::default()?;
    let monitors = display.monitors();
    for index in 0..monitors.n_items() {
        let Some(item) = monitors.item(index) else {
            continue;
        };
        let Ok(monitor) = item.downcast::<gtk::gdk::Monitor>() else {
            continue;
        };
        if monitor_matches_output(&monitor, output) {
            return Some(monitor);
        }
    }
    None
}

fn monitor_matches_output(monitor: &gtk::gdk::Monitor, output: &str) -> bool {
    let output = output.trim();
    if output.is_empty() {
        return false;
    }

    // Prefer connector identifiers because they match compositor output names
    if monitor
        .connector()
        .as_deref()
        .is_some_and(|connector| connector.eq_ignore_ascii_case(output))
    {
        return true;
    }

    // Keep model matching for compatibility with existing configs
    monitor
        .model()
        .as_deref()
        .is_some_and(|model| model.eq_ignore_ascii_case(output))
}

fn default_monitor() -> Option<gtk::gdk::Monitor> {
    let display = gtk::gdk::Display::default()?;
    let monitors = display.monitors();
    let mut best: Option<gtk::gdk::Monitor> = None;
    let mut best_area = 0i64;

    // Pick the largest monitor as a reasonable default when no primary API is available
    for index in 0..monitors.n_items() {
        let Some(item) = monitors.item(index) else {
            continue;
        };
        let Ok(monitor) = item.downcast::<gtk::gdk::Monitor>() else {
            continue;
        };
        let geometry = monitor.geometry();
        let area = i64::from(geometry.width()) * i64::from(geometry.height());
        if area > best_area {
            best_area = area;
            best = Some(monitor);
        }
    }

    if best.is_some() {
        return best;
    }

    // Fall back to the first enumerated monitor when discovery yields nothing
    let item = monitors.item(0)?;
    item.downcast::<gtk::gdk::Monitor>().ok()
}
