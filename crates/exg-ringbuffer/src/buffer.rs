use std::sync::atomic::{AtomicU64, Ordering};

use memmap2::MmapMut;

use crate::error::RingBufferError;

// Memory layout constants
const HEAD_OFFSET: usize = 0;
const TAIL_OFFSET: usize = 128;
const META_OFFSET: usize = 256;
const SLOTS_OFFSET: usize = 512;

/// Message header size within each slot (u32 LE length prefix).
const MSG_HEADER_SIZE: usize = 4;

/// SPSC ring buffer backed by anonymous mmap.
///
/// Memory layout:
/// ```text
/// Offset 0:    [head: AtomicU64] [padding to 128 bytes]
/// Offset 128:  [tail: AtomicU64] [padding to 128 bytes]
/// Offset 256:  [slot_count: u64] [slot_size: u64]
/// Offset 272:  [padding to 512]
/// Offset 512:  [Slot 0: [msg_len: u32 LE][payload][padding to slot_size]]
///              [Slot 1: ...]
/// ```
pub struct RingBuffer {
    mmap: MmapMut,
    slot_count: usize,
    slot_size: usize,
    mask: usize,
}

/// Producer handle for the ring buffer. Only one should exist at a time.
pub struct Producer {
    /// Raw pointer to the start of the mmap region.
    base: *mut u8,
    slot_count: usize,
    slot_size: usize,
    mask: usize,
    /// Maximum payload bytes per slot.
    max_payload: usize,
}

/// Consumer handle for the ring buffer. Only one should exist at a time.
pub struct Consumer {
    base: *mut u8,
    slot_size: usize,
    mask: usize,
    max_payload: usize,
}

// SAFETY: The SPSC contract guarantees that Producer and Consumer access
// disjoint cache lines for their respective head/tail pointers, and slots
// are only written by the producer before publishing and only read by the
// consumer after observing the publish. The mmap region outlives both handles
// because `RingBuffer` owns it and `split` borrows `&mut self`.
unsafe impl Send for Producer {}
unsafe impl Send for Consumer {}

impl RingBuffer {
    /// Create a new ring buffer backed by anonymous mmap.
    ///
    /// - `slot_count` must be a power of 2.
    /// - `slot_size` is the total bytes per slot (including the 4-byte length header).
    pub fn new(slot_count: usize, slot_size: usize) -> Result<Self, RingBufferError> {
        if !slot_count.is_power_of_two() || slot_count == 0 {
            return Err(RingBufferError::InvalidSlotCount);
        }

        let total_size = SLOTS_OFFSET + slot_count * slot_size;
        let mmap = MmapMut::map_anon(total_size)?;

        let mut rb = Self {
            mmap,
            slot_count,
            slot_size,
            mask: slot_count - 1,
        };

        // Initialize header
        rb.head().store(0, Ordering::Relaxed);
        rb.tail().store(0, Ordering::Relaxed);

        // Write metadata
        let meta = &mut rb.mmap[META_OFFSET..META_OFFSET + 16];
        meta[..8].copy_from_slice(&(slot_count as u64).to_le_bytes());
        meta[8..16].copy_from_slice(&(slot_size as u64).to_le_bytes());

        Ok(rb)
    }

    /// Split into producer and consumer handles.
    ///
    /// # Safety contract
    /// Caller must ensure only one `Producer` and one `Consumer` exist concurrently.
    /// The returned handles borrow the `RingBuffer` mutably, so the borrow checker
    /// enforces that no further splits occur while handles are alive.
    pub fn split(&mut self) -> (Producer, Consumer) {
        let base = self.mmap.as_mut_ptr();
        let max_payload = self.slot_size.saturating_sub(MSG_HEADER_SIZE);
        (
            Producer {
                base,
                slot_count: self.slot_count,
                slot_size: self.slot_size,
                mask: self.mask,
                max_payload,
            },
            Consumer {
                base,
                slot_size: self.slot_size,
                mask: self.mask,
                max_payload,
            },
        )
    }

    fn head(&self) -> &AtomicU64 {
        // SAFETY: HEAD_OFFSET is within the mmap, and we guarantee alignment.
        unsafe { &*(self.mmap.as_ptr().add(HEAD_OFFSET) as *const AtomicU64) }
    }

    fn tail(&self) -> &AtomicU64 {
        // SAFETY: TAIL_OFFSET is within the mmap, and we guarantee alignment.
        unsafe { &*(self.mmap.as_ptr().add(TAIL_OFFSET) as *const AtomicU64) }
    }
}

