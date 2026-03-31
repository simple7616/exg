use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use crate::error::WalError;

const SNAPSHOT_PREFIX: &str = "snapshot-";
const SNAPSHOT_SUFFIX: &str = ".snap";
const MAX_SNAPSHOTS_KEPT: usize = 3;

/// CRC32 of snapshot data, stored as 4-byte LE trailer.
const SNAPSHOT_CRC_SIZE: usize = 4;

/// Format a snapshot file name.
pub fn snapshot_filename(sequence: u64) -> String {
    format!("{SNAPSHOT_PREFIX}{sequence:020}{SNAPSHOT_SUFFIX}")
}

/// Parse sequence from a snapshot file name.
fn parse_snapshot_filename(name: &str) -> Option<u64> {
    let name = name.strip_prefix(SNAPSHOT_PREFIX)?;
    let name = name.strip_suffix(SNAPSHOT_SUFFIX)?;
    name.parse().ok()
}

/// List all snapshot files sorted by sequence (ascending).
fn list_snapshots(dir: &Path) -> Result<Vec<(u64, PathBuf)>, WalError> {
    let mut snapshots = Vec::new();

    if !dir.exists() {
        return Ok(snapshots);
    }

    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let file_name = entry.file_name();
        let name = file_name.to_string_lossy();
        if let Some(seq) = parse_snapshot_filename(&name) {
            snapshots.push((seq, entry.path()));
        }
    }

    snapshots.sort_by_key(|(seq, _)| *seq);
    Ok(snapshots)
}

/// Save a snapshot atomically: write to temp file with CRC, fsync, rename, fsync dir.
pub fn save_snapshot(dir: &Path, sequence: u64, data: &[u8]) -> Result<(), WalError> {
    let final_path = dir.join(snapshot_filename(sequence));
    let tmp_path = dir.join(format!("{}.tmp", snapshot_filename(sequence)));

    // Write data + CRC32 trailer to temp file
    let crc = crc32fast::hash(data);
    let file = File::create(&tmp_path)?;
    let mut writer = BufWriter::new(file);
    writer.write_all(data)?;
    writer.write_all(&crc.to_le_bytes())?;
    writer.flush()?;
    // fsync data + metadata before rename
    writer.into_inner().map_err(|e| e.into_error())?.sync_all()?;

    // Atomic rename
    fs::rename(&tmp_path, &final_path)?;

    // fsync directory to ensure rename is durable
    if let Ok(dir_file) = File::open(dir) {
        let _ = dir_file.sync_all();
    }

    // Clean up old snapshots, keeping the latest MAX_SNAPSHOTS_KEPT
    cleanup_old_snapshots(dir)?;

    Ok(())
}

/// Load the latest snapshot with CRC integrity check.
/// Returns (sequence, data) or None if no snapshots exist.
pub fn load_latest_snapshot(dir: &Path) -> Result<Option<(u64, Vec<u8>)>, WalError> {
    let snapshots = list_snapshots(dir)?;
    match snapshots.last() {
        Some((seq, path)) => {
            let raw = fs::read(path)?;
            if raw.len() < SNAPSHOT_CRC_SIZE {
                return Err(WalError::Corrupt {
                    sequence: *seq,
                    reason: "snapshot too small to contain CRC".to_string(),
                });
            }
            let (data, crc_bytes) = raw.split_at(raw.len() - SNAPSHOT_CRC_SIZE);
            let stored_crc = u32::from_le_bytes(crc_bytes.try_into().unwrap());
            let computed_crc = crc32fast::hash(data);
            if stored_crc != computed_crc {
                return Err(WalError::Corrupt {
                    sequence: *seq,
                    reason: format!(
                        "snapshot CRC mismatch: stored={stored_crc:#010x}, computed={computed_crc:#010x}"
                    ),
                });
            }
            Ok(Some((*seq, data.to_vec())))
        }
        None => Ok(None),
    }
}

/// Remove old snapshots, keeping only the latest `MAX_SNAPSHOTS_KEPT`.
fn cleanup_old_snapshots(dir: &Path) -> Result<(), WalError> {
    let snapshots = list_snapshots(dir)?;
    if snapshots.len() > MAX_SNAPSHOTS_KEPT {
        let to_remove = snapshots.len() - MAX_SNAPSHOTS_KEPT;
        for (_, path) in &snapshots[..to_remove] {
            let _ = fs::remove_file(path);
        }
    }
    Ok(())
}
