pub mod error;
pub mod reader;
pub mod segment;
pub mod snapshot;
pub mod writer;

use std::path::PathBuf;

pub use error::WalError;
pub use reader::WalReader;
pub use writer::WalWriter;

/// Configuration for the WAL.
pub struct WalConfig {
    pub dir: PathBuf,
    /// Maximum segment file size in bytes. Default: 256MB.
    pub segment_size: usize,
    /// Flush interval in microseconds. Default: 1000 (1ms).
    pub flush_interval_us: u64,
    /// Flush after every N appended events. Default: 1000.
    pub flush_every_n: usize,
}

impl Default for WalConfig {
    fn default() -> Self {
        Self {
            dir: PathBuf::from("wal"),
            segment_size: 256 * 1024 * 1024,
            flush_interval_us: 1000,
            flush_every_n: 1000,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn test_config(dir: &std::path::Path) -> WalConfig {
        WalConfig {
            dir: dir.to_path_buf(),
            segment_size: 256 * 1024 * 1024,
            flush_interval_us: 1000,
            flush_every_n: 10000, // large so we control flush manually
        }
    }

    /// Test 1: Write-read roundtrip
    #[test]
    fn test_write_read_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let config = test_config(tmp.path());

        let payloads: Vec<Vec<u8>> = (0..100)
            .map(|i| format!("record-{i}").into_bytes())
            .collect();

        {
            let mut w = WalWriter::open(config).unwrap();
            for (i, p) in payloads.iter().enumerate() {
                let seq = w.append(p).unwrap();
                assert_eq!(seq, i as u64);
            }
            w.flush().unwrap();
        }

        let mut r = WalReader::open(tmp.path()).unwrap();
        let mut read_payloads = Vec::new();
        let count = r
            .read_from(0, |seq, payload| {
                assert_eq!(seq, read_payloads.len() as u64);
                read_payloads.push(payload.to_vec());
                true
            })
            .unwrap();

        assert_eq!(count, 100);
        assert_eq!(read_payloads, payloads);
    }

    /// Test 2: Segment rotation
    #[test]
    fn test_segment_rotation() {
        let tmp = TempDir::new().unwrap();
        let config = WalConfig {
            dir: tmp.path().to_path_buf(),
            segment_size: 200, // very small to force rotation
            flush_interval_us: 1000,
            flush_every_n: 10000,
        };

        let num_records = 50;
        let payload = b"test-payload-data";

        {
            let mut w = WalWriter::open(config).unwrap();
            for _ in 0..num_records {
                w.append(payload).unwrap();
            }
            w.flush().unwrap();
        }

        // Verify multiple segment files exist
        let segments = segment::list_segments(tmp.path()).unwrap();
        assert!(
            segments.len() > 1,
            "expected multiple segments, got {}",
            segments.len()
        );

        // Verify all records readable across segments
        let mut r = WalReader::open(tmp.path()).unwrap();
        let mut count = 0u64;
        let total = r
            .read_from(0, |seq, p| {
                assert_eq!(seq, count);
                assert_eq!(p, payload);
                count += 1;
                true
            })
            .unwrap();
        assert_eq!(total, num_records);
    }

    /// Test 3: CRC validation
    #[test]
    fn test_crc_validation() {
        let tmp = TempDir::new().unwrap();
        let config = test_config(tmp.path());

        {
            let mut w = WalWriter::open(config).unwrap();
            w.append(b"valid-record-0").unwrap();
            w.append(b"valid-record-1").unwrap();
            w.append(b"valid-record-2").unwrap();
            w.flush().unwrap();
        }

        // Corrupt a byte in the middle of the second record
        let segments = segment::list_segments(tmp.path()).unwrap();
        let (_, seg_path) = &segments[0];
        let mut data = fs::read(seg_path).unwrap();
        // The first record: 8(seq) + 4(len) + 14(payload) + 4(crc) = 30 bytes
        // Corrupt a payload byte in the second record
        let corrupt_offset = 30 + 12 + 2; // into second record's payload
        data[corrupt_offset] ^= 0xFF;
        fs::write(seg_path, &data).unwrap();

        let mut r = WalReader::open(tmp.path()).unwrap();
        let result = r.read_from(0, |_seq, _payload| true);
        assert!(result.is_err());
        match result {
            Err(WalError::Corrupt { sequence, .. }) => {
                assert_eq!(sequence, 1);
            }
            other => panic!("expected Corrupt error, got {other:?}"),
        }
    }

    /// Test 4: Crash recovery (truncation)
    #[test]
    fn test_crash_recovery_truncation() {
        let tmp = TempDir::new().unwrap();

        // Write 5 records
        {
            let config = test_config(tmp.path());
            let mut w = WalWriter::open(config).unwrap();
            for i in 0..5u64 {
                let payload = format!("record-{i}");
                w.append(payload.as_bytes()).unwrap();
            }
            w.flush().unwrap();
        }

        // Truncate the file mid-record (remove last few bytes)
        let segments = segment::list_segments(tmp.path()).unwrap();
        let (_, seg_path) = &segments[0];
        let data = fs::read(seg_path).unwrap();
        let truncated_len = data.len() - 5; // chop off part of last record
        fs::write(seg_path, &data[..truncated_len]).unwrap();

        // Reopen writer — should recover and truncate the partial record
        {
            let config = test_config(tmp.path());
            let mut w = WalWriter::open(config).unwrap();
            // Should resume from sequence 4 (0..3 valid, 4 was partial → truncated)
            assert_eq!(w.current_sequence(), 4);

            // Write a new record
            let seq = w.append(b"recovered-record").unwrap();
            assert_eq!(seq, 4);
            w.flush().unwrap();
        }

        // Verify: 5 records total (0..=4)
        let mut r = WalReader::open(tmp.path()).unwrap();
        let mut count = 0u64;
        let total = r
            .read_from(0, |seq, payload| {
                if seq == 4 {
                    assert_eq!(payload, b"recovered-record");
                }
                count += 1;
                true
            })
            .unwrap();
        assert_eq!(total, 5);
    }

    /// Test 5: Snapshot save/load
    #[test]
    fn test_snapshot_save_load() {
        let tmp = TempDir::new().unwrap();
        let config = test_config(tmp.path());

        let snapshot_data = b"snapshot-state-at-42";

        let mut w = WalWriter::open(config).unwrap();
        w.save_snapshot(42, snapshot_data).unwrap();

        let r = WalReader::open(tmp.path()).unwrap();
        let result = r.load_latest_snapshot().unwrap();
        assert!(result.is_some());
        let (seq, data) = result.unwrap();
        assert_eq!(seq, 42);
        assert_eq!(data, snapshot_data);
    }

    /// Test 6: Snapshot cleanup — only latest 3 kept
    #[test]
    fn test_snapshot_cleanup() {
        let tmp = TempDir::new().unwrap();
        let config = test_config(tmp.path());

        let mut w = WalWriter::open(config).unwrap();
        for i in 0..5u64 {
            w.save_snapshot(i * 10, format!("snap-{i}").as_bytes())
                .unwrap();
        }

        // Count snapshot files
        let snap_count = fs::read_dir(tmp.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().starts_with("snapshot-"))
            .count();

        assert_eq!(snap_count, 3);

        // Latest snapshot should be sequence 40
        let r = WalReader::open(tmp.path()).unwrap();
        let (seq, _) = r.load_latest_snapshot().unwrap().unwrap();
        assert_eq!(seq, 40);
    }

    /// Test 7: Empty WAL — read returns 0 records
    #[test]
    fn test_empty_wal() {
        let tmp = TempDir::new().unwrap();

        let mut r = WalReader::open(tmp.path()).unwrap();
        let count = r.read_from(0, |_seq, _payload| true).unwrap();
        assert_eq!(count, 0);
    }

    /// Test 8: Sequential consistency — 10K records
    #[test]
    fn test_sequential_consistency() {
        let tmp = TempDir::new().unwrap();
        let config = WalConfig {
            dir: tmp.path().to_path_buf(),
            segment_size: 4096, // small to force many rotations
            flush_interval_us: 1000,
            flush_every_n: 10000,
        };

        let num_records = 10_000u64;

        {
            let mut w = WalWriter::open(config).unwrap();
            for i in 0..num_records {
                let seq = w.append(&i.to_le_bytes()).unwrap();
                assert_eq!(seq, i);
            }
            w.flush().unwrap();
        }

        let mut r = WalReader::open(tmp.path()).unwrap();
        let mut expected_seq = 0u64;
        let total = r
            .read_from(0, |seq, payload| {
                assert_eq!(seq, expected_seq);
                let val = u64::from_le_bytes(payload.try_into().unwrap());
                assert_eq!(val, expected_seq);
                expected_seq += 1;
                true
            })
            .unwrap();

        assert_eq!(total, num_records);
        assert_eq!(expected_seq, num_records);
    }

    /// Test: read_from with non-zero start_sequence
    #[test]
    fn test_read_from_offset() {
        let tmp = TempDir::new().unwrap();
        let config = test_config(tmp.path());

        {
            let mut w = WalWriter::open(config).unwrap();
            for i in 0..20u64 {
                w.append(&i.to_le_bytes()).unwrap();
            }
            w.flush().unwrap();
        }

        let mut r = WalReader::open(tmp.path()).unwrap();
        let mut first_seq = None;
        let count = r
            .read_from(10, |seq, _| {
                if first_seq.is_none() {
                    first_seq = Some(seq);
                }
                true
            })
            .unwrap();

        assert_eq!(count, 10);
        assert_eq!(first_seq, Some(10));
    }
}
