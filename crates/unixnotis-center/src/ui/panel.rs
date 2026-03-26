//! Panel layout and widget construction for the center window.

use gtk::gdk;
use gtk::gdk::prelude::*;
use gtk::prelude::*;
use gtk::Align;
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use unixnotis_core::{
    Anchor, Config, Margins, PanelKeyboardInteractivity, PANEL_RUNTIME_WIDTH_MIN,
};

// Keep panel width reasonable on narrow displays to avoid dominating screen real estate.
const PANEL_WIDTH_MONITOR_RATIO_CAP: f32 = 0.32;
// Width floor keeps controls readable when monitor geometry is very small.
const PANEL_WIDTH_MIN: i32 = PANEL_RUNTIME_WIDTH_MIN;

/// GTK widgets backing the notification center panel window.
pub struct PanelWidgets {
    pub window: gtk::ApplicationWindow,
    pub root: gtk::Box,
    pub widget_revealer: gtk::Revealer,
    pub quick_controls: gtk::Box,
    pub toggle_container: gtk::Box,
    pub stat_container: gtk::Box,
    pub card_container: gtk::Box,
    pub scroller: gtk::ScrolledWindow,
    pub media_container: gtk::Box,
    pub search_revealer: gtk::Revealer,
    pub search_entry: gtk::SearchEntry,
    pub search_toggle: gtk::ToggleButton,
    pub header_count: gtk::Label,
    pub focus_toggle: gtk::ToggleButton,
    pub dnd_toggle: gtk::ToggleButton,
    pub clear_button: gtk::Button,
    pub close_button: gtk::Button,
}

