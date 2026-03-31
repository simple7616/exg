use std::path::{Path, PathBuf};

use memmap2::Mmap;

use crate::error::WalError;
use crate::segment::{DecodeError, decode_record, list_segments};
use crate::snapshot;

pub struct WalReader {
    /// Directory containing WAL segments.
    ///
    /// # Safety contract
    ///
    /// `WalReader` uses mmap for fast sequential reads. It must NOT be used
    /// concurrently with a `WalWriter` that may truncate or rotate the same
    /// segment files. In the exchange architecture, reads happen either:
    /// - During startup recovery (writer not yet active), or
    /// - On completed/sealed segments (writer has moved to a new segment).
    dir: PathBuf,
}

impl WalReader {
    pub fn open(dir: &Path) -> Result<Self, WalError> {
        Ok(Self {
            dir: dir.to_path_buf(),
        })
    }

    /// Read records starting from `start_sequence`.
    /// Calls `callback` for each record. Returns the number of records read.
    pub fn read_from<F>(&mut self, start_sequence: u64, mut callback: F) -> Result<u64, WalError>
    where
        F: FnMut(u64, &[u8]) -> bool,
    {
        let segments = list_segments(&self.dir)?;

        if segments.is_empty() {
            return Ok(0);
        }

        let mut records_read: u64 = 0;

        // Find the first segment that could contain start_sequence.
        // The segment with the largest first_seq <= start_sequence.
        let start_idx = match segments.binary_search_by_key(&start_sequence, |(seq, _)| *seq) {
            Ok(i) => i,
            Err(0) => 0,
            Err(i) => i - 1,
        };

        for (_, path) in &segments[start_idx..] {
            let file = std::fs::File::open(path)?;
            let file_len = file.metadata()?.len() as usize;

            if file_len == 0 {
                continue;
            }

            // SAFETY: read-only mmap of a file we just opened
            let mmap = unsafe { Mmap::map(&file)? };

            let mut offset = 0;
            loop {
                if offset >= mmap.len() {
                    break;
                }

                match decode_record(&mmap, offset) {
                    Ok((seq, payload, consumed)) => {
                        offset += consumed;

                        if seq < start_sequence {
                            continue;
                        }

                        records_read += 1;
                        if !callback(seq, payload) {
                            return Ok(records_read);
                        }
                    }
                    Err(DecodeError::Incomplete) => {
                        // Partial record at end — stop reading this segment
                        break;
                    }
                    Err(DecodeError::CrcMismatch { sequence }) => {
                        return Err(WalError::Corrupt {
                            sequence,
                            reason: "CRC mismatch".to_string(),
                        });
                    }
                }
            }
        }

        Ok(records_read)
    }

    /// Load the latest snapshot. Returns (sequence, data) or None.
    pub fn load_latest_snapshot(&self) -> Result<Option<(u64, Vec<u8>)>, WalError> {
        snapshot::load_latest_snapshot(&self.dir)
    }
}
