use super::*;
#[test]
fn old_publication_cannot_reappear_after_disable_or_new_collection() {
    let mailbox = Mailbox::new();
    assert!(!mailbox.read(false).unwrap().enabled);
    assert_eq!(mailbox.read(true).unwrap().sample_age_ms, None);
    let first = mailbox.control();
    mailbox
        .publish(
            first,
            RuntimeDiagnostics {
                enabled: true,
                turns: 10,
                ..RuntimeDiagnostics::default()
            },
        )
        .unwrap();
    assert_eq!(mailbox.read(true).unwrap().turns, 10);
    assert!(mailbox.read(true).unwrap().sample_age_ms.is_some());
    let disabled = mailbox.read(false).unwrap();
    assert_eq!(disabled.sample_age_ms, None);
    assert_eq!(disabled.turns, 0);
    mailbox
        .publish(
            first,
            RuntimeDiagnostics {
                enabled: true,
                turns: 99,
                ..RuntimeDiagnostics::default()
            },
        )
        .unwrap();
    let fresh = mailbox.read(true).unwrap();
    assert_ne!(fresh.collection, first >> 1);
    assert_eq!(fresh.turns, 0);
    assert_eq!(fresh.sample_age_ms, None);
    mailbox
        .publish(
            first,
            RuntimeDiagnostics {
                enabled: true,
                turns: 99,
                ..RuntimeDiagnostics::default()
            },
        )
        .unwrap();
    assert_eq!(mailbox.read(true).unwrap().sample_age_ms, None);
}
#[test]
fn concurrent_readers_preserve_epoch_and_final_disable() {
    let mailbox = std::sync::Arc::new(Mailbox::new());
    std::thread::scope(|scope| {
        for index in 0..4 {
            let mailbox = mailbox.clone();
            scope.spawn(move || {
                for turn in 0..100 {
                    mailbox.read((turn + index) % 2 == 0).unwrap();
                }
            });
        }
    });
    let old = mailbox.control();
    let disabled = mailbox.read(false).unwrap();
    mailbox
        .publish(
            old | 1,
            RuntimeDiagnostics {
                enabled: true,
                turns: 999,
                ..RuntimeDiagnostics::default()
            },
        )
        .unwrap();
    let current = mailbox.read(false).unwrap();
    assert_eq!(disabled.collection, current.collection);
    assert!(!current.enabled);
    assert_eq!(current.turns, 0);
    assert_eq!(current.sample_age_ms, None);
}
#[test]
fn fixed_storage_and_numeric_json_are_bounded() {
    assert!(
        std::mem::size_of::<Mailbox>()
            + std::mem::size_of::<super::super::ActorDiagnostics>()
            + std::mem::size_of::<skvoz_core::runtime::TurnDiagnostics>()
            + 64
            <= RESERVATION
    );
    let json = serde_json::to_vec(&RuntimeDiagnostics::default()).unwrap();
    assert!(json.len() <= 4096);
    let mut largest = serde_json::to_value(RuntimeDiagnostics::default()).unwrap();
    for value in largest.as_object_mut().unwrap().values_mut() {
        if value.is_number() || value.is_null() {
            *value = serde_json::Value::from(u64::MAX);
        }
    }
    assert!(serde_json::to_vec(&largest).unwrap().len() <= 4096);
}
