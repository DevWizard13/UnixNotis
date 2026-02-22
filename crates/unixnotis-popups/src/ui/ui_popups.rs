//! Popup list management and visibility updates.
//!
//! Focuses on maintaining popup ordering and visibility rules.

use gtk::prelude::*;
use tracing::debug;
use unixnotis_core::NotificationView;

use super::ui_window::{popup_stack_has_active_transitions, refresh_popup_input_region};
use super::UiState;

impl UiState {
    pub(super) fn add_popup(&mut self, notification: NotificationView) {
        let id = notification.id;
        // Ignore duplicates so ordering and animations stay deterministic
        if self.popups.contains_key(&id) {
            return;
        }

        // Build widgets first so ordering updates remain consistent.
        let entry = self.build_popup_entry(&notification);
        self.popup_stack.prepend(&entry.revealer);
        self.popups.insert(id, entry);
        self.popup_order.push_front(id);
        self.update_popup_visibility();
        debug!(id, total = self.popup_order.len(), "popup inserted");
    }

    pub(super) fn replace_popup(&mut self, notification: NotificationView, show_popup: bool) {
        let id = notification.id;
        // Replace path keeps one source of truth for update semantics
        self.remove_popup(id);
        if show_popup {
            self.add_popup(notification);
        }
    }

    pub(super) fn remove_popup(&mut self, id: u32) {
        if let Some(entry) = self.popups.remove(&id) {
            // Revealers animate out before removing from the stack.
            entry.revealer.set_reveal_child(false);
            let stack = self.popup_stack.clone();
            let popup_window = self.popup_window.clone();
            let popup_input_region = self.popup_input_region.clone();
            entry
                .revealer
                .connect_notify_local(Some("child-revealed"), move |revealer, _| {
                    // Remove only after transition completes to avoid visual pop
                    if !revealer.is_child_revealed() && revealer.parent().is_some() {
                        stack.remove(revealer);
                    }
                    // Re-sync clickable shape after each reveal step
                    let has_active_transitions = popup_stack_has_active_transitions(&stack);
                    refresh_popup_input_region(
                        &popup_window,
                        &stack,
                        &popup_input_region,
                        has_active_transitions,
                    );
                });
        }
        self.popup_order.retain(|item| *item != id);
        self.update_popup_visibility();
        debug!(id, total = self.popup_order.len(), "popup removed");
    }

    pub(super) fn clear_popups(&mut self) {
        // Snapshot ids to avoid mutating while iterating.
        let ids: Vec<u32> = self.popup_order.iter().copied().collect();
        for id in ids {
            self.remove_popup(id);
        }
    }

    pub(super) fn update_popup_visibility(&self) {
        // Visibility contract is driven strictly by configured max_visible count
        let max_visible = self.config.popups.max_visible;

        // Max-visible of zero disables popups entirely.
        if max_visible == 0 {
            for entry in self.popups.values() {
                entry.root.set_visible(false);
                entry.revealer.set_reveal_child(false);
            }
            self.popup_window.set_visible(false);
            // Keep input region empty when popups are disabled
            refresh_popup_input_region(
                &self.popup_window,
                &self.popup_stack,
                &self.popup_input_region,
                false,
            );
            debug!("popups disabled by max_visible = 0");
            return;
        }

        // Hide the top-level window when there are no active popups.
        if self.popup_order.is_empty() {
            self.popup_window.set_visible(false);
        } else {
            self.popup_window.set_visible(true);
        }

        for (index, id) in self.popup_order.iter().enumerate() {
            if let Some(entry) = self.popups.get(id) {
                // Clean up previous state classes.
                entry.root.remove_css_class("unixnotis-popup-visible");

                if index < max_visible {
                    // Fully visible notification.
                    entry.root.set_visible(true);
                    entry.revealer.set_reveal_child(true);
                    entry.root.add_css_class("unixnotis-popup-visible");
                } else {
                    // Keep overflow notifications hidden to avoid visual layering artifacts.
                    // Hidden entries still stay in memory so close/update events stay coherent
                    entry.root.set_visible(false);
                    entry.revealer.set_reveal_child(false);
                }
            }
        }
        // Tick while transitions run so interactive area tracks animation frames
        let has_active_transitions = popup_stack_has_active_transitions(&self.popup_stack);
        refresh_popup_input_region(
            &self.popup_window,
            &self.popup_stack,
            &self.popup_input_region,
            has_active_transitions,
        );
        debug!(
            visible = self.popup_order.len().min(max_visible),
            total = self.popup_order.len(),
            "popup visibility updated"
        );
    }
}
