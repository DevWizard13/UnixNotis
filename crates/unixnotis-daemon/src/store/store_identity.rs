use tracing::warn;
use unixnotis_core::Notification;

use super::NotificationStore;

impl NotificationStore {
    pub fn is_notification_owned_by(&self, id: u32, sender: &str, sender_pid: Option<u32>) -> bool {
        // Ownership checks are valid only against active notifications
        let Some(notification) = self.active.get(&id) else {
            return false;
        };
        notification_is_owned_by(notification, Some(sender), sender_pid)
    }

    pub(super) fn next_id(&mut self) -> u32 {
        // Search at most active+history+1 IDs, which guarantees one free slot in that window
        let start = self.next_id.max(1);
        let mut candidate = start;
        let used = self.active.len().saturating_add(self.history.len());
        let max_attempts = used.saturating_add(1).max(1);
        for _ in 0..max_attempts {
            // Candidate must be absent from both active and history sets
            if !self.active.contains_key(&candidate) && !self.history.contains(&candidate) {
                self.next_id = candidate.wrapping_add(1);
                if self.next_id == 0 {
                    self.next_id = 1;
                }
                return candidate;
            }
            // wrapping_add keeps progress valid even near u32::MAX
            candidate = candidate.wrapping_add(1);
            if candidate == 0 {
                candidate = 1;
            }
        }
        warn!(
            used,
            "notification id space exhausted; reusing id to avoid deadlock"
        );
        self.next_id = start.wrapping_add(1);
        if self.next_id == 0 {
            self.next_id = 1;
        }
        start
    }

    pub(super) fn can_replace_notification_for_sender(
        &self,
        id: u32,
        sender: Option<&str>,
        sender_pid: Option<u32>,
    ) -> bool {
        // Replacement is allowed only for the sender that owns the original notification
        let Some(existing) = self.active.get(&id).or_else(|| self.history.get(&id)) else {
            return false;
        };
        notification_is_owned_by(existing, sender, sender_pid)
    }
}

pub(super) fn notification_is_owned_by(
    notification: &Notification,
    sender: Option<&str>,
    sender_pid: Option<u32>,
) -> bool {
    // PID matching permits reconnecting clients without enabling cross-process takeover
    if let (Some(caller_pid), Some(owner_pid)) = (sender_pid, notification.sender_pid) {
        if caller_pid == owner_pid {
            return true;
        }
    }
    // Bus-name fallback handles senders without stable PID metadata
    match (sender, notification.sender_name.as_deref()) {
        (Some(caller), Some(owner)) => caller == owner,
        _ => false,
    }
}
