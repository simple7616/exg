//! WAL event dumper library. Reads rkyv-encoded `exg_protocol::Event` records
//! from a WAL directory and writes them as one JSON line per record,
//! prefixed with the sequence number and a tab.
//!
//! Stage 0 §5.3 — verification tool for the demo and integration tests.

use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result};
use exg_protocol::Event;
use exg_wal::WalReader;

/// Dump events from `wal_dir` starting at `from_seq`, writing one JSON line
/// per event to `out`.
///
/// Returns an error on CRC failure, malformed rkyv payload, or IO.
pub fn dump(wal_dir: &Path, from_seq: u64, out: &mut dyn Write) -> Result<()> {
    if !wal_dir.exists() {
        return Ok(());
    }
    let mut reader = WalReader::open(wal_dir)
        .with_context(|| format!("opening WAL dir {}", wal_dir.display()))?;

    let mut write_err: Option<std::io::Error> = None;
    let result = reader.read_from(from_seq, |seq, payload| {
        if write_err.is_some() {
            return false;
        }
        // Copy into an owned Vec so rkyv gets a guaranteed-aligned buffer.
        // The mmap payload slice starts at header offset 12, which may not
        // satisfy rkyv's 16-byte alignment requirement.
        let owned: Vec<u8> = payload.to_vec();
        let evt = match rkyv::from_bytes::<Event, rkyv::rancor::Error>(&owned) {
            Ok(e) => e,
            Err(e) => {
                write_err = Some(std::io::Error::other(format!(
                    "rkyv decode at seq {seq}: {e}"
                )));
                return false;
            }
        };
        let json = match serde_json::to_string(&evt) {
            Ok(s) => s,
            Err(e) => {
                write_err = Some(std::io::Error::other(format!(
                    "json encode at seq {seq}: {e}"
                )));
                return false;
            }
        };
        if let Err(e) = writeln!(out, "{seq}\t{json}") {
            write_err = Some(e);
            return false;
        }
        true
    });

    if let Some(e) = write_err {
        return Err(anyhow::Error::new(e));
    }
    result.with_context(|| format!("reading WAL at {}", wal_dir.display()))?;
    Ok(())
}