// ---------------------------------------------------------------------------
// Helper functions shared by Producer / Consumer
// ---------------------------------------------------------------------------

#[inline(always)]
fn head_ref(base: *mut u8) -> &'static AtomicU64 {
    unsafe { &*(base.add(HEAD_OFFSET) as *const AtomicU64) }
}

#[inline(always)]
fn tail_ref(base: *mut u8) -> &'static AtomicU64 {
    unsafe { &*(base.add(TAIL_OFFSET) as *const AtomicU64) }
}

#[inline(always)]
fn slot_ptr(base: *mut u8, index: usize, slot_size: usize) -> *mut u8 {
    unsafe { base.add(SLOTS_OFFSET + index * slot_size) }
}

// ---------------------------------------------------------------------------
// Producer
// ---------------------------------------------------------------------------

impl Producer {
    /// Try to write `data` into the next slot.
    /// Returns the sequence number of the written slot, or `WouldBlock` if full.
    pub fn try_push(&self, data: &[u8]) -> Result<u64, RingBufferError> {
        if data.len() > self.max_payload {
            return Err(RingBufferError::MessageTooLarge {
                size: data.len(),
                slot_size: self.slot_size,
            });
        }

        let tail = tail_ref(self.base).load(Ordering::Relaxed);
        let head = head_ref(self.base).load(Ordering::Acquire);

        // Buffer is full when tail - head == slot_count
        if (tail.wrapping_sub(head)) as usize >= self.slot_count {
            return Err(RingBufferError::WouldBlock);
        }

        let idx = (tail as usize) & self.mask;
        let slot = slot_ptr(self.base, idx, self.slot_size);

        // Write message length (u32 LE) then payload
        unsafe {
            let len_bytes = (data.len() as u32).to_le_bytes();
            std::ptr::copy_nonoverlapping(len_bytes.as_ptr(), slot, MSG_HEADER_SIZE);
            std::ptr::copy_nonoverlapping(data.as_ptr(), slot.add(MSG_HEADER_SIZE), data.len());
        }

        // Publish: store tail + 1 with Release so the consumer sees the written data.
        tail_ref(self.base).store(tail.wrapping_add(1), Ordering::Release);

        Ok(tail)
    }

    /// Blocking push -- spin-waits if the buffer is full.
    pub fn push(&self, data: &[u8]) -> Result<u64, RingBufferError> {
        loop {
            match self.try_push(data) {
                Err(RingBufferError::WouldBlock) => {
                    std::hint::spin_loop();
                }
                other => return other,
            }
        }
    }

    /// Batch write multiple messages. Returns the number of messages successfully written.
    /// Stops at the first slot that cannot be written (buffer full or message too large).
    pub fn try_push_batch(&self, items: &[&[u8]]) -> Result<usize, RingBufferError> {
        let mut count = 0usize;
        for item in items {
            match self.try_push(item) {
                Ok(_) => count += 1,
                Err(RingBufferError::WouldBlock) if count > 0 => break,
                Err(e) => return Err(e),
            }
        }
        Ok(count)
    }
}

// ---------------------------------------------------------------------------
// Consumer
// ---------------------------------------------------------------------------

impl Consumer {
    /// Try to read the next message into `buf`.
    /// Returns the number of payload bytes read, or `Empty` if no message available.
    pub fn try_pop(&self, buf: &mut [u8]) -> Result<usize, RingBufferError> {
        let head = head_ref(self.base).load(Ordering::Relaxed);
        let tail = tail_ref(self.base).load(Ordering::Acquire);

        if head == tail {
            return Err(RingBufferError::Empty);
        }

        let idx = (head as usize) & self.mask;
        let slot = slot_ptr(self.base, idx, self.slot_size);

        let msg_len = unsafe {
            let mut len_buf = [0u8; 4];
            std::ptr::copy_nonoverlapping(slot, len_buf.as_mut_ptr(), MSG_HEADER_SIZE);
            u32::from_le_bytes(len_buf) as usize
        };

        let copy_len = msg_len.min(buf.len()).min(self.max_payload);
        unsafe {
            std::ptr::copy_nonoverlapping(slot.add(MSG_HEADER_SIZE), buf.as_mut_ptr(), copy_len);
        }

        // Advance head with Release so the producer sees the freed slot.
        head_ref(self.base).store(head.wrapping_add(1), Ordering::Release);

        Ok(copy_len)
    }

