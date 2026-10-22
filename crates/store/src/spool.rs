//! Spool: a durable, append-only buffer on disk for data waiting to be sent
//! (a segment log).
//!
//! Records get increasing sequence numbers and are stored one per line as
//! `seq<TAB>json` in segment files named after their first sequence. Appends
//! are fsynced per batch. The receiver confirms up to a sequence with
//! [`Spool::ack`]; fully confirmed segments are deleted. Beyond `max_bytes`
//! the oldest segment is dropped. After a crash, a torn last line is cut off.

use serde::Serialize;
use serde::de::DeserializeOwned;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

const SEGMENT_BYTES: u64 = 16 << 20;
const CURSOR: &str = "acked";

struct Segment {
    path: PathBuf,
    first: u64,
    last: u64,
    bytes: u64,
}

pub struct Spool {
    dir: PathBuf,
    max_bytes: u64,
    segment_bytes: u64,
    segments: Vec<Segment>,
    /// The newest segment, open for appending.
    current: Option<File>,
    next: u64,
    acked: u64,
}

impl Spool {
    pub fn open(dir: &Path, max_bytes: u64) -> io::Result<Self> {
        Self::open_with(dir, max_bytes, SEGMENT_BYTES)
    }

    fn open_with(dir: &Path, max_bytes: u64, segment_bytes: u64) -> io::Result<Self> {
        fs::create_dir_all(dir)?;
        let acked = match fs::read_to_string(dir.join(CURSOR)) {
            Ok(text) => text.trim().parse().unwrap_or(0),
            Err(e) if e.kind() == io::ErrorKind::NotFound => 0,
            Err(e) => return Err(e),
        };

        let mut firsts: Vec<(u64, PathBuf)> = fs::read_dir(dir)?
            .filter_map(|e| e.ok())
            .filter_map(|e| {
                let path = e.path();
                let first = path
                    .file_name()?
                    .to_str()?
                    .strip_suffix(".seg")?
                    .parse()
                    .ok()?;
                Some((first, path))
            })
            .collect();
        firsts.sort();

        let mut segments = Vec::new();
        for (i, (first, path)) in firsts.iter().enumerate() {
            let last = match firsts.get(i + 1) {
                Some((next_first, _)) => next_first - 1,
                // The newest segment: cut a torn line, then find its last record.
                None => repair_tail(path)?.unwrap_or(first - 1),
            };
            let bytes = fs::metadata(path)?.len();
            segments.push(Segment {
                path: path.clone(),
                first: *first,
                last,
                bytes,
            });
        }
        let next = segments
            .last()
            .map_or(acked + 1, |s| s.last + 1)
            .max(acked + 1);
        let current = match segments.last() {
            Some(s) => Some(OpenOptions::new().append(true).open(&s.path)?),
            None => None,
        };

        let mut spool = Spool {
            dir: dir.to_path_buf(),
            max_bytes,
            segment_bytes,
            segments,
            current,
            next,
            acked,
        };
        spool.remove_acked()?;
        Ok(spool)
    }

    /// Records not yet acknowledged.
    pub fn pending(&self) -> u64 {
        self.next - 1 - self.acked
    }

    pub fn bytes(&self) -> u64 {
        self.segments.iter().map(|s| s.bytes).sum()
    }

    /// Appends records with one fsync. Returns how many old records were
    /// dropped to stay within `max_bytes`.
    pub fn append<T: Serialize>(&mut self, records: &[T]) -> io::Result<u64> {
        if records.is_empty() {
            return Ok(0);
        }
        for record in records {
            let json = serde_json::to_string(record).map_err(io::Error::other)?;
            let line = format!("{}\t{json}\n", self.next);
            let roll = self
                .segments
                .last()
                .is_none_or(|s| s.bytes >= self.segment_bytes)
                || self.current.is_none();
            if roll {
                if let Some(file) = self.current.take() {
                    file.sync_data()?;
                }
                let path = self.dir.join(format!("{:020}.seg", self.next));
                self.current = Some(OpenOptions::new().create(true).append(true).open(&path)?);
                self.segments.push(Segment {
                    path,
                    first: self.next,
                    last: self.next - 1,
                    bytes: 0,
                });
            }
            let file = self.current.as_mut().expect("opened above");
            file.write_all(line.as_bytes())?;
            let segment = self.segments.last_mut().expect("pushed above");
            segment.last = self.next;
            segment.bytes += line.len() as u64;
            self.next += 1;
        }
        if let Some(file) = &self.current {
            file.sync_data()?;
        }

        // Over the limit: drop the oldest segments, but never the current one.
        let mut dropped = 0;
        while self.bytes() > self.max_bytes && self.segments.len() > 1 {
            let oldest = self.segments.remove(0);
            dropped += oldest.last.saturating_sub(self.acked.max(oldest.first - 1));
            fs::remove_file(&oldest.path)?;
            if oldest.last > self.acked {
                self.acked = oldest.last;
                self.write_cursor()?;
            }
        }
        Ok(dropped)
    }

