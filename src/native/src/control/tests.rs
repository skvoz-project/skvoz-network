use super::*;
use std::io::Read;

fn send_rights(socket: RawFd, fd: RawFd, count: usize) {
    let byte = 0u8;
    let mut control = [0usize; 128];
    let mut iov = libc::iovec {
        iov_base: (&byte as *const u8).cast_mut().cast(),
        iov_len: 1,
    };
    // SAFETY: zeroed msghdr has valid null/zero fields before setup below.
    let mut message: libc::msghdr = unsafe { zeroed() };
    message.msg_iov = &mut iov;
    message.msg_iovlen = 1;
    message.msg_control = control.as_mut_ptr().cast();
    // SAFETY: aligned ancillary storage covers at most 80 RawFd entries;
    // header/data writes stay inside that buffer and all pointers stay live.
    unsafe {
        message.msg_controllen = libc::CMSG_SPACE((count * size_of::<RawFd>()) as u32) as usize;
        assert!(message.msg_controllen <= size_of_val(&control));
        let header = libc::CMSG_FIRSTHDR(&message);
        (*header).cmsg_level = libc::SOL_SOCKET;
        (*header).cmsg_type = libc::SCM_RIGHTS;
        (*header).cmsg_len = libc::CMSG_LEN((count * size_of::<RawFd>()) as u32) as usize;
        for index in 0..count {
            libc::CMSG_DATA(header)
                .add(index * size_of::<RawFd>())
                .cast::<RawFd>()
                .write_unaligned(fd);
        }
        assert_eq!(libc::sendmsg(socket, &message, libc::MSG_NOSIGNAL), 1);
    }
}

#[test]
fn extra_rights_and_truncated_ancillary_close_every_received_fd() {
    for count in [2, 80] {
        let (sender, mut receiver) = FramedUnix::pair().unwrap();
        let (stream, mut peer) = UnixStream::pair().unwrap();
        send_rights(sender.as_raw_fd(), stream.as_raw_fd(), count);
        drop(stream);
        assert!(receiver.receive_frame().is_err());
        peer.set_read_timeout(Some(Duration::from_millis(100)))
            .unwrap();
        assert_eq!(
            peer.read(&mut [0]).unwrap(),
            0,
            "all duplicated rights must close on rejection"
        );
    }
}

#[test]
fn incremental_deadline_releases_retained_rights_without_readiness() {
    let (sender, receiver) = FramedUnix::pair().unwrap();
    let mut receiver = IncrementalUnix::from_owned_fd(receiver.fd).unwrap();
    let (stream, mut peer) = UnixStream::pair().unwrap();
    send_rights(sender.as_raw_fd(), stream.as_raw_fd(), 1);
    drop(stream);
    assert_eq!(
        receiver.try_receive_frame().unwrap_err().kind(),
        io::ErrorKind::WouldBlock
    );
    receiver.reading.as_mut().unwrap().deadline = Instant::now();
    assert_eq!(
        receiver.check_deadlines().unwrap_err().kind(),
        io::ErrorKind::TimedOut
    );
    peer.set_read_timeout(Some(Duration::from_millis(100)))
        .unwrap();
    assert_eq!(
        peer.read(&mut [0]).unwrap(),
        0,
        "pending received FD must close on deadline"
    );
    assert_eq!(
        receiver.try_receive_frame().unwrap_err().kind(),
        io::ErrorKind::BrokenPipe
    );
}
