use super::*;
#[test]
fn completion_prefix_waits_for_older_packet_before_later_drop() {
    let mut receipts = VecDeque::from([
        Receipt {
            _reservation: None,
            end: 28,
            size: 28,
            state: ReceiptState::Delivered,
        },
        Receipt {
            _reservation: None,
            end: 56,
            size: 0,
            state: ReceiptState::Done,
        },
        Receipt {
            _reservation: None,
            end: 84,
            size: 28,
            state: ReceiptState::Delivered,
        },
    ]);
    assert_eq!(completed_prefix(&mut receipts), None);
    complete_receipt(&mut receipts, 84).unwrap();
    assert_eq!(completed_prefix(&mut receipts), None);
    complete_receipt(&mut receipts, 28).unwrap();
    assert_eq!(completed_prefix(&mut receipts), Some(84));
    assert!(receipts.is_empty());
}
#[test]
fn completion_requires_delivered_exact_token_and_is_single_use() {
    let mut receipts = VecDeque::from([Receipt {
        _reservation: None,
        end: 28,
        size: 28,
        state: ReceiptState::Queued,
    }]);
    assert_eq!(
        complete_receipt(&mut receipts, 28),
        Err(NetworkError::InvalidState)
    );
    receipts[0].state = ReceiptState::Delivered;
    assert_eq!(
        complete_receipt(&mut receipts, 27),
        Err(NetworkError::InvalidState)
    );
    assert_eq!(complete_receipt(&mut receipts, 28), Ok(28));
    assert_eq!(
        complete_receipt(&mut receipts, 28),
        Err(NetworkError::InvalidState)
    );
    assert_eq!(completed_prefix(&mut receipts), Some(28));
}
#[test]
fn queued_packet_is_not_consumed_by_polling_or_copying() {
    let mut receipts = VecDeque::from([Receipt {
        _reservation: None,
        end: 28,
        size: 28,
        state: ReceiptState::Queued,
    }]);
    assert_eq!(completed_prefix(&mut receipts), None);
    receipts[0].state = ReceiptState::Delivered;
    assert_eq!(completed_prefix(&mut receipts), None);
}