pub fn build_panel_widgets(app: &gtk::Application, config: &Config) -> PanelWidgets {
    let window = gtk::ApplicationWindow::new(app);
    window.set_decorated(false);
    window.set_resizable(false);
    window.set_title(Some("UnixNotis Center"));
    window.add_css_class("unixnotis-panel-window");
    if let Some(settings) = gtk::Settings::default() {
        // GTK global setting that controls whether scrollbars overlay content.
        // Enabled here to keep scrollbar behavior consistent across widgets.
        settings.set_property("gtk-overlay-scrolling", true);
    }

    window.init_layer_shell();
    window.set_namespace(Some("unixnotis-panel"));
    window.set_layer(Layer::Overlay);
    apply_anchor(&window, config.panel.anchor, config.panel.margin);
    window.set_exclusive_zone(0);
    window.set_keyboard_mode(map_keyboard_mode(config.panel.keyboard_interactivity));

    let monitor = if let Some(output) = config.panel.output.as_ref() {
        find_monitor(output).or_else(default_monitor)
    } else {
        default_monitor()
    };
    if let Some(monitor) = monitor.as_ref() {
        window.set_monitor(Some(monitor));
    }

    let (width, height) = resolve_panel_size(config, monitor.as_ref(), None);
    window.set_default_size(width, height);
    if height > 0 {
        window.set_size_request(width, height);
    } else {
        window.set_size_request(width, -1);
    }

    let root = gtk::Box::new(gtk::Orientation::Vertical, 12);
    root.add_css_class("unixnotis-panel");
    root.set_focusable(true);
    root.set_hexpand(true);
    root.set_vexpand(true);
    // Keep the panel width stable regardless of child content.
    root.set_size_request(width, -1);

    let header = gtk::Box::new(gtk::Orientation::Vertical, 8);
    header.add_css_class("unixnotis-panel-header");
    // Top row stays minimal so horizontal width remains stable across themes.
    let header_top = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    header_top.add_css_class("unixnotis-panel-header-top");

    let title_box = gtk::Box::new(gtk::Orientation::Vertical, 2);
    title_box.add_css_class("unixnotis-panel-title-stack");
    let title = gtk::Label::new(Some("Notifications"));
    title.set_xalign(0.0);
    title.add_css_class("unixnotis-panel-title");
    let count = gtk::Label::new(Some("0"));
    count.set_xalign(0.5);
    count.set_valign(Align::Center);
    count.add_css_class("unixnotis-panel-count");
    let title_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    title_row.add_css_class("unixnotis-panel-title-row");
    title_row.append(&title);
    title_row.append(&count);
    title_box.append(&title_row);

    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    actions.add_css_class("unixnotis-panel-actions");
    // Action row is separated from the title row to avoid widening the panel.
    let action_primary = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    action_primary.add_css_class("unixnotis-panel-action-group");

    let focus_toggle = gtk::ToggleButton::new();
    focus_toggle.add_css_class("unixnotis-panel-action");
    focus_toggle.add_css_class("unixnotis-panel-action-focus");
    focus_toggle.add_css_class("unixnotis-panel-action-with-icon");
    focus_toggle.set_tooltip_text(Some("Toggle widget visibility"));
    let focus_content = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    focus_content.add_css_class("unixnotis-panel-action-content");
    let focus_icon = gtk::Image::from_icon_name("applications-system-symbolic");
    focus_icon.add_css_class("unixnotis-panel-action-glyph");
    let focus_label = gtk::Label::new(Some("Widgets"));
    focus_label.add_css_class("unixnotis-panel-action-label");
    focus_content.append(&focus_icon);
    focus_content.append(&focus_label);
    focus_toggle.set_child(Some(&focus_content));

    let dnd_toggle = gtk::ToggleButton::new();
    dnd_toggle.add_css_class("unixnotis-panel-action");
    dnd_toggle.add_css_class("unixnotis-panel-action-primary");
    dnd_toggle.add_css_class("unixnotis-panel-action-with-icon");
    dnd_toggle.set_tooltip_text(Some("Silence incoming notifications"));
    let dnd_content = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    dnd_content.add_css_class("unixnotis-panel-action-content");
    let dnd_icon = gtk::Image::from_icon_name("weather-clear-night-symbolic");
    dnd_icon.add_css_class("unixnotis-panel-action-glyph");
    let dnd_label = gtk::Label::new(Some("DND"));
    dnd_label.add_css_class("unixnotis-panel-action-label");
    dnd_content.append(&dnd_icon);
    dnd_content.append(&dnd_label);
    dnd_toggle.set_child(Some(&dnd_content));

    let clear_button = gtk::Button::new();
    clear_button.add_css_class("unixnotis-panel-action");
    clear_button.add_css_class("unixnotis-panel-action-muted");
    clear_button.add_css_class("unixnotis-panel-action-with-icon");
    clear_button.set_tooltip_text(Some("Clear all notifications"));
    let clear_content = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    clear_content.add_css_class("unixnotis-panel-action-content");
    let clear_icon = gtk::Image::from_icon_name("user-trash-symbolic");
    clear_icon.add_css_class("unixnotis-panel-action-glyph");
    let clear_label = gtk::Label::new(Some("Clear"));
    clear_label.add_css_class("unixnotis-panel-action-label");
    clear_content.append(&clear_icon);
    clear_content.append(&clear_label);
    clear_button.set_child(Some(&clear_content));

    let search_toggle = gtk::ToggleButton::new();
    search_toggle.add_css_class("unixnotis-panel-action");
    search_toggle.add_css_class("unixnotis-panel-action-search");
    search_toggle.add_css_class("unixnotis-panel-action-icon");
    search_toggle.set_tooltip_text(Some("Toggle search"));
    let search_icon = gtk::Image::from_icon_name("system-search-symbolic");
    search_icon.add_css_class("unixnotis-panel-action-glyph");
    search_toggle.set_child(Some(&search_icon));

    let close_button = gtk::Button::from_icon_name("window-close-symbolic");
    close_button.add_css_class("unixnotis-panel-action");
    close_button.add_css_class("unixnotis-panel-action-icon");
    close_button.add_css_class("unixnotis-panel-action-close");
    close_button.set_tooltip_text(Some("Close panel"));

    action_primary.append(&focus_toggle);
    action_primary.append(&dnd_toggle);
    action_primary.append(&clear_button);
    action_primary.append(&search_toggle);
    actions.append(&action_primary);

    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 1);
    spacer.set_hexpand(true);
    // Spacer expands to align actions to the trailing edge.
    header_top.append(&title_box);
    header_top.append(&spacer);
    // Keep close action isolated from destructive controls like "Clear".
    header_top.append(&close_button);
    header.append(&header_top);
    // Action controls are placed on a dedicated row to keep panel width stable.
    header.append(&actions);

    let search_entry = gtk::SearchEntry::new();
    search_entry.add_css_class("unixnotis-panel-search");
    search_entry.set_placeholder_text(Some("Search app, title, or message"));
    search_entry.set_hexpand(true);
    search_entry.set_tooltip_text(Some("Type to filter notifications"));
    let search_revealer = gtk::Revealer::new();
    search_revealer.add_css_class("unixnotis-panel-search-revealer");
    search_revealer.set_transition_type(gtk::RevealerTransitionType::SlideDown);
    search_revealer.set_transition_duration(180);
    // Keep search hidden by default to preserve notification space until requested.
    search_revealer.set_reveal_child(false);
    search_revealer.set_child(Some(&search_entry));
    header.append(&search_revealer);

    let media_container = gtk::Box::new(gtk::Orientation::Vertical, 8);
    media_container.add_css_class("unixnotis-media-container");
    media_container.set_hexpand(true);
    media_container.set_halign(Align::Fill);

    let quick_controls = gtk::Box::new(gtk::Orientation::Vertical, 10);
    quick_controls.add_css_class("unixnotis-quick-controls");

    let toggle_container = gtk::Box::new(gtk::Orientation::Vertical, 0);
    toggle_container.add_css_class("unixnotis-toggle-section");
    toggle_container.set_hexpand(true);
    toggle_container.set_visible(false);

    let stat_container = gtk::Box::new(gtk::Orientation::Vertical, 0);
    stat_container.add_css_class("unixnotis-stat-section");
    stat_container.set_hexpand(true);
    stat_container.set_visible(false);

    let card_container = gtk::Box::new(gtk::Orientation::Vertical, 0);
    card_container.add_css_class("unixnotis-card-section");
    card_container.set_hexpand(true);
    card_container.set_visible(false);

    let widget_stack = gtk::Box::new(gtk::Orientation::Vertical, 8);
    widget_stack.add_css_class("unixnotis-widget-stack");
    widget_stack.append(&quick_controls);
    widget_stack.append(&media_container);
    widget_stack.append(&toggle_container);
    widget_stack.append(&stat_container);
    widget_stack.append(&card_container);

    let widget_revealer = gtk::Revealer::new();
    widget_revealer.add_css_class("unixnotis-widget-revealer");
    widget_revealer.set_transition_type(gtk::RevealerTransitionType::SlideDown);
    widget_revealer.set_transition_duration(180);
    widget_revealer.set_reveal_child(true);
    // Widget stack remains mounted so collapse/expand does not rebuild child state.
    widget_revealer.set_child(Some(&widget_stack));

    let scroller = gtk::ScrolledWindow::new();
    scroller.set_vexpand(true);
    scroller.set_hexpand(true);
    // Keep vertical scrollbars allocated to avoid width jitter on hover.
    // Horizontal scrolling remains disabled because the panel is fixed-width.
    scroller.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Always);
    // Disable overlay scrolling so the scrollbar width stays constant.
    // The panel layout relies on a fixed width; overlay scrollbars can shift content.
    scroller.set_overlay_scrolling(false);
    scroller.set_min_content_width(width);
    scroller.set_max_content_width(width);

    root.append(&header);
    root.append(&widget_revealer);
    root.append(&scroller);

    window.set_child(Some(&root));
    window.set_visible(false);

    PanelWidgets {
        window,
        root,
        widget_revealer,
        quick_controls,
        toggle_container,
        stat_container,
        card_container,
        scroller,
        media_container,
        search_revealer,
        search_entry,
        search_toggle,
        header_count: count,
        focus_toggle,
        dnd_toggle,
        clear_button,
        close_button,
    }
}

