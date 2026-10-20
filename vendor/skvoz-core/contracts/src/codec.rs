//! Bounded cold DTO serialization. Payload bytes never use this codec.
use crate::abi::MAX_DTO_BYTES;
use std::io::{self, Write};
struct Capped(Vec<u8>);
impl Write for Capped {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > MAX_DTO_BYTES - self.0.len() {
            return Err(io::Error::other("Core DTO exceeds ABI bound"));
        }
        let length = self.0.len() + bytes.len();
        if length > self.0.capacity() {
            let capacity = length
                .max(self.0.capacity().saturating_mul(2))
                .min(MAX_DTO_BYTES);
            self.0.reserve_exact(capacity - self.0.len());
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
pub fn encode(value: &impl serde::Serialize) -> Result<Vec<u8>, serde_json::Error> {
    let mut output = Capped(Vec::new());
    serde_json::to_writer(&mut output, value)?;
    Ok(output.0)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn oversized_sequence_stops_at_the_bridge_bound() {
        use serde::ser::SerializeSeq;
        let visited = std::cell::Cell::new(0);
        struct Sequence<'a>(&'a std::cell::Cell<usize>);
        impl serde::Serialize for Sequence<'_> {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                let mut sequence = serializer.serialize_seq(None)?;
                for n in 0..1_000_000 {
                    self.0.set(n + 1);
                    sequence.serialize_element(&255u8)?;
                }
                sequence.end()
            }
        }
        assert!(encode(&Sequence(&visited)).is_err());
        assert!(visited.get() < MAX_DTO_BYTES);
        let mut output = Capped(Vec::new());
        output.write_all(&vec![0; MAX_DTO_BYTES]).unwrap();
        assert!(output.write_all(&[0]).is_err());
        assert!(output.0.capacity() <= MAX_DTO_BYTES);
    }
}
