use std::sync::Arc;
use std::time::Instant;

use unixnotis_core::{Notification, Urgency};

use super::{DismissOutcome, InsertOutcome, NotificationStore};

impl NotificationStore {
    pub fn insert(&mut self, mut notification: Notification, replaces_id: u32) -> InsertOutcome {
        // Rule transforms happen before any storage decision
        self.apply_rules(&mut notification);
        if self.should_drop_inhibited() {
            // DropAll mode still assigns an ID so call sites can log consistent metadata
            let assigned_id = self.next_id();
            notification.id = assigned_id;
            let notification = Arc::new(notification);
            return InsertOutcome {
                show_popup: false,
                allow_sound: false,
                notification,
                replaced: false,
                evicted: Vec::new(),
                dropped: true,
            };
        }

        // replaces_id is valid only when it points to an existing, owned notification
        let has_replaces_id = replaces_id != 0;
        let replaced = has_replaces_id
            && self.can_replace_notification_for_sender(
                replaces_id,
                notification.sender_name.as_deref(),
                notification.sender_pid,
            );
        let assigned_id = if replaced {
            replaces_id
        } else {
            self.next_id()
        };
        notification.id = assigned_id;

        // Drop stale copies before inserting the fresh one
        self.active.shift_remove(&assigned_id);
        self.history.remove(&assigned_id);
        self.expirations.remove(&assigned_id);

        let notification = Arc::new(notification);
        // Active map keeps insertion order so oldest eviction is deterministic
        self.active.insert(assigned_id, notification.clone());
        let evicted = self.enforce_active_limit();

        InsertOutcome {
            show_popup: self.should_show_popup(&notification),
            allow_sound: self.should_play_sound(&notification),
            notification,
            replaced,
            evicted,
            dropped: false,
        }
    }

    pub fn close(&mut self, id: u32) -> Option<Arc<Notification>> {
        // Active removal and expiration cleanup always happen together
        let removed = self.active.shift_remove(&id);
        self.expirations.remove(&id);
        if let Some(notification) = removed.clone() {
            // History entries are created only when a notification really leaves active state
            self.push_history(notification.clone());
        }
        removed
    }

    pub fn dismiss_from_panel(&mut self, id: u32) -> DismissOutcome {
        // Panel dismissal can target active, history, or both
        let removed_active = self.active.shift_remove(&id).is_some();
        if removed_active {
            self.expirations.remove(&id);
        }

        let removed_history = self.history.remove(&id).is_some();

        DismissOutcome {
            removed_active,
            removed_history,
        }
    }

    pub fn drain_active_ids(&mut self) -> Vec<u32> {
        // Drain in one pass so callers do not need repeated lookups
        let ids = self.active.keys().rev().copied().collect();
        self.active.clear();
        self.expirations.clear();
        ids
    }

    pub fn set_expiration(&mut self, id: u32, deadline: Option<Instant>) {
        // None removes a stale timer for resident or already-dismissed notifications
        match deadline {
            Some(deadline) => {
                self.expirations.insert(id, deadline);
            }
            None => {
                self.expirations.remove(&id);
            }
        }
    }

    pub fn expiration_for(&self, id: u32) -> Option<Instant> {
        self.expirations.get(&id).copied()
    }

    fn enforce_active_limit(&mut self) -> Vec<u32> {
        let max_active = self.config.history.max_active;
        if max_active == 0 {
            // max_active=0 means archive everything immediately
            let mut evicted = Vec::new();
            while let Some((id, notification)) = self.active.shift_remove_index(0) {
                // Evicted notifications should not retain pending expiration entries
                self.expirations.remove(&id);
                self.push_history(notification);
                evicted.push(id);
            }
            return evicted;
        }

        let mut evicted = Vec::new();
        while self.active.len() > max_active {
            // remove_index(0) always pops the oldest notification first
            if let Some((id, notification)) = self.active.shift_remove_index(0) {
                // Eviction path mirrors close path so state stays consistent
                self.expirations.remove(&id);
                self.push_history(notification);
                evicted.push(id);
            } else {
                break;
            }
        }
        evicted
    }

    fn push_history(&mut self, notification: Arc<Notification>) {
        if self.config.history.max_entries == 0 {
            // Clear keeps memory bounded when history feature is disabled
            self.history.clear();
            return;
        }
        // Transient notifications are optional in history by config policy
        if notification.is_transient && !self.config.history.transient_to_history {
            return;
        }
        // to_history strips non-history-only fields and keeps stored payload compact
        let stored = Arc::new(notification.to_history());
        self.history.insert(stored);
        self.history.evict_to_limit(self.config.history.max_entries);
    }

    fn should_show_popup(&self, notification: &Notification) -> bool {
        // Rule-level popup suppression is highest priority
        if notification.suppress_popup {
            return false;
        }
        // Runtime inhibitor suppression applies after rule transforms
        if self.inhibited {
            return false;
        }
        // DND allows only critical popups
        if self.dnd_enabled {
            return notification.urgency == Urgency::Critical;
        }
        true
    }

    fn should_play_sound(&self, notification: &Notification) -> bool {
        // Rule-level silence always wins
        if notification.suppress_sound {
            return false;
        }
        // Inhibitors suppress popups only, while sound follows DND and rule flags
        if self.dnd_enabled {
            return notification.urgency == Urgency::Critical;
        }
        true
    }
}
