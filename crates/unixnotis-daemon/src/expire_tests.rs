use super::*;
use std::time::Duration;

#[test]
fn expiration_heap_orders_by_deadline() {
    let now = Instant::now();
    let mut heap = BinaryHeap::new();
    heap.push(ExpirationItem {
        id: 1,
        deadline: now + Duration::from_secs(2),
    });
    heap.push(ExpirationItem {
        id: 2,
        deadline: now + Duration::from_secs(1),
    });

    let first = heap.pop().expect("first item");
    assert_eq!(first.id, 2);
}

#[test]
fn apply_command_tracks_latest_schedule() {
    let now = Instant::now();
    let mut heap = BinaryHeap::new();
    let mut scheduled = HashMap::new();

    apply_command(
        ExpirationCommand::Schedule {
            id: 7,
            deadline: now + Duration::from_secs(5),
        },
        &mut heap,
        &mut scheduled,
    );
    apply_command(
        ExpirationCommand::Schedule {
            id: 7,
            deadline: now + Duration::from_secs(3),
        },
        &mut heap,
        &mut scheduled,
    );

    assert_eq!(scheduled.len(), 1);
    assert_eq!(scheduled.get(&7), Some(&(now + Duration::from_secs(3))));
    assert_eq!(heap.len(), 2);
}

#[test]
fn apply_command_cancel_removes_schedule() {
    let now = Instant::now();
    let mut heap = BinaryHeap::new();
    let mut scheduled = HashMap::new();

    apply_command(
        ExpirationCommand::Schedule {
            id: 9,
            deadline: now + Duration::from_secs(2),
        },
        &mut heap,
        &mut scheduled,
    );
    apply_command(
        ExpirationCommand::Cancel { id: 9 },
        &mut heap,
        &mut scheduled,
    );

    assert!(scheduled.is_empty());
}

#[test]
fn maybe_compact_rebuilds_from_scheduled() {
    let now = Instant::now();
    let mut heap = BinaryHeap::new();
    let mut scheduled = HashMap::new();

    scheduled.insert(1_u32, now + Duration::from_secs(1));
    for id in 0..129_u32 {
        heap.push(ExpirationItem {
            id,
            deadline: now + Duration::from_secs(id as u64 + 1),
        });
    }

    maybe_compact(&mut heap, &scheduled);
    assert_eq!(heap.len(), scheduled.len());
    let item = heap.pop().expect("rebuilt item");
    assert_eq!(item.id, 1);
}
