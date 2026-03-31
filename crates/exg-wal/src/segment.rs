use std::path::{Path, PathBuf};

use crate::error::WalError;

/// Record header: [sequence: u64 LE] [payload_len: u32 LE] = 12 bytes
/// Record trailer: [crc32: u32 LE] = 4 bytes
/// Total overhead: 16 bytes
pub const HEADER_SIZE: usize = 12;
pub const TRAILER_SIZE: usize = 4;
pub const RECORD_OVERHEAD: usize = HEADER_SIZE + TRAILER_SIZE;

/// Maximum payload size: 256MB - overhead (practical limit)
pub const MAX_PAYLOAD_SIZE: usize = 256 * 1024 * 1024;

/// Segment file name prefix/suffix
const SEGMENT_PREFIX: &str = "wal-";
const SEGMENT_SUFFIX: &str = ".log";

/// Format a segment file name from its first sequence number.
pub fn segment_filename(first_sequence: u64) -> String {
    format!("{SEGMENT_PREFIX}{first_sequence:020}{SEGMENT_SUFFIX}")
}

/// Parse the first sequence number from a segment file name.
/// Returns `None` if the file name doesn't match the expected pattern.
pub fn parse_segment_filename(name: &str) -> Option<u64> {
    let name = name.strip_prefix(SEGMENT_PREFIX)?;
    let name = name.strip_suffix(SEGMENT_SUFFIX)?;
    name.parse().ok()
}

/// List all segment files in the directory, sorted by first sequence.
pub fn list_segments(dir: &Path) -> Result<Vec<(u64, PathBuf)>, WalError> {
    let mut segments = Vec::new();

    if !dir.exists() {
        return Ok(segments);
    }

    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let file_name = entry.file_name();
        let name = file_name.to_string_lossy();
        if let Some(seq) = parse_segment_filename(&name) {
            segments.push((seq, entry.path()));
        }
    }

    segments.sort_by_key(|(seq, _)| *seq);
    Ok(segments)
}

/// Compute CRC32 over the header + payload bytes.
pub fn compute_crc(sequence: u64, payload: &[u8]) -> u32 {
    let mut hasher = crc32fast::Hasher::new();
    hasher.update(&sequence.to_le_bytes());
    hasher.update(&(payload.len() as u32).to_le_bytes());
    hasher.update(payload);
    hasher.finalize()
}

/// Encode a record into a pre-allocated buffer.
/// Buffer must be at least `RECORD_OVERHEAD + payload.len()` bytes.
pub fn encode_record(buf: &mut [u8], sequence: u64, payload: &[u8]) {
    let payload_len = payload.len() as u32;
    buf[..8].copy_from_slice(&sequence.to_le_bytes());
    buf[8..12].copy_from_slice(&payload_len.to_le_bytes());
    buf[12..12 + payload.len()].copy_from_slice(payload);
    let crc = compute_crc(sequence, payload);
    let crc_offset = 12 + payload.len();
    buf[crc_offset..crc_offset + 4].copy_from_slice(&crc.to_le_bytes());
}

/// Decode a record from a byte slice starting at `offset`.
/// Returns `(sequence, payload_slice, bytes_consumed)` or an error.
pub fn decode_record(data: &[u8], offset: usize) -> Result<(u64, &[u8], usize), DecodeError> {
    let remaining = data.len() - offset;

    if remaining < HEADER_SIZE {
        return Err(DecodeError::Incomplete);
    }

    let sequence = u64::from_le_bytes(data[offset..offset + 8].try_into().unwrap());
    let payload_len =
        u32::from_le_bytes(data[offset + 8..offset + 12].try_into().unwrap()) as usize;

    let total = RECORD_OVERHEAD + payload_len;
    if remaining < total {
        return Err(DecodeError::Incomplete);
    }

    let payload = &data[offset + 12..offset + 12 + payload_len];
    let stored_crc = u32::from_le_bytes(
        data[offset + 12 + payload_len..offset + total]
            .try_into()
            .unwrap(),
    );

    let computed_crc = compute_crc(sequence, payload);
    if stored_crc != computed_crc {
        return Err(DecodeError::CrcMismatch { sequence });
    }

    Ok((sequence, payload, total))
}

/// Errors during record decoding.
#[derive(Debug)]
pub enum DecodeError {
    /// Not enough bytes to form a complete record (partial write).
    Incomplete,
    /// CRC mismatch — corrupt record.
    CrcMismatch { sequence: u64 },
}