    /// Blocking pop -- spin-waits if empty.
    pub fn pop(&self, buf: &mut [u8]) -> Result<usize, RingBufferError> {
        loop {
            match self.try_pop(buf) {
                Err(RingBufferError::Empty) => {
                    std::hint::spin_loop();
                }
                other => return other,
            }
        }
    }

    /// Peek at the next message without consuming it.
    pub fn peek(&self, buf: &mut [u8]) -> Result<usize, RingBufferError> {
        let head = head_ref(self.base).load(Ordering::Relaxed);
        let tail = tail_ref(self.base).load(Ordering::Acquire);

        if head == tail {
            return Err(RingBufferError::Empty);
        }

        let idx = (head as usize) & self.mask;
        let slot = slot_ptr(self.base, idx, self.slot_size);

        let msg_len = unsafe {
            let mut len_buf = [0u8; 4];
            std::ptr::copy_nonoverlapping(slot, len_buf.as_mut_ptr(), MSG_HEADER_SIZE);
            u32::from_le_bytes(len_buf) as usize
        };

        let copy_len = msg_len.min(buf.len()).min(self.max_payload);
        unsafe {
            std::ptr::copy_nonoverlapping(slot.add(MSG_HEADER_SIZE), buf.as_mut_ptr(), copy_len);
        }

        // Do NOT advance head -- this is peek.
        Ok(copy_len)
    }

