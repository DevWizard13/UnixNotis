use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::Align;

use crate::media::MediaHandle;

use super::super::marquee::MarqueeLabel;
use super::card::MediaCardWidgets;
use super::selection::MediaSelection;

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

pub(super) fn build_media_card(
    handle: &MediaHandle,
    selection: Rc<RefCell<MediaSelection>>,
    marquee_width: i32,
    title_char_limit: usize,
) -> MediaCardWidgets {
    let root = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    root.add_css_class("unixnotis-media-card");
    root.set_hexpand(true);
    root.set_halign(Align::Fill);
    root.set_valign(Align::Center);
    // Fixed height keeps the media pill steady across metadata changes
    root.set_size_request(-1, 72);

    let art = build_art_picture();
    // The frame keeps the card shape steady even when art is missing
    let art_frame = build_art_frame(&art);
    let info_row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    info_row.set_hexpand(true);
    info_row.set_halign(Align::Fill);
    info_row.set_valign(Align::Center);

    let text_box = build_text_box(marquee_width);
    let source_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let source_label = gtk::Label::new(Some(""));
    source_label.set_xalign(0.0);
    source_label.add_css_class("unixnotis-media-source");

    let position_label = gtk::Label::new(Some(""));
    position_label.set_xalign(1.0);
    position_label.add_css_class("unixnotis-media-position");

    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 1);
    spacer.set_hexpand(true);
    source_row.append(&source_label);
    source_row.append(&spacer);
    source_row.append(&position_label);

    let title_label = MarqueeLabel::new("unixnotis-media-title", marquee_width, title_char_limit);
    let marquee_widget = title_label.widget();
    marquee_widget.set_hexpand(false);
    marquee_widget.set_halign(Align::Start);

    let artist_label = gtk::Label::new(None);
    artist_label.set_xalign(0.0);
    artist_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    artist_label.add_css_class("unixnotis-media-artist");

    text_box.append(&source_row);
    text_box.append(&marquee_widget);
    text_box.append(&artist_label);

    let (controls, prev_button, play_button, next_button) = build_controls();
    info_row.append(&text_box);
    info_row.append(&controls);
    root.append(&art_frame);
    root.append(&info_row);

    connect_playback_buttons(handle, selection, &play_button, &next_button, &prev_button);
    // The art key lets async art loads ignore stale completions
    let art_key = Rc::new(RefCell::new(None));

    MediaCardWidgets {
        root,
        art,
        text_box,
        source_label,
        position_label,
        title_label,
        artist_label,
        play_button,
        next_button,
        prev_button,
        art_key,
    }
}

fn build_art_picture() -> gtk::Picture {
    let art = gtk::Picture::new();
    art.add_css_class("unixnotis-media-art");
    art.set_can_shrink(true);
    art.set_size_request(50, 50);
    art.set_keep_aspect_ratio(true);
    art.set_hexpand(false);
    art.set_vexpand(false);
    art.set_halign(Align::Center);
    art.set_valign(Align::Center);
    art.set_visible(false);
    art
}

fn build_art_frame(art: &gtk::Picture) -> gtk::Box {
    let art_frame = gtk::Box::new(gtk::Orientation::Vertical, 0);
    art_frame.add_css_class("unixnotis-media-art-frame");
    art_frame.set_size_request(54, 54);
    art_frame.set_hexpand(false);
    art_frame.set_vexpand(false);
    art_frame.set_halign(Align::Center);
    art_frame.set_valign(Align::Center);
    art_frame.append(art);
    art_frame
}

fn build_text_box(marquee_width: i32) -> gtk::Box {
    let text_box = gtk::Box::new(gtk::Orientation::Vertical, 2);
    text_box.set_hexpand(false);
    text_box.set_halign(Align::Fill);
    text_box.set_size_request(marquee_width, -1);
    text_box
}

fn build_controls() -> (gtk::Box, gtk::Button, gtk::Button, gtk::Button) {
    let controls = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    controls.add_css_class("unixnotis-media-controls");
    controls.set_halign(Align::End);
    controls.set_valign(Align::Center);

    let prev_button = gtk::Button::from_icon_name("media-skip-backward-symbolic");
    let play_button = gtk::Button::from_icon_name("media-playback-start-symbolic");
    let next_button = gtk::Button::from_icon_name("media-skip-forward-symbolic");

    prev_button.add_css_class("unixnotis-media-button");
    play_button.add_css_class("unixnotis-media-button");
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
        if let Some(bus_name) = selection_next.borrow().current_bus() {
            handle_next.next(&bus_name);
        }
    });

    let selection_prev = selection;
    let handle_prev = handle.clone();
    prev_button.connect_clicked(move |_| {
        if let Some(bus_name) = selection_prev.borrow().current_bus() {
            handle_prev.previous(&bus_name);
        }
    });
}
