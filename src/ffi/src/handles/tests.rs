use super::*;

#[test]
fn stale_handles_cannot_access_reused_slot() {
    let mut table = Handles::default();
    let old = table.insert(11).unwrap();
    assert!(table.get(old).is_some());
    assert!(table.remove(old).is_some());
    let current = table.insert(22).unwrap();
    assert_ne!(old, current);
    assert_eq!(old as u32, current as u32);
    assert!(table.get(old).is_none());
    assert!(table.remove(old).is_none());
    assert_eq!(*table.get(current).unwrap().lock().unwrap(), 22);
}

#[test]
fn exhausted_generation_is_retired() {
    let mut table = Handles {
        slots: vec![Slot {
            generation: u32::MAX - 1,
            value: Some(Arc::new(Mutex::new(()))),
        }],
    };
    let old = encode(0, u32::MAX - 1);
    assert!(table.remove(old).is_some());
    let new = table.insert(()).unwrap();
    assert_eq!(new as u32, 2);
    assert!(table.get(old).is_none());
    assert!(table.get(encode(0, u32::MAX)).is_none());
}

#[test]
fn malformed_and_unknown_handles_are_rejected() {
    let mut table = Handles::<()>::default();
    for handle in [0, 1, 1 << 32, u64::MAX] {
        assert!(table.get(handle).is_none());
        assert!(table.remove(handle).is_none());
    }
}

#[test]
fn outstanding_lookup_cannot_resurrect_retired_slot() {
    let mut table = Handles::default();
    let old = table.insert(Some(11)).unwrap();
    let outstanding = table.get(old).unwrap();
    let retired = table.remove(old).unwrap();
    assert_eq!(retired.lock().unwrap().take(), Some(11));
    let current = table.insert(Some(22)).unwrap();
    assert_ne!(old, current);
    assert!(outstanding.lock().unwrap().is_none());
    assert!(table.get(old).is_none());
    assert_eq!(*table.get(current).unwrap().lock().unwrap(), Some(22));
}
