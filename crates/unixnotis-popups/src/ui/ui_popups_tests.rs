use std::collections::{HashMap, VecDeque};

use super::{build_reconcile_plan, desired_seed_popups, reload_refresh_plan, visible_popup_target};
use unixnotis_core::{Action, ControlState, NotificationImage, NotificationView, Urgency};

fn make_view(id: u32, urgency: Urgency, summary: &str, body: &str) -> NotificationView {
    // Keep test payloads small and explicit so reconcile changes are easy to spot
    NotificationView {
        id,
        app_name: "Test".to_string(),
        summary: summary.to_string(),
        body: body.to_string(),
        actions: vec![Action {
            key: "default".to_string(),
            label: "Open".to_string(),
        }],
        urgency: urgency as u8,
        is_transient: false,
        // Test rows only need the transport fields used by the popup UI
        image: NotificationImage::default(),
    }
}

#[test]
fn desired_seed_popups_clears_all_when_inhibited() {
    // Inhibit is stronger than urgency, so even critical rows are filtered out
    let state = ControlState {
        inhibited: true,
        ..ControlState::default()
    };
    let desired = desired_seed_popups(
        vec![make_view(1, Urgency::Critical, "critical", "body")],
        &state,
    );
    assert!(desired.is_empty());
}

#[test]
fn desired_seed_popups_keeps_only_critical_during_dnd() {
    // DND keeps only critical rows visible in the popup snapshot
    let state = ControlState {
        dnd_enabled: true,
        ..ControlState::default()
    };
    let desired = desired_seed_popups(
        vec![
            make_view(1, Urgency::Normal, "normal", "body"),
            make_view(2, Urgency::Critical, "critical", "body"),
        ],
        &state,
    );
    assert_eq!(desired.len(), 1);
    assert_eq!(desired[0].id, 2);
}

#[test]
fn desired_seed_popups_preserves_seed_order() {
    // Order from seed matters because popup layout is newest-first
    let state = ControlState::default();
    let desired = desired_seed_popups(
        vec![
            make_view(9, Urgency::Critical, "newest", "body"),
            make_view(4, Urgency::Normal, "older", "body"),
        ],
        &state,
    );
    assert_eq!(desired[0].id, 9);
    assert_eq!(desired[1].id, 4);
}

#[test]
fn build_reconcile_plan_removes_local_rows_missing_from_seed() {
    // Empty seed means the daemon no longer has this popup at all
    let mut local = HashMap::new();
    local.insert(7, make_view(7, Urgency::Normal, "stale", "body"));
    let local_order = VecDeque::from([7]);

    let plan = build_reconcile_plan(&local, &local_order, &[]);

    assert_eq!(plan.stale_ids, vec![7]);
    assert!(plan.upserts.is_empty());
    assert!(plan.desired_order.is_empty());
}

#[test]
fn build_reconcile_plan_rebuilds_rows_when_payload_changes() {
    // Same id with changed payload must rebuild instead of being ignored
    let mut local = HashMap::new();
    local.insert(5, make_view(5, Urgency::Normal, "old", "body"));
    let local_order = VecDeque::from([5]);
    let desired = vec![make_view(5, Urgency::Normal, "new", "body changed")];

    let plan = build_reconcile_plan(&local, &local_order, &desired);

    assert!(plan.stale_ids.is_empty());
    assert_eq!(plan.upserts.len(), 1);
    assert_eq!(plan.upserts[0].id, 5);
    assert_eq!(plan.upserts[0].summary, "new");
    assert_eq!(plan.upserts[0].body, "body changed");
}

#[test]
fn visible_popup_target_clamps_to_runtime_limit() {
    // Visible target should stop at max_visible even when more popups are queued
    assert_eq!(visible_popup_target(5, 2), 2);
}

#[test]
fn visible_popup_target_stays_within_available_popups() {
    // Visible target should not claim more rows than actually exist
    assert_eq!(visible_popup_target(2, 5), 2);
    assert_eq!(visible_popup_target(0, 3), 0);
}

#[test]
fn reload_refresh_plan_keeps_width_updates_to_materialized_rows() {
    // Reload should resize only built rows while still respecting popup limits
    let plan = reload_refresh_plan(5, 2, 3);

    assert_eq!(plan.resized_roots, 2);
    assert_eq!(plan.visible_target, 3);
}

#[test]
fn reload_refresh_plan_handles_popup_disable_cleanly() {
    // Disabling popups should still resize built rows before visibility drops to zero
    let plan = reload_refresh_plan(4, 3, 0);

    assert_eq!(plan.resized_roots, 3);
    assert_eq!(plan.visible_target, 0);
}
