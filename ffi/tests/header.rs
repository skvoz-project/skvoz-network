//! Header identity and fixed-width ABI signatures remain part of supplier checks.
use skvoz_network_ffi::*;
#[test]
fn public_header_matches_export_widths_and_statuses() {
    let _: extern "C" fn() -> u32 = skvoz_network_abi_version;
    let _: unsafe extern "C" fn(*const u8, usize, i32, *mut u64) -> u32 = skvoz_network_create;
    let _: unsafe extern "C" fn(u64, *const u8, usize, i32, *mut u32) -> u32 =
        skvoz_network_request;
    let _: unsafe extern "C" fn(u64, *mut u8, usize, *mut usize, *mut i32, u32) -> u32 =
        skvoz_network_next_event;
    let _: extern "C" fn(u64) -> u32 = skvoz_network_destroy;
    let header = include_str!("../include/skvoz_network.h");
    for (name, value) in [
        ("ABI_VERSION", skvoz_network_abi_version()),
        ("SUCCESS", SUCCESS),
        ("INSUFFICIENT_BUFFER", INSUFFICIENT_BUFFER),
        ("TIMEOUT", TIMEOUT),
        ("INVALID_ARGUMENT", INVALID_ARGUMENT),
        ("CLOSED", CLOSED),
        ("INTERNAL", INTERNAL),
    ] {
        assert!(header.contains(&format!("#define SKVOZ_NETWORK_{name} UINT32_C({value})")));
    }
    assert_eq!(std::mem::size_of::<u64>(), 8);
    assert_eq!(std::mem::size_of::<u32>(), 4);
    assert_eq!(std::mem::size_of::<i32>(), 4);
}