fn resolve_panel_size(
    config: &Config,
    monitor: Option<&gdk::Monitor>,
    reserved: Option<Margins>,
) -> (i32, i32) {
    // Width is constrained by monitor geometry so defaults stay usable on laptops
    let width = resolve_panel_width(config, monitor);
    let height = resolve_panel_height(config, monitor, reserved).unwrap_or(-1);
    (width, height)
}

fn resolve_panel_width(config: &Config, monitor: Option<&gdk::Monitor>) -> i32 {
    let requested = config.panel.width.max(1);
    let Some(monitor) = monitor else {
        return requested;
    };
    let geometry = monitor.geometry();
    let available = geometry.width() - (config.panel.margin.left + config.panel.margin.right);
    if available <= 0 {
        return requested;
    }
    // Ratio cap prevents oversized side panels on compact displays.
    let ratio_cap = ((available as f32) * PANEL_WIDTH_MONITOR_RATIO_CAP).round() as i32;
    let max_width = available.max(1);
    let min_width = PANEL_WIDTH_MIN.min(max_width);
    let bounded_cap = ratio_cap.clamp(min_width, max_width);
    requested.min(bounded_cap).max(1)
}

fn resolve_panel_height(
    config: &Config,
    monitor: Option<&gdk::Monitor>,
    reserved: Option<Margins>,
) -> Option<i32> {
    let usable_height = usable_panel_height(config, monitor, reserved);
    if let Some(height_override) = config.panel.height_override {
        // Pixel override is still bounded by the current monitor work area
        return Some(
            usable_height
                .map(|usable| height_override.min(usable))
                .unwrap_or(height_override)
                .max(1),
        );
    }
    let usable_height = usable_height?;
    Some(height_from_percent(usable_height, config.panel.height))
}

fn usable_panel_height(
    config: &Config,
    monitor: Option<&gdk::Monitor>,
    reserved: Option<Margins>,
) -> Option<i32> {
    let monitor = monitor?;
    let geometry = monitor.geometry();
    let mut usable = geometry.height() - (config.panel.margin.top + config.panel.margin.bottom);
    if config.panel.respect_work_area {
        if let Some(reserved) = reserved {
            usable -= reserved.top + reserved.bottom;
        }
    }
    (usable > 0).then_some(usable)
}

