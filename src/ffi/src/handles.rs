//! Generation-checked indices; caller-visible handles never contain pointers.
use std::sync::{Arc, Mutex};

struct Slot<T> {
    generation: u32,
    value: Option<Arc<Mutex<T>>>,
}

pub(crate) struct Handles<T> {
    slots: Vec<Slot<T>>,
}

impl<T> Default for Handles<T> {
    fn default() -> Self {
        Self { slots: Vec::new() }
    }
}

impl<T> Handles<T> {
    pub(crate) fn insert(&mut self, value: T) -> Option<u64> {
        let value = Arc::new(Mutex::new(value));
        if let Some((index, slot)) = self
            .slots
            .iter_mut()
            .enumerate()
            .find(|(_, slot)| slot.value.is_none() && slot.generation != u32::MAX)
        {
            slot.value = Some(value);
            return Some(encode(index, slot.generation));
        }
        if self.slots.len() >= u32::MAX as usize {
            return None;
        }
        let index = self.slots.len();
        self.slots.push(Slot {
            generation: 1,
            value: Some(value),
        });
        Some(encode(index, 1))
    }

    pub(crate) fn get(&self, handle: u64) -> Option<Arc<Mutex<T>>> {
        let (index, generation) = decode(handle)?;
        let slot = self.slots.get(index)?;
        (slot.generation == generation)
            .then(|| slot.value.clone())
            .flatten()
    }

    pub(crate) fn remove(&mut self, handle: u64) -> Option<Arc<Mutex<T>>> {
        let (index, generation) = decode(handle)?;
        let slot = self.slots.get_mut(index)?;
        if slot.generation != generation {
            return None;
        }
        let value = slot.value.take()?;
        // The last generation is permanently retired instead of wrapping.
        slot.generation = slot.generation.saturating_add(1);
        Some(value)
    }
}

fn encode(index: usize, generation: u32) -> u64 {
    (u64::from(generation) << 32) | (index as u64 + 1)
}

fn decode(handle: u64) -> Option<(usize, u32)> {
    let index = handle as u32;
    let generation = (handle >> 32) as u32;
    (index != 0 && generation != 0).then(|| ((index - 1) as usize, generation))
}

#[cfg(test)]
#[path = "handles/tests.rs"]
mod tests;
