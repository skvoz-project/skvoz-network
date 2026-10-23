use super::*;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::net::UnixStream;

#[test]
fn panic_does_not_cross_the_abi() {
    assert_eq!(guarded(|| panic!("boundary test")), INTERNAL);
}

#[test]
fn message_delivery_transfers_fd_once_until_caller_closes_it() {
    let (socket, mut peer) = UnixStream::pair().unwrap();
    let expected = socket.as_raw_fd();
    let message = OwnedMessage {
        json: b"{}".to_vec(),
        fd: Some(socket.into()),
    };
    let mut bytes = [0; 2];
    let mut len = 0;
    let mut fd = -1;
    // SAFETY: all outputs are valid disjoint local storage.
    assert_eq!(
        unsafe { deliver(message, bytes.as_mut_ptr(), bytes.len(), &mut len, &mut fd) },
        SUCCESS
    );
    assert_eq!(bytes, *b"{}");
    assert_eq!(len, 2);
    assert_eq!(fd, expected);
    // SAFETY: deliver transferred this one descriptor to its caller exactly
    // once. This test now represents the C caller owning/closing the FD.
    let mut caller = UnixStream::from(unsafe { OwnedFd::from_raw_fd(fd) });
    caller.write_all(b"x").unwrap();
    let mut received = [0];
    peer.read_exact(&mut received).unwrap();
    assert_eq!(received, *b"x");
    drop(caller);
    assert_eq!(peer.read(&mut received).unwrap(), 0);
}

#[test]
fn invalid_actor_delivery_closes_its_descriptor_without_exposing_it() {
    let (socket, mut peer) = UnixStream::pair().unwrap();
    let message = OwnedMessage {
        json: b"{}".to_vec(),
        fd: Some(socket.into()),
    };
    let mut bytes = [0; 1];
    let mut len = 0;
    let mut fd = -1;
    // SAFETY: valid local output regions; the message is larger than the
    // buffer and must reject before copying or transferring its descriptor.
    assert_eq!(
        unsafe { deliver(message, bytes.as_mut_ptr(), bytes.len(), &mut len, &mut fd) },
        INTERNAL
    );
    assert_eq!((len, fd), (0, -1));
    assert_eq!(peer.read(&mut [0]).unwrap(), 0);
}
