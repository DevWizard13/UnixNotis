use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::Align;
use unixnotis_core::{MediaConfig, MediaLayout};

use crate::media::MediaHandle;

use super::card::MediaCardWidgets;
use super::layout::{
    card_height_for_layout, card_layout_class, marquee_width_for_layout, row_layout_class,
    stack_layout_class,
};
use super::parts::{build_media_card_parts, MediaCardLayoutParts};
use super::selection::MediaSelection;

pub(super) struct MediaWidgetParts {
    pub(super) root: gtk::Box,
    pub(super) nav_prev: gtk::Button,
    pub(super) nav_next: gtk::Button,
    pub(super) card: MediaCardWidgets,
}

pub(super) fn build_media_widget(
    handle: &MediaHandle,
    selection: Rc<RefCell<MediaSelection>>,
    panel_width: i32,
    config: &MediaConfig,
) -> MediaWidgetParts {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
    root.add_css_class("unixnotis-media-stack");
    root.add_css_class(stack_layout_class(config.layout));
    root.set_visible(false);

    let row = gtk::Box::new(row_orientation(config.layout), 6);
    row.add_css_class("unixnotis-media-row");
    row.add_css_class(row_layout_class(config.layout));
    row.set_hexpand(true);
    row.set_halign(Align::Fill);
    row.set_valign(Align::Center);

    let nav_prev = build_navigation_button("<");
    let nav_next = build_navigation_button(">");
    let marquee_width = marquee_width_for_layout(config.layout, panel_width);
    let parts = build_media_card_parts(handle, selection, marquee_width, config.title_char_limit);
    parts
        .card
        .root
        .add_css_class(card_layout_class(config.layout));
    // Fixed heights keep each preset steady while metadata changes underneath
    parts
        .card
        .root
        .set_size_request(-1, card_height_for_layout(config.layout));
    parts
        .card
        .apply_metadata_visibility(config.show_source, config.show_position);

    match config.layout {
        MediaLayout::Carousel => compose_carousel(&row, &nav_prev, &parts, &nav_next),
        MediaLayout::Inline => compose_inline(&row, &nav_prev, &parts, &nav_next),
        MediaLayout::Stacked => compose_stacked(&row, &nav_prev, &parts, &nav_next),
        MediaLayout::Showcase => compose_showcase(&row, &nav_prev, &parts, &nav_next),
    }

    root.append(&row);
    MediaWidgetParts {
        root,
        nav_prev,
        nav_next,
        card: parts.card,
    }
}

pub(super) fn build_navigation_button(label_text: &str) -> gtk::Button {
    let button = gtk::Button::with_label(label_text);
    button.add_css_class("unixnotis-media-nav");
    // Explicit centering keeps tiny glyph buttons aligned across themes
    button.set_halign(Align::Center);
    button.set_valign(Align::Center);
    if let Some(label) = button
        .child()
        .and_then(|child| child.downcast::<gtk::Label>().ok())
    {
        label.set_xalign(0.5);
        label.set_yalign(0.5);
    }
    button
}

fn row_orientation(layout: MediaLayout) -> gtk::Orientation {
    match layout {
        MediaLayout::Carousel => gtk::Orientation::Horizontal,
        MediaLayout::Inline | MediaLayout::Stacked | MediaLayout::Showcase => {
            gtk::Orientation::Vertical
        }
    }
}

fn compose_carousel(
    row: &gtk::Box,
    nav_prev: &gtk::Button,
    parts: &MediaCardLayoutParts,
    nav_next: &gtk::Button,
) {
    let main = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    main.add_css_class("unixnotis-media-main");
    main.set_hexpand(true);
    main.set_halign(Align::Fill);
    main.set_valign(Align::Center);

    parts
        .card
        .root
        .set_orientation(gtk::Orientation::Horizontal);
    parts.card.root.set_spacing(10);
    parts.card.root.append(&parts.art_frame);
    main.append(&parts.card.text_box);
    main.append(&parts.controls);
    parts.card.root.append(&main);

    // The carousel preset keeps player navigation outside the transport card
    row.append(nav_prev);
    row.append(&parts.card.root);
    row.append(nav_next);
}