fn height_from_percent(usable_height: i32, percent: i32) -> i32 {
    let usable_height = usable_height.max(1);
    let percent = percent.clamp(1, 100);
    let scaled = (i64::from(usable_height) * i64::from(percent) + 50) / 100;
    i32::try_from(scaled).unwrap_or(i32::MAX).max(1)
}

fn default_monitor() -> Option<gdk::Monitor> {
    let display = gdk::Display::default()?;
    let monitors = display.monitors();
    let mut best: Option<gdk::Monitor> = None;
    let mut best_area = 0i64;

    // Pick the largest monitor as a reasonable default when no primary API is available.
    for index in 0..monitors.n_items() {
        let Some(item) = monitors.item(index) else {
            continue;
        };
        let Ok(monitor) = item.downcast::<gdk::Monitor>() else {
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

    // Fall back to the first enumerated monitor when discovery yields nothing.
    let item = monitors.item(0)?;
    item.downcast::<gdk::Monitor>().ok()
}

fn apply_anchor(window: &impl IsA<gtk::Window>, anchor: Anchor, margin: Margins) {
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
            // Avoid bottom anchoring so computed height and overrides are respected.
        }
        Anchor::Right => {
            window.set_anchor(Edge::Right, true);
            window.set_anchor(Edge::Top, true);
            // Avoid bottom anchoring so computed height and overrides are respected.
        }
    }

    window.set_margin(Edge::Top, margin.top);
    window.set_margin(Edge::Right, margin.right);
    window.set_margin(Edge::Bottom, margin.bottom);
    window.set_margin(Edge::Left, margin.left);
}

pub fn apply_panel_config(panel: &PanelWidgets, config: &Config, reserved: Option<Margins>) {
    let monitor = if let Some(output) = config.panel.output.as_ref() {
        find_monitor(output).or_else(default_monitor)
    } else {
        default_monitor()
    };
    if let Some(monitor) = monitor.as_ref() {
        panel.window.set_monitor(Some(monitor));
    }

    panel
        .window
        .set_keyboard_mode(map_keyboard_mode(config.panel.keyboard_interactivity));
    apply_anchor(&panel.window, config.panel.anchor, config.panel.margin);

    let (width, height) = resolve_panel_size(config, monitor.as_ref(), reserved);
    panel.window.set_default_size(width, height);
    if height > 0 {
        panel.window.set_size_request(width, height);
    } else {
        panel.window.set_size_request(width, -1);
    }
    panel.root.set_size_request(width, -1);
    panel.scroller.set_min_content_width(width);
    panel.scroller.set_max_content_width(width);
}

fn map_keyboard_mode(mode: PanelKeyboardInteractivity) -> KeyboardMode {
    match mode {
        PanelKeyboardInteractivity::None => KeyboardMode::None,
        PanelKeyboardInteractivity::OnDemand => KeyboardMode::OnDemand,
        PanelKeyboardInteractivity::Exclusive => KeyboardMode::Exclusive,
    }
}

fn find_monitor(output: &str) -> Option<gdk::Monitor> {
    let display = gdk::Display::default()?;
    let monitors = display.monitors();
    for index in 0..monitors.n_items() {
        let Some(item) = monitors.item(index) else {
            continue;
        };
        let Ok(monitor) = item.downcast::<gdk::Monitor>() else {
            continue;
        };
        if monitor_matches_output(&monitor, output) {
            return Some(monitor);
        }
    }
    None
}

fn monitor_matches_output(monitor: &gdk::Monitor, output: &str) -> bool {
    let output = output.trim();
    if output.is_empty() {
        return false;
    }

    let connector = monitor
        .connector()
        .map(|value| value.to_string())
        .unwrap_or_default();
    if !connector.is_empty() && connector.eq_ignore_ascii_case(output) {
        return true;
    }

    let model = monitor
        .model()
        .map(|value| value.to_string())
        .unwrap_or_default();
    if !model.is_empty() && model.eq_ignore_ascii_case(output) {
        return true;
    }

    let manufacturer = monitor
        .manufacturer()
        .map(|value| value.to_string())
        .unwrap_or_default();
    let joined = if manufacturer.is_empty() {
        model
    } else if model.is_empty() {
        manufacturer
    } else {
        format!("{manufacturer} {model}")
    };

    !joined.is_empty() && joined.eq_ignore_ascii_case(output)
}

#[cfg(test)]
mod tests {
    use super::height_from_percent;

    #[test]
    fn height_from_percent_scales_usable_height() {
        assert_eq!(height_from_percent(1000, 84), 840);
        assert_eq!(height_from_percent(701, 84), 589);
    }

    #[test]
    fn height_from_percent_keeps_a_positive_floor() {
        assert_eq!(height_from_percent(1, 1), 1);
        assert_eq!(height_from_percent(40, 1), 1);
    }
}
