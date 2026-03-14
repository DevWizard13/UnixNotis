mod build;
mod card;
mod selection;

use std::cell::RefCell;
use std::rc::Rc;

use glib::clone;
use gtk::prelude::*;

use crate::media::{MediaHandle, MediaInfo};

use self::build::{build_media_card, build_navigation_button};
use self::card::MediaCardWidgets;
use self::selection::MediaSelection;

pub struct MediaWidget {
    root: gtk::Box,
    nav_prev: gtk::Button,
    nav_next: gtk::Button,
    card: MediaCardWidgets,
    selection: Rc<RefCell<MediaSelection>>,
}

impl MediaWidget {
    pub fn new(
        container: &gtk::Box,
        handle: MediaHandle,
        panel_width: i32,
        title_char_limit: usize,
    ) -> Self {
        // Reserve room for controls and art so the marquee width stays stable
        let marquee_width = panel_width.saturating_sub(240).max(140);
        let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
        root.add_css_class("unixnotis-media-stack");
        root.set_visible(false);

        let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        row.add_css_class("unixnotis-media-row");
        row.set_hexpand(true);

        // The nav buttons stay outside the card so the center text width stays stable
        let nav_prev = build_navigation_button("<");
        let nav_next = build_navigation_button(">");
        let selection = Rc::new(RefCell::new(MediaSelection::default()));
        // The card builder owns the playback buttons and artwork widget setup
        let card = build_media_card(&handle, selection.clone(), marquee_width, title_char_limit);

        row.append(&nav_prev);
        row.append(&card.root);
        row.append(&nav_next);
        root.append(&row);
        container.append(&root);

        connect_prev_button(&nav_prev, &nav_next, &root, selection.clone(), card.clone());
        connect_next_button(&nav_prev, &nav_next, &root, selection.clone(), card.clone());

        Self {
            root,
            nav_prev,
            nav_next,
            card,
            selection,
        }
    }

    pub fn update(&mut self, infos: &[MediaInfo]) {
        // Snapshot replacement keeps the carousel in sync with the latest media list
        self.selection.borrow_mut().set_players(infos.to_vec());
        apply_selection(
            &self.selection.borrow(),
            &self.card,
            &self.root,
            &self.nav_prev,
            &self.nav_next,
        );
    }

    pub fn clear(&mut self) {
        // Clearing hides the whole media stack so stale art never lingers
        self.selection.borrow_mut().players.clear();
        self.root.set_visible(false);
    }

    pub fn apply_layout(&mut self, panel_width: i32, title_char_limit: usize) {
        // The text box is the only part that needs a live width update
        let marquee_width = panel_width.saturating_sub(240).max(140);
        self.card.text_box.set_size_request(marquee_width, -1);
        self.card
            .title_label
            .update_limits(marquee_width, title_char_limit);
    }
}

fn connect_prev_button(
    nav_prev: &gtk::Button,
    nav_next: &gtk::Button,
    root: &gtk::Box,
    selection: Rc<RefCell<MediaSelection>>,
    card: MediaCardWidgets,
) {
    let selection_prev = selection.clone();
    let card_prev = card.clone();
    nav_prev.connect_clicked(clone!(
        #[weak]
        root,
        #[weak]
        nav_prev,
        #[weak]
        nav_next,
        #[strong]
        selection_prev,
        #[strong]
        card_prev,
        move |_| {
            // Weak captures avoid cycles when the panel rebuilds on config reload
            selection_prev.borrow_mut().prev();
            apply_selection(
                &selection_prev.borrow(),
                &card_prev,
                &root,
                &nav_prev,
                &nav_next,
            );
        }
    ));
}

fn connect_next_button(
    nav_prev: &gtk::Button,
    nav_next: &gtk::Button,
    root: &gtk::Box,
    selection: Rc<RefCell<MediaSelection>>,
    card: MediaCardWidgets,
) {
    let selection_next = selection;
    let card_next = card;
    nav_next.connect_clicked(clone!(
        #[weak]
        root,
        #[weak]
        nav_prev,
        #[weak]
        nav_next,
        #[strong]
        selection_next,
        #[strong]
        card_next,
        move |_| {
            // Weak captures avoid cycles when the panel rebuilds on config reload
            selection_next.borrow_mut().next();
            apply_selection(
                &selection_next.borrow(),
                &card_next,
                &root,
                &nav_prev,
                &nav_next,
            );
        }
    ));
}

fn apply_selection(
    selection: &MediaSelection,
    card: &MediaCardWidgets,
    root: &gtk::Box,
    nav_prev: &gtk::Button,
    nav_next: &gtk::Button,
) {
    if let Some(info) = selection.current() {
        let (current, total) = selection.position();
        // One update call refreshes labels, controls, and artwork together
        card.update(info, current, total);
        root.set_visible(true);
    } else {
        root.set_visible(false);
    }

    let has_multiple = selection.has_multiple();
    // Single-player snapshots do not need navigation affordances
    nav_prev.set_sensitive(has_multiple);
    nav_next.set_sensitive(has_multiple);
}