fn compose_inline(
    row: &gtk::Box,
    nav_prev: &gtk::Button,
    parts: &MediaCardLayoutParts,
    nav_next: &gtk::Button,
) {
    let main = gtk::Box::new(gtk::Orientation::Vertical, 8);
    main.add_css_class("unixnotis-media-main");
    main.set_hexpand(true);
    main.set_halign(Align::Fill);
    main.set_valign(Align::Center);

    let control_strip = build_control_strip(nav_prev, &parts.controls, nav_next);

    parts
        .card
        .root
        .set_orientation(gtk::Orientation::Horizontal);
    parts.card.root.set_spacing(10);
    parts.card.root.append(&parts.art_frame);
    main.append(&parts.card.text_box);
    main.append(&control_strip);
    parts.card.root.append(&main);

    // Inline keeps the whole interaction model inside one card for easier theme experiments
    row.append(&parts.card.root);
}

fn compose_stacked(
    row: &gtk::Box,
    nav_prev: &gtk::Button,
    parts: &MediaCardLayoutParts,
    nav_next: &gtk::Button,
) {
    let header = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    header.add_css_class("unixnotis-media-main");
    header.set_hexpand(true);
    header.set_halign(Align::Fill);
    header.set_valign(Align::Center);

    let control_strip = build_control_strip(nav_prev, &parts.controls, nav_next);

    parts.card.root.set_orientation(gtk::Orientation::Vertical);
    parts.card.root.set_spacing(10);
    header.append(&parts.art_frame);
    header.append(&parts.card.text_box);
    parts.card.root.append(&header);
    parts.card.root.append(&control_strip);

    // Stacked opens more room for vertical themes without changing playback semantics
    row.append(&parts.card.root);
}

fn compose_showcase(
    row: &gtk::Box,
    nav_prev: &gtk::Button,
    parts: &MediaCardLayoutParts,
    nav_next: &gtk::Button,
) {
    let main = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    main.add_css_class("unixnotis-media-main");
    main.set_hexpand(true);
    main.set_halign(Align::Fill);
    main.set_valign(Align::Center);

    let action_rail = build_action_rail(nav_prev, &parts.controls, nav_next);

    parts
        .card
        .root
        .set_orientation(gtk::Orientation::Horizontal);
    parts.card.root.set_spacing(12);
    // Showcase spends width on text first and keeps controls in their own rail
    parts.card.text_box.set_hexpand(true);
    parts.card.text_box.set_halign(Align::Fill);
    main.append(&parts.card.text_box);
    main.append(&action_rail);
    parts.card.root.append(&parts.art_frame);
    parts.card.root.append(&main);

    // Showcase stays as one wide card so themes can treat it like a dashboard module
    row.append(&parts.card.root);
}

fn build_control_strip(
    nav_prev: &gtk::Button,
    controls: &gtk::Box,
    nav_next: &gtk::Button,
) -> gtk::Box {
    let control_strip = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    control_strip.add_css_class("unixnotis-media-control-strip");
    control_strip.set_halign(Align::Fill);
    control_strip.set_valign(Align::Center);

    control_strip.append(nav_prev);
    control_strip.append(controls);
    control_strip.append(nav_next);
    control_strip
}

fn build_action_rail(
    nav_prev: &gtk::Button,
    controls: &gtk::Box,
    nav_next: &gtk::Button,
) -> gtk::Box {
    let action_rail = gtk::Box::new(gtk::Orientation::Vertical, 8);
    action_rail.add_css_class("unixnotis-media-action-rail");
    action_rail.set_halign(Align::End);
    action_rail.set_valign(Align::Center);

    let nav_strip = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    nav_strip.add_css_class("unixnotis-media-nav-strip");
    nav_strip.set_halign(Align::Center);
    nav_strip.set_valign(Align::Center);

    nav_strip.append(nav_prev);
    nav_strip.append(nav_next);
    action_rail.append(controls);
    action_rail.append(&nav_strip);
    action_rail
}
