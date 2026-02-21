//! Anchor and margin helpers for popup layer-shell windows

use gtk::prelude::*;
use gtk4_layer_shell::{Edge, LayerShell};
use unixnotis_core::{Anchor, Margins};

pub(super) fn apply_anchor(window: &impl IsA<gtk::Window>, anchor: Anchor, margin: Margins) {
    // Reset all edges first so anchor changes never leave stale flags behind
    for edge in [Edge::Top, Edge::Right, Edge::Bottom, Edge::Left] {
        window.set_anchor(edge, false);
    }

    match anchor {
        Anchor::TopRight => {
            // Corner anchor for default popup placement
            window.set_anchor(Edge::Top, true);
            window.set_anchor(Edge::Right, true);
        }
        Anchor::TopLeft => {
            // Mirrored corner anchor
            window.set_anchor(Edge::Top, true);
            window.set_anchor(Edge::Left, true);
        }
        Anchor::BottomRight => {
            // Bottom-right pile for dock-style layouts
            window.set_anchor(Edge::Bottom, true);
            window.set_anchor(Edge::Right, true);
        }
        Anchor::BottomLeft => {
            // Bottom-left mirror of default dock-style placement
            window.set_anchor(Edge::Bottom, true);
            window.set_anchor(Edge::Left, true);
        }
        Anchor::Top => {
            // Stretch horizontally while pinned to top edge
            window.set_anchor(Edge::Top, true);
            window.set_anchor(Edge::Left, true);
            window.set_anchor(Edge::Right, true);
        }
        Anchor::Bottom => {
            // Stretch horizontally while pinned to bottom edge
            window.set_anchor(Edge::Bottom, true);
            window.set_anchor(Edge::Left, true);
            window.set_anchor(Edge::Right, true);
        }
        Anchor::Left => {
            // Vertical rail on left edge
            window.set_anchor(Edge::Left, true);
            window.set_anchor(Edge::Top, true);
            window.set_anchor(Edge::Bottom, true);
        }
        Anchor::Right => {
            // Vertical rail on right edge
            window.set_anchor(Edge::Right, true);
            window.set_anchor(Edge::Top, true);
            window.set_anchor(Edge::Bottom, true);
        }
    }

    // Margins are always applied after anchor selection
    window.set_margin(Edge::Top, margin.top);
    window.set_margin(Edge::Right, margin.right);
    window.set_margin(Edge::Bottom, margin.bottom);
    window.set_margin(Edge::Left, margin.left);
}
