use super::*;
#[test]
fn coalesced_send_keeps_every_partial_record_suffix() {
    for cut in 1..=30 {
        let mut queue = VecDeque::from([
            Pending {
                bytes: (0..10).collect(),
                cursor: 0,
            },
            Pending {
                bytes: (10..20).collect(),
                cursor: 0,
            },
            Pending {
                bytes: (20..30).collect(),
                cursor: 0,
            },
        ]);
        let batch = gather_output(&queue, 32768, 16);
        assert_eq!(batch, (0..30).collect::<Vec<_>>());
        let (_, records) = advance_output(&mut queue, cut);
        assert_eq!(records, cut / 10);
        assert_eq!(
            gather_output(&queue, 32768, 16),
            (cut as u8..30).collect::<Vec<_>>()
        );
    }
}
#[test]
fn gather_respects_record_and_byte_quantum() {
    let queue = VecDeque::from([
        Pending {
            bytes: vec![1; 1508],
            cursor: 100,
        },
        Pending {
            bytes: vec![2; 1508],
            cursor: 0,
        },
        Pending {
            bytes: vec![3; 1508],
            cursor: 0,
        },
    ]);
    assert_eq!(gather_output(&queue, 32768, 1).len(), 1408);
    assert_eq!(gather_output(&queue, 2000, 16).len(), 2000);
    assert_eq!(gather_output(&queue, 32768, 2).len(), 2916);
}
#[test]
fn frame_sized_gather_avoids_recopying_unacceptable_suffix() {
    let mut queue = VecDeque::from([
        Pending {
            bytes: vec![1; 14000],
            cursor: 0,
        },
        Pending {
            bytes: vec![2; 10000],
            cursor: 0,
        },
    ]);
    let first = gather_output(&queue, 32768, 16);
    assert_eq!(first.len(), 16384);
    assert_eq!(advance_output(&mut queue, first.len()), (14000, 1));
    let second = gather_output(&queue, 32768 - first.len(), 15);
    assert_eq!(second, vec![2; 7616]);
    assert_eq!(advance_output(&mut queue, second.len()), (10000, 1));
    assert!(queue.is_empty());
}
