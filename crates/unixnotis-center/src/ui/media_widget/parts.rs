use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{Align, Overflow};

use crate::media::MediaHandle;

use super::super::marquee::MarqueeLabel;
use super::card::MediaCardWidgets;
use super::selection::MediaSelection;

pub(super) struct MediaCardLayoutParts {
    pub(super) card: MediaCardWidgets,
    pub(super) art_frame: gtk::Box,
    pub(super) controls: gtk::Box,
}

pub(super) fn build_media_card_parts(
    handle: &MediaHandle,
    selection: Rc<RefCell<MediaSelection>>,
    marquee_width: i32,
    title_char_limit: usize,
) -> MediaCardLayoutParts {
    // The shared card starts neutral and gets arranged later by each layout shell
    let root = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    root.add_css_class("unixnotis-media-card");
    root.set_hexpand(true);
    root.set_halign(Align::Fill);
    root.set_valign(Align::Center);

    let art = build_art_picture();
    // The frame owns the visible slot size even when artwork is missing
    let art_frame = build_art_frame(&art);

    let text_box = build_text_box(marquee_width);
    // The meta row keeps source and counter grouped so one visibility toggle can hide both
    let meta_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    meta_row.add_css_class("unixnotis-media-meta");
    meta_row.set_hexpand(true);
    meta_row.set_halign(Align::Fill);

    let source_label = gtk::Label::new(Some(""));
    source_label.set_xalign(0.0);
    source_label.add_css_class("unixnotis-media-source");

    let position_label = gtk::Label::new(Some(""));
    position_label.set_xalign(1.0);
    position_label.add_css_class("unixnotis-media-position");

    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 1);
    spacer.set_hexpand(true);
    // The spacer keeps source left-aligned and the counter pushed to the far edge
    meta_row.append(&source_label);
    meta_row.append(&spacer);
    meta_row.append(&position_label);

    // The marquee widget owns title truncation and scrolling rules in one place
    let title_label = MarqueeLabel::new("unixnotis-media-title", marquee_width, title_char_limit);
    let title_widget = title_label.widget();
    title_widget.set_hexpand(false);
    title_widget.set_halign(Align::Start);

    let artist_label = gtk::Label::new(None);
    artist_label.set_xalign(0.0);
    artist_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    artist_label.add_css_class("unixnotis-media-artist");

    text_box.append(&meta_row);
    text_box.append(&title_widget);
    text_box.append(&artist_label);

    // Transport buttons are shared across every shell preset
    let (controls, prev_button, play_button, next_button) = build_controls();
    connect_playback_buttons(handle, selection, &play_button, &next_button, &prev_button);

    // The art key lets async art loads ignore stale completions
    let art_key = Rc::new(RefCell::new(None));
    // Metadata visibility depends on both config and current player count
    let show_source_pref = Rc::new(Cell::new(true));
    let show_position_pref = Rc::new(Cell::new(true));
    let player_total = Rc::new(Cell::new(0usize));

    MediaCardLayoutParts {
        art_frame,
        controls,
        card: MediaCardWidgets {
            root,
            art,
            text_box,
            meta_row,
            source_label,
            position_label,
            title_label,
            artist_label,
            play_button,
            next_button,
            prev_button,
            art_key,
            show_source_pref,
            show_position_pref,
            player_total,
        },
    }
}

fn build_art_picture() -> gtk::Picture {
    // Artwork starts hidden so empty players do not leave a blank image box
    let art = gtk::Picture::new();
    art.add_css_class("unixnotis-media-art");
    art.set_can_shrink(true);
    art.set_size_request(50, 50);
    art.set_keep_aspect_ratio(true);
    art.set_hexpand(false);
    art.set_vexpand(false);
    art.set_halign(Align::Center);
    art.set_valign(Align::Center);
    art.set_overflow(Overflow::Hidden);
    art.set_visible(false);
    art
}

fn build_art_frame(art: &gtk::Picture) -> gtk::Box {
    // The frame keeps the slot size stable even when the art widget is hidden
    let art_frame = gtk::Box::new(gtk::Orientation::Vertical, 0);
    art_frame.add_css_class("unixnotis-media-art-frame");
    art_frame.set_size_request(54, 54);
    art_frame.set_hexpand(false);
    art_frame.set_vexpand(false);
    art_frame.set_halign(Align::Center);
    art_frame.set_valign(Align::Center);
    art_frame.set_overflow(Overflow::Hidden);
    art_frame.append(art);
    art_frame
}

fn build_text_box(marquee_width: i32) -> gtk::Box {
    // The title lane gets an explicit width so relayout math stays predictable
    let text_box = gtk::Box::new(gtk::Orientation::Vertical, 2);
    text_box.set_hexpand(false);
    text_box.set_halign(Align::Fill);
    text_box.set_valign(Align::Center);
    text_box.set_size_request(marquee_width, -1);
    text_box
}

fn build_controls() -> (gtk::Box, gtk::Button, gtk::Button, gtk::Button) {
    // Controls stay grouped so shells can move them as one block
    let controls = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    controls.add_css_class("unixnotis-media-controls");
    controls.set_halign(Align::End);
    controls.set_valign(Align::Center);

    let prev_button = gtk::Button::from_icon_name("media-skip-backward-symbolic");
    let play_button = gtk::Button::from_icon_name("media-playback-start-symbolic");
    let next_button = gtk::Button::from_icon_name("media-skip-forward-symbolic");

    prev_button.add_css_class("unixnotis-media-button");
    play_button.add_css_class("unixnotis-media-button");
    // The primary class gives themes one hook for the play or pause button
    play_button.add_css_class("primary");
    next_button.add_css_class("unixnotis-media-button");

    controls.append(&prev_button);
    controls.append(&play_button);
    controls.append(&next_button);
    (controls, prev_button, play_button, next_button)
}

fn connect_playback_buttons(
    handle: &MediaHandle,
    selection: Rc<RefCell<MediaSelection>>,
    play_button: &gtk::Button,
    next_button: &gtk::Button,
    prev_button: &gtk::Button,
) {
    let selection_play = selection.clone();
    let handle_play = handle.clone();
    play_button.connect_clicked(move |_| {
        // The current card decides which player receives transport commands
        if let Some(bus_name) = selection_play.borrow().current_bus() {
            handle_play.play_pause(&bus_name);
        }
    });

    let selection_next = selection.clone();
    let handle_next = handle.clone();
    next_button.connect_clicked(move |_| {
        // Next only targets the currently selected player
        if let Some(bus_name) = selection_next.borrow().current_bus() {
            handle_next.next(&bus_name);
        }
    });

    let selection_prev = selection;
    let handle_prev = handle.clone();
    prev_button.connect_clicked(move |_| {
        // Previous only targets the currently selected player
        if let Some(bus_name) = selection_prev.borrow().current_bus() {
            handle_prev.previous(&bus_name);
        }
    });
}
