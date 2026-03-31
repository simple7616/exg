use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::PathBuf;

use crate::error::WalError;
use crate::segment::{
    self, DecodeError, RECORD_OVERHEAD, decode_record, encode_record, list_segments,
    segment_filename,
};
use crate::snapshot;
use crate::WalConfig;

pub struct WalWriter {
    config: WalConfig,
    /// Current segment's BufWriter
    writer: BufWriter<File>,
    /// Current segment's file path
    current_segment_path: PathBuf,
    /// First sequence number in the current segment
    current_segment_first_seq: u64,
    /// Bytes written to the current segment
    current_segment_bytes: usize,
    /// Next sequence number to assign
    next_sequence: u64,
    /// Number of records since last flush
    records_since_flush: usize,
}

impl WalWriter {
    pub fn open(config: WalConfig) -> Result<Self, WalError> {
        fs::create_dir_all(&config.dir)?;

        let segments = list_segments(&config.dir)?;

        // Scan all existing segments to find the last valid sequence and recover state
        let (next_sequence, truncate_info) = Self::recover_state(&segments)?;

        // If we need to truncate a partially-written record at the END of the last segment
        if let Some((path, valid_size)) = &truncate_info {
            let file = OpenOptions::new().write(true).open(path)?;
            file.set_len(*valid_size as u64)?;
            // Use sync_all (not sync_data) to ensure metadata (file size) is durable
            file.sync_all()?;
        }

        // Determine current segment
        let (segment_first_seq, segment_path, segment_bytes) = if let Some((first_seq, path)) =
            segments.last()
        {
            let file_len = if let Some((truncate_path, valid_size)) = &truncate_info {
                if truncate_path == path {
                    *valid_size
                } else {
                    fs::metadata(path)?.len() as usize
                }
            } else {
                fs::metadata(path)?.len() as usize
            };

            // Check if the current segment is full
            if file_len >= config.segment_size {
                // Start a new segment
                let new_path = config.dir.join(segment_filename(next_sequence));
                (next_sequence, new_path, 0)
            } else {
                (*first_seq, path.clone(), file_len)
            }
        } else {
            // No existing segments — create the first one
            let path = config.dir.join(segment_filename(0));
            (0, path, 0)
        };

        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&segment_path)?;
        let writer = BufWriter::new(file);

        Ok(Self {
            config,
            writer,
            current_segment_path: segment_path,
            current_segment_first_seq: segment_first_seq,
            current_segment_bytes: segment_bytes,
            next_sequence,
            records_since_flush: 0,
        })
    }

    /// Append a record. Returns the assigned sequence number.
    pub fn append(&mut self, payload: &[u8]) -> Result<u64, WalError> {
        if payload.len() > segment::MAX_PAYLOAD_SIZE {
            return Err(WalError::PayloadTooLarge {
                size: payload.len(),
            });
        }

        let record_size = RECORD_OVERHEAD + payload.len();

        // Rotate segment if needed
        if self.current_segment_bytes > 0
            && self.current_segment_bytes + record_size > self.config.segment_size
        {
            self.flush()?;
            self.rotate_segment()?;
        }

        let seq = self.next_sequence;
        let mut buf = vec![0u8; record_size];
        encode_record(&mut buf, seq, payload);

        self.writer.write_all(&buf)?;
        self.current_segment_bytes += record_size;
        self.next_sequence += 1;
        self.records_since_flush += 1;

        // Auto-flush based on flush_every_n
        if self.records_since_flush >= self.config.flush_every_n {
            self.flush()?;
        }

        Ok(seq)
    }

    /// Flush buffered writes to disk.
    pub fn flush(&mut self) -> Result<(), WalError> {
        self.writer.flush()?;
        self.writer.get_ref().sync_data()?;
        self.records_since_flush = 0;
        Ok(())
    }

    /// Get the current sequence number (next to be assigned).
    pub fn current_sequence(&self) -> u64 {
        self.next_sequence
    }

    /// Save a snapshot at the given sequence number.
    pub fn save_snapshot(&mut self, sequence: u64, data: &[u8]) -> Result<(), WalError> {
        snapshot::save_snapshot(&self.config.dir, sequence, data)
    }

    /// Rotate to a new segment file.
    fn rotate_segment(&mut self) -> Result<(), WalError> {
        let new_first_seq = self.next_sequence;
        let new_path = self.config.dir.join(segment_filename(new_first_seq));

        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&new_path)?;
        self.writer = BufWriter::new(file);
        self.current_segment_path = new_path;
        self.current_segment_first_seq = new_first_seq;
        self.current_segment_bytes = 0;

        Ok(())
    }

    /// Scan all segments to find the next valid sequence and detect partial writes.
    ///
    /// Only the LAST segment's trailing corrupt/incomplete records are truncated.
    /// CRC errors in any non-trailing position indicate confirmed data loss and
    /// return `WalError::Corrupt`.
    fn recover_state(
        segments: &[(u64, PathBuf)],
    ) -> Result<(u64, Option<(PathBuf, usize)>), WalError> {
        let mut next_sequence: u64 = 0;
        let num_segments = segments.len();

        for (seg_idx, (_, path)) in segments.iter().enumerate() {
            let data = fs::read(path)?;
            let mut offset = 0;
            let is_last_segment = seg_idx == num_segments - 1;

            loop {
                if offset >= data.len() {
                    break;
                }

                match decode_record(&data, offset) {
                    Ok((seq, _, consumed)) => {
                        // Validate sequence continuity
                        if seq != next_sequence {
                            return Err(WalError::SequenceGap {
                                expected: next_sequence,
                                found: seq,
                            });
                        }
                        next_sequence = seq + 1;
                        offset += consumed;
                    }
                    Err(DecodeError::Incomplete | DecodeError::CrcMismatch { .. }) => {
                        if is_last_segment {
                            // Trailing partial/corrupt record in last segment — safe to truncate
                            return Ok((next_sequence, Some((path.clone(), offset))));
                        }
                        // CRC error in non-last segment = confirmed data loss
                        let sequence = match decode_record(&data, offset) {
                            Err(DecodeError::CrcMismatch { sequence }) => sequence,
                            _ => next_sequence,
                        };
                        return Err(WalError::Corrupt {
                            sequence,
                            reason: format!(
                                "corrupt record in non-trailing segment {}",
                                path.display()
                            ),
                        });
                    }
                }
            }
        }

        Ok((next_sequence, None))
    }
}
