//! Popup list management and visibility updates.
//!
//! Focuses on maintaining popup ordering and visibility rules.

use std::collections::{HashSet, VecDeque};

use gtk::prelude::*;
use tracing::debug;
use unixnotis_core::{popup_allowed_by_state, ControlState, NotificationView};

use super::ui_window::{popup_stack_has_active_transitions, refresh_popup_input_region};
use super::UiState;

struct ReconcilePlan {
    // Local rows missing from the daemon snapshot
    stale_ids: Vec<u32>,
    // Rows that must be added or rebuilt to match daemon truth
    upserts: Vec<NotificationView>,
    // Final order copied from the daemon seed
    desired_order: VecDeque<u32>,
}

impl UiState {
    pub(super) fn reconcile_seed(&mut self, active: Vec<NotificationView>) {
        // Seed is a full snapshot, so desired popups come only from this list
        let desired = desired_seed_popups(active, &self.control_state);
        // Compare only the portable notification payload so seed logic stays deterministic
        let local = self
            .popups
            .iter()
            .map(|(id, entry)| (*id, entry.notification.clone()))
            .collect();
        let plan = build_reconcile_plan(&local, &self.popup_order, &desired);

        // Remove stale rows first so replacements and reorders do not leave duplicates behind
        for id in plan.stale_ids {
            self.remove_popup(id);
        }

        // Walk oldest to newest so prepend-based insertion lands in daemon order
        for notification in plan.upserts.iter().rev() {
            match self.popups.contains_key(&notification.id) {
                // Existing rows are rebuilt when seed content or order says they changed
                true => self.replace_popup(notification.clone(), true),
                // Missing rows are inserted from the daemon snapshot
                false => self.add_popup(notification.clone()),
            }
        }

        // Seed order wins even if local insert timing was different before reconnect
        self.popup_order = plan.desired_order;
        self.update_popup_visibility();
        debug!(total = self.popup_order.len(), "popup seed reconciled");
    }

    pub(super) fn retain_allowed_popups(&mut self) {
        // State changes only remove popups that are no longer allowed
        let remove_ids: Vec<u32> = self
            .popups
            .iter()
            .filter_map(|(id, entry)| {
                (!popup_allowed_by_state(entry.notification.urgency, &self.control_state))
                    .then_some(*id)
            })
            .collect();
        for id in remove_ids {
            self.remove_popup(id);
        }
    }

    pub(super) fn add_popup(&mut self, notification: NotificationView) {
        let id = notification.id;
        // Reconcile paths call replace_popup for changes, so duplicates still mean no-op here
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

fn build_reconcile_plan(
    local: &std::collections::HashMap<u32, NotificationView>,
    local_order: &VecDeque<u32>,
    desired: &[NotificationView],
) -> ReconcilePlan {
    let desired_order = desired
        .iter()
        .map(|notification| notification.id)
        .collect::<VecDeque<u32>>();
    let desired_ids = desired
        .iter()
        .map(|notification| notification.id)
        .collect::<HashSet<u32>>();
    let order_changed = local_order != &desired_order;

    // Local rows that the daemon no longer lists must be removed
    let stale_ids = local_order
        .iter()
        .copied()
        .filter(|id| !desired_ids.contains(id))
        .collect::<Vec<u32>>();
    // Rows are rebuilt when content changed or when order must be restored from seed
    let upserts = desired
        .iter()
        .filter(|notification| match local.get(&notification.id) {
            // Identical rows can stay as they are while the daemon order is stable
            Some(existing) => existing != *notification || order_changed,
            // Missing rows must be inserted from seed
            None => true,
        })
        .cloned()
        .collect::<Vec<NotificationView>>();

    ReconcilePlan {
        stale_ids,
        upserts,
        desired_order,
    }
}

fn desired_seed_popups(
    active: Vec<NotificationView>,
    state: &ControlState,
) -> Vec<NotificationView> {
    // Seed filtering uses the same gate as runtime state changes
    active
        .into_iter()
        .filter(|notification| popup_allowed_by_state(notification.urgency, state))
        .collect()
}

#[cfg(test)]
#[path = "ui_popups_tests.rs"]
mod tests;