    /// Up to `max` unacknowledged records, oldest first.
    pub fn read<T: DeserializeOwned>(&self, max: usize) -> io::Result<Vec<(u64, T)>> {
        let mut out = Vec::new();
        for segment in self.segments.iter().filter(|s| s.last > self.acked) {
            let reader = BufReader::new(File::open(&segment.path)?);
            for line in reader.lines() {
                let line = line?;
                let Some((seq, json)) = line.split_once('\t') else {
                    continue;
                };
                let Ok(seq) = seq.parse::<u64>() else {
                    continue;
                };
                if seq <= self.acked {
                    continue;
                }
                let record = serde_json::from_str(json).map_err(io::Error::other)?;
                out.push((seq, record));
                if out.len() >= max {
                    return Ok(out);
                }
            }
        }
        Ok(out)
    }

    /// Confirms everything up to and including `seq`.
    pub fn ack(&mut self, seq: u64) -> io::Result<()> {
        if seq <= self.acked {
            return Ok(());
        }
        self.acked = seq.min(self.next - 1);
        self.write_cursor()?;
        self.remove_acked()
    }

    fn write_cursor(&self) -> io::Result<()> {
        let tmp = self.dir.join(format!("{CURSOR}.tmp"));
        let mut file = File::create(&tmp)?;
        writeln!(file, "{}", self.acked)?;
        file.sync_data()?;
        fs::rename(tmp, self.dir.join(CURSOR))
    }

    /// Deletes segments that are fully acknowledged.
    fn remove_acked(&mut self) -> io::Result<()> {
        while let Some(first) = self.segments.first() {
            if first.last > self.acked || first.last < first.first {
                break;
            }
            let done = self.segments.remove(0);
            if self.segments.is_empty() {
                self.current = None;
            }
            fs::remove_file(&done.path)?;
        }
        Ok(())
    }
}

/// Cuts a torn last line (crash during a write) and returns the last
/// complete sequence number in the file.
fn repair_tail(path: &Path) -> io::Result<Option<u64>> {
    let data = fs::read(path)?;
    let complete = data.iter().rposition(|&b| b == b'\n').map_or(0, |i| i + 1);
    if complete < data.len() {
        OpenOptions::new()
            .write(true)
            .open(path)?
            .set_len(complete as u64)?;
    }
    let text = String::from_utf8_lossy(&data[..complete]);
    Ok(text
        .lines()
        .next_back()
        .and_then(|l| l.split_once('\t'))
        .and_then(|(seq, _)| seq.parse().ok()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("watcher-spool-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        d
    }

    fn values(spool: &Spool) -> Vec<(u64, String)> {
        spool.read(1000).unwrap()
    }

    #[test]
    fn append_read_ack_across_segments_and_restarts() {
        let d = dir("basic");
        let mut s = Spool::open_with(&d, 1 << 20, 64).unwrap();
        let records: Vec<String> = (1..=10).map(|i| format!("record {i}")).collect();
        s.append(&records).unwrap();
        assert_eq!(s.pending(), 10);
        assert!(s.segments.len() > 1, "small segments roll over");
        assert_eq!(values(&s)[0], (1, "record 1".to_string()));
        assert_eq!(s.read::<String>(3).unwrap().len(), 3);

        s.ack(4).unwrap();
        assert_eq!(s.pending(), 6);
        assert_eq!(values(&s)[0].0, 5);
        drop(s);

        // Reopen: cursor and data survive, sequence numbers continue.
        let mut s = Spool::open_with(&d, 1 << 20, 64).unwrap();
        assert_eq!(s.pending(), 6);
        s.append(&["more".to_string()]).unwrap();
        assert_eq!(values(&s).last().unwrap(), &(11, "more".to_string()));
        s.ack(11).unwrap();
        assert_eq!(s.pending(), 0);
        assert!(values(&s).is_empty());
        let segs = fs::read_dir(&d)
            .unwrap()
            .filter(|e| {
                e.as_ref()
                    .unwrap()
                    .path()
                    .extension()
                    .is_some_and(|x| x == "seg")
            })
            .count();
        assert_eq!(segs, 0, "acknowledged segments are deleted");
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn torn_line_is_cut_after_a_crash() {
        let d = dir("torn");
        let mut s = Spool::open(&d, 1 << 20).unwrap();
        s.append(&["a".to_string(), "b".to_string()]).unwrap();
        let path = s.segments[0].path.clone();
        drop(s);
        // A crash in the middle of writing record 3.
        OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"3\t\"par")
            .unwrap();

        let mut s = Spool::open(&d, 1 << 20).unwrap();
        assert_eq!(s.pending(), 2);
        s.append(&["c".to_string()]).unwrap();
        let got: Vec<_> = values(&s).into_iter().map(|(_, v)| v).collect();
        assert_eq!(got, ["a", "b", "c"]);
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn drops_oldest_segments_beyond_the_limit() {
        let d = dir("full");
        let mut s = Spool::open_with(&d, 200, 64).unwrap();
        let records: Vec<String> = (1..=40).map(|i| format!("record {i:02}")).collect();
        let dropped = s.append(&records).unwrap();
        assert!(dropped > 0);
        assert!(s.bytes() <= 200 + 64);
        let left = values(&s);
        assert_eq!(left.len() as u64 + dropped, 40);
        assert_eq!(left.last().unwrap().1, "record 40");
        fs::remove_dir_all(&d).unwrap();
    }
}
