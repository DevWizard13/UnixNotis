use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;

use crate::media::MediaInfo;

use super::super::marquee::MarqueeLabel;
use super::super::media_art::apply_media_art;

#[derive(Clone)]
pub(super) struct MediaCardWidgets {
    pub(super) root: gtk::Box,
    pub(super) art: gtk::Picture,
    pub(super) text_box: gtk::Box,
    pub(super) source_label: gtk::Label,
    pub(super) position_label: gtk::Label,
    pub(super) title_label: MarqueeLabel,
    pub(super) artist_label: gtk::Label,
    pub(super) play_button: gtk::Button,
    pub(super) next_button: gtk::Button,
    pub(super) prev_button: gtk::Button,
    pub(super) art_key: Rc<RefCell<Option<String>>>,
}

impl MediaCardWidgets {
    pub(super) fn update(&self, info: &MediaInfo, current: usize, total: usize) {
        // Skip text work when the value already matches the visible label
        if self.source_label.text() != info.identity.as_str() {
            self.source_label.set_text(&info.identity);
        }
        let position = format!("{current}/{total}");
        if self.position_label.text() != position.as_str() {
            self.position_label.set_text(&position);
        }

        let title = if info.title.is_empty() {
            // Missing titles fall back to the player name instead of a blank line
            info.identity.clone()
        } else {
            info.title.clone()
        };
        // The marquee handles its own internal caching
        self.title_label.set_text(&title);
        update_artist_label(&self.artist_label, &info.artist);
        update_play_button(&self.play_button, &info.playback_status);
        update_control_sensitivity(self, info);

        // Artwork loading is centralized so remote and local sources share one safety path
        apply_media_art(&self.art, &self.art_key, info.art_source.as_ref());
        if !self.art.is_visible() {
            self.art.set_visible(true);
        }

        update_playing_class(&self.root, &info.playback_status);
    }
}

fn update_artist_label(label: &gtk::Label, artist: &str) {
    if artist.is_empty() {
        // A blank placeholder keeps the card height from jumping
        if label.text() != " " {
            label.set_text(" ");
        }
        if !label.has_css_class("empty") {
            label.add_css_class("empty");
        }
    } else {
        if label.text() != artist {
            label.set_text(artist);
        }
        if label.has_css_class("empty") {
            label.remove_css_class("empty");
        }
    }
    label.set_visible(true);
}

fn update_play_button(button: &gtk::Button, playback_status: &str) {
    let icon_name = if playback_status == "Playing" {
        "media-playback-pause-symbolic"
    } else {
        "media-playback-start-symbolic"
    };
    // Skip icon churn when playback state has not changed
    if button.icon_name().as_deref() != Some(icon_name) {
        button.set_icon_name(icon_name);
    }
}

fn update_control_sensitivity(card: &MediaCardWidgets, info: &MediaInfo) {
    // Each flag comes from MPRIS capabilities, so the UI mirrors player support directly
    let can_play = info.can_play || info.can_pause;
    if card.play_button.is_sensitive() != can_play {
        card.play_button.set_sensitive(can_play);
    }
    if card.next_button.is_sensitive() != info.can_next {
        card.next_button.set_sensitive(info.can_next);
    }
    if card.prev_button.is_sensitive() != info.can_prev {
        card.prev_button.set_sensitive(info.can_prev);
    }
}

fn update_playing_class(root: &gtk::Box, playback_status: &str) {
    if playback_status == "Playing" {
        // The css class drives the active glow only while playback is live
        if !root.has_css_class("playing") {
            root.add_css_class("playing");
        }
    } else if root.has_css_class("playing") {
        root.remove_css_class("playing");
    }
}
