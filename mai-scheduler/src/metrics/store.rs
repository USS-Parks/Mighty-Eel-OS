use std::collections::VecDeque;

/// A generic ring buffer with configurable capacity.
/// Oldest entries are evicted when capacity is reached.
#[derive(Debug, Clone)]
pub struct RingBuffer<T> {
    buffer: VecDeque<T>,
    capacity: usize,
}

impl<T> RingBuffer<T> {
    pub fn new(capacity: usize) -> Self {
        Self {
            buffer: VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    pub fn push(&mut self, item: T) {
        if self.capacity == 0 {
            return;
        }
        if self.buffer.len() >= self.capacity {
            self.buffer.pop_front();
        }
        self.buffer.push_back(item);
    }

    pub fn len(&self) -> usize {
        self.buffer.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn latest(&self) -> Option<&T> {
        self.buffer.back()
    }

    pub fn iter(&self) -> impl Iterator<Item = &T> {
        self.buffer.iter()
    }

    pub fn clear(&mut self) {
        self.buffer.clear();
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_push_and_latest() {
        let mut buf: RingBuffer<i32> = RingBuffer::new(3);
        assert!(buf.is_empty());
        buf.push(1);
        buf.push(2);
        assert_eq!(*buf.latest().unwrap(), 2);
        assert_eq!(buf.len(), 2);
    }

    #[test]
    fn test_eviction_when_full() {
        let mut buf: RingBuffer<i32> = RingBuffer::new(3);
        buf.push(1);
        buf.push(2);
        buf.push(3);
        buf.push(4);
        assert_eq!(buf.len(), 3);
        let collected: Vec<&i32> = buf.iter().collect();
        assert_eq!(collected, vec![&2, &3, &4]);
    }

    #[test]
    fn test_clear() {
        let mut buf: RingBuffer<i32> = RingBuffer::new(3);
        buf.push(1);
        buf.clear();
        assert!(buf.is_empty());
        assert_eq!(buf.len(), 0);
    }

    #[test]
    fn test_capacity_zero() {
        let mut buf: RingBuffer<i32> = RingBuffer::new(0);
        buf.push(1);
        assert!(buf.is_empty());
    }

    #[test]
    fn test_iter_empty() {
        let buf: RingBuffer<i32> = RingBuffer::new(5);
        assert_eq!(buf.iter().count(), 0);
    }
}
