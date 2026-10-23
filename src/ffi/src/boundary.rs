use std::os::fd::OwnedFd;

pub(crate) const JSON_MAX: usize = 32_768;

pub(crate) fn duplicate(fd: i32) -> Result<Option<OwnedFd>, ()> {
    match fd {
        -1 => Ok(None),
        0.. => skvoz_network_native::duplicate_inherited(fd)
            .map(Some)
            .map_err(|_| ()),
        _ => Err(()),
    }
}

pub(crate) fn output_valid<T>(ptr: *mut T) -> bool {
    !ptr.is_null()
        && (ptr as usize).is_multiple_of(std::mem::align_of::<T>())
        && range_valid(ptr as usize, std::mem::size_of::<T>())
}

pub(crate) fn range_valid(address: usize, length: usize) -> bool {
    length <= isize::MAX as usize && address.checked_add(length).is_some()
}

pub(crate) fn disjoint(a: usize, a_len: usize, b: usize, b_len: usize) -> bool {
    match (a.checked_add(a_len), b.checked_add(b_len)) {
        (Some(a_end), Some(b_end)) => a_len == 0 || b_len == 0 || a_end <= b || b_end <= a,
        _ => false,
    }
}

/// Copy the bounded input while the caller still owns its storage.
///
/// # Safety
/// The caller must provide readable, initialized bytes in one allocation for
/// the full input size, without concurrent mutation for the duration of the call.
pub(crate) unsafe fn input(ptr: *const u8, length: usize) -> Result<Vec<u8>, ()> {
    if ptr.is_null() || length == 0 || length > JSON_MAX || !range_valid(ptr as usize, length) {
        return Err(());
    }
    // SAFETY: the caller guarantees readability; nonnull and finite length were
    // checked before creating the slice. No caller pointer survives this call.
    Ok(unsafe { std::slice::from_raw_parts(ptr, length) }.to_vec())
}

#[cfg(test)]
#[path = "boundary/tests.rs"]
mod tests;