    /// Batch read up to `max` messages. Returns the number of messages read.
    /// Each message is appended into the corresponding entry of `bufs`.
    pub fn try_pop_batch(
        &self,
        bufs: &mut [Vec<u8>],
        max: usize,
    ) -> Result<usize, RingBufferError> {
        let limit = max.min(bufs.len());
        let mut count = 0usize;

        for entry in bufs.iter_mut().take(limit) {
            let head = head_ref(self.base).load(Ordering::Relaxed);
            let tail = tail_ref(self.base).load(Ordering::Acquire);

            if head == tail {
                break;
            }

            let idx = (head as usize) & self.mask;
            let slot = slot_ptr(self.base, idx, self.slot_size);

            let msg_len = unsafe {
                let mut len_buf = [0u8; 4];
                std::ptr::copy_nonoverlapping(slot, len_buf.as_mut_ptr(), MSG_HEADER_SIZE);
                u32::from_le_bytes(len_buf) as usize
            };

            let copy_len = msg_len.min(self.max_payload);
            entry.resize(copy_len, 0);
            unsafe {
                std::ptr::copy_nonoverlapping(
                    slot.add(MSG_HEADER_SIZE),
                    entry.as_mut_ptr(),
                    copy_len,
                );
            }

            head_ref(self.base).store(head.wrapping_add(1), Ordering::Release);
            count += 1;
        }

        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SLOT_COUNT: usize = 16;
    const SLOT_SIZE: usize = 256;

    // 1. Single-thread correctness
    #[test]
    fn test_push_pop_correctness() {
        let mut rb = RingBuffer::new(SLOT_COUNT, SLOT_SIZE).unwrap();
        let (producer, consumer) = rb.split();

        for i in 0u64..10 {
            let msg = format!("message-{i}");
            producer.try_push(msg.as_bytes()).unwrap();
        }

        for i in 0u64..10 {
            let mut buf = [0u8; 256];
            let n = consumer.try_pop(&mut buf).unwrap();
            let expected = format!("message-{i}");
            assert_eq!(&buf[..n], expected.as_bytes());
        }
    }

    // 2. Full / empty boundary
    #[test]
    fn test_full_empty_boundary() {
        let mut rb = RingBuffer::new(4, SLOT_SIZE).unwrap();
        let (producer, consumer) = rb.split();

        // Fill it up
        for i in 0..4 {
            let msg = format!("m{i}");
            producer.try_push(msg.as_bytes()).unwrap();
        }

        // Should be full
        assert!(matches!(
            producer.try_push(b"overflow"),
            Err(RingBufferError::WouldBlock)
        ));

        // Drain
        for _ in 0..4 {
            let mut buf = [0u8; 64];
            consumer.try_pop(&mut buf).unwrap();
        }

        // Should be empty
        assert!(matches!(
            consumer.try_pop(&mut [0u8; 64]),
            Err(RingBufferError::Empty)
        ));
    }

    // 3. Wraparound
    #[test]
    fn test_wraparound() {
        let mut rb = RingBuffer::new(4, SLOT_SIZE).unwrap();
        let (producer, consumer) = rb.split();

        // Push and pop repeatedly to force wraparound multiple times
        for round in 0..10 {
            for i in 0..4 {
                let msg = format!("r{round}-m{i}");
                producer.try_push(msg.as_bytes()).unwrap();
            }
            for i in 0..4 {
                let mut buf = [0u8; 64];
                let n = consumer.try_pop(&mut buf).unwrap();
                let expected = format!("r{round}-m{i}");
                assert_eq!(&buf[..n], expected.as_bytes());
            }
        }
    }

    // 4. Message too large
    #[test]
    fn test_message_too_large() {
        let mut rb = RingBuffer::new(4, 32).unwrap();
        let (producer, _consumer) = rb.split();

        // max payload = 32 - 4 = 28 bytes
        let big = vec![0u8; 29];
        assert!(matches!(
            producer.try_push(&big),
            Err(RingBufferError::MessageTooLarge { .. })
        ));

        // Exactly 28 bytes should work
        let exact = vec![0u8; 28];
        producer.try_push(&exact).unwrap();
    }

    // 5. Multi-thread SPSC
    #[test]
    fn test_spsc_multithread() {
        const MSG_COUNT: u64 = 100_000;
        let mut rb = RingBuffer::new(1024, 128).unwrap();
        let (producer, consumer) = rb.split();

        let producer_handle = std::thread::spawn(move || {
            for seq in 0..MSG_COUNT {
                let msg = seq.to_le_bytes();
                producer.push(&msg).unwrap();
            }
        });

        let consumer_handle = std::thread::spawn(move || {
            let mut received = Vec::with_capacity(MSG_COUNT as usize);
            let mut buf = [0u8; 128];
            for _ in 0..MSG_COUNT {
                let n = consumer.pop(&mut buf).unwrap();
                assert_eq!(n, 8);
                let val = u64::from_le_bytes(buf[..8].try_into().unwrap());
                received.push(val);
            }
            received
        });

        producer_handle.join().unwrap();
        let received = consumer_handle.join().unwrap();

        assert_eq!(received.len(), MSG_COUNT as usize);
        for (i, &val) in received.iter().enumerate() {
            assert_eq!(val, i as u64, "mismatch at index {i}");
        }
    }

    // 6. Batch operations
    #[test]
    fn test_batch_push_pop() {
        let mut rb = RingBuffer::new(8, SLOT_SIZE).unwrap();
        let (producer, consumer) = rb.split();

        let items: Vec<&[u8]> = vec![b"aaa", b"bbb", b"ccc", b"ddd"];
        let pushed = producer.try_push_batch(&items).unwrap();
        assert_eq!(pushed, 4);

        let mut bufs = vec![Vec::new(); 8];
        let popped = consumer.try_pop_batch(&mut bufs, 8).unwrap();
        assert_eq!(popped, 4);
        assert_eq!(bufs[0], b"aaa");
        assert_eq!(bufs[1], b"bbb");
        assert_eq!(bufs[2], b"ccc");
        assert_eq!(bufs[3], b"ddd");
    }

    // 7. Peek
    #[test]
    fn test_peek_does_not_consume() {
        let mut rb = RingBuffer::new(4, SLOT_SIZE).unwrap();
        let (producer, consumer) = rb.split();

        producer.try_push(b"hello").unwrap();

        // Peek should return the message
        let mut buf = [0u8; 64];
        let n = consumer.peek(&mut buf).unwrap();
        assert_eq!(&buf[..n], b"hello");

        // Peek again -- same message still there
        let mut buf2 = [0u8; 64];
        let n2 = consumer.peek(&mut buf2).unwrap();
        assert_eq!(&buf2[..n2], b"hello");

        // Pop should return same message and consume it
        let mut buf3 = [0u8; 64];
        let n3 = consumer.try_pop(&mut buf3).unwrap();
        assert_eq!(&buf3[..n3], b"hello");

        // Now empty
        assert!(matches!(
            consumer.peek(&mut [0u8; 64]),
            Err(RingBufferError::Empty)
        ));
    }

    // Invalid slot count
    #[test]
    fn test_invalid_slot_count() {
        assert!(matches!(
            RingBuffer::new(3, 256),
            Err(RingBufferError::InvalidSlotCount)
        ));
        assert!(matches!(
            RingBuffer::new(0, 256),
            Err(RingBufferError::InvalidSlotCount)
        ));
    }
}
