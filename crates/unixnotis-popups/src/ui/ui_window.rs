//! Window construction and layout helpers for the popup surface
//!
//! This module keeps top-level window wiring compact while delegating
//! anchor, monitor selection, and input-region shaping to focused helpers

use gtk::prelude::*;
use gtk4_layer_shell::{KeyboardMode, Layer, LayerShell};
use unixnotis_core::Config;

mod anchor;
mod input_region;
mod monitor;

use self::anchor::apply_anchor;
pub(super) use self::input_region::{
    popup_stack_has_active_transitions, refresh_popup_input_region, PopupInputRegionState,
};
use self::monitor::{default_monitor, find_monitor};

pub(super) fn build_popup_window(
    app: &gtk::Application,
    config: &Config,
) -> (gtk::ApplicationWindow, gtk::Box, PopupInputRegionState) {
    // Window lifecycle hooks are centralized here to keep popup setup deterministic
    let window = gtk::ApplicationWindow::new(app);
    window.set_decorated(false);
    window.set_resizable(false);
    window.set_title(Some("UnixNotis Popups"));
    window.add_css_class("unixnotis-popup-window");

    // Layer-shell keeps popups above regular windows without traditional decorations
    window.init_layer_shell();
    window.set_namespace(Some("unixnotis-popups"));
    window.set_layer(Layer::Overlay);

    // Stack owns popup layout and reveal order for visible entries
    let stack = gtk::Box::new(gtk::Orientation::Vertical, config.popups.spacing);
    stack.add_css_class("unixnotis-popup-stack");
    window.set_child(Some(&stack));
    window.set_visible(false);

    // Shared input-region state is reused by config reloads and runtime visibility updates
    let input_region = PopupInputRegionState::new(config.popups.allow_click_through);
    apply_popup_config(&window, &stack, config, &input_region);

    window.connect_realize({
        let stack = stack.clone();
        let input_region = input_region.clone();
        move |window| {
            // Realize is the first safe point for surface input-region calls
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
            // Mapping can change surface geometry after realize
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
            // Scale changes alter pixel bounds so hit regions must be rebuilt
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
    // Width is fixed by config while height remains content-driven
    window.set_default_size(config.popups.width, 1);
    window.set_size_request(config.popups.width, -1);
    stack.set_spacing(config.popups.spacing);

    apply_anchor(window, config.popups.anchor, config.popups.margin);
    // Popup layer should not reserve workarea space
    window.set_exclusive_zone(0);
    // Keyboard focus stays with the underlying active window
    window.set_keyboard_mode(KeyboardMode::None);

    let monitor = if let Some(output) = config.popups.output.as_ref() {
        // Explicit output is attempted first
        find_monitor(output).or_else(default_monitor)
    } else {
        // Fallback monitor selection keeps behavior stable without explicit output
        default_monitor()
    };

    if let Some(monitor) = monitor.as_ref() {
        window.set_monitor(Some(monitor));
    } else {
        window.set_monitor(None);
    }

    // Apply passthrough mode changes immediately on config reload
    input_region.set_allow_click_through(config.popups.allow_click_through);
    refresh_popup_input_region(
        window,
        stack,
        input_region,
        popup_stack_has_active_transitions(stack),
    );
}
