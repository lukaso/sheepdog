//! The registration wire (PLAN.md §3.2, PHASE2.md D5), shared by sheepr, the fixture and the
//! tests: the `SHEEPR_OUTER` chain and the record an inner run sends.
//!
//! The chain is one entry per line: 32 hex digits (the outer job's nonce), one space, the socket
//! path verbatim. The record is 24 bytes: `SDR1`, the 16 nonce bytes, the sender's own pid
//! (native-endian i32), which is a claim the outer checks against the kernel's answer.

use std::path::PathBuf;

/// The env var that carries the chain of enclosing supervisors.
pub const VAR: &str = "SHEEPR_OUTER";
/// At most this many entries; an inner run that finds this many does not append its own.
pub const MAX: usize = 16;
/// The record's size and its first four bytes.
pub const RECORD: usize = 24;
pub const MAGIC: &[u8; 4] = b"SDR1";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub nonce: [u8; 16],
    pub path: PathBuf,
}

/// The entries of a chain, and how many lines did not parse (skipped).
pub fn parse(s: &str) -> (Vec<Entry>, usize) {
    let mut out = Vec::new();
    let mut bad = 0;
    for l in s.split('\n').filter(|l| !l.is_empty()) {
        match l.split_once(' ').and_then(|(n, p)| Some((unhex16(n)?, p))) {
            Some((nonce, p)) if !p.is_empty() => out.push(Entry { nonce, path: PathBuf::from(p) }),
            _ => bad += 1,
        }
    }
    (out, bad)
}

/// The chain's text for these entries.
pub fn format(entries: &[Entry]) -> String {
    entries.iter().map(|e| format!("{} {}", hex(&e.nonce), e.path.display())).collect::<Vec<_>>().join("\n")
}

/// A path can be an entry only if it has no newline (the separator) and is valid UTF-8.
pub fn path_fits(p: &std::path::Path) -> bool {
    p.to_str().is_some_and(|s| !s.contains('\n') && !s.is_empty())
}

pub fn record(nonce: &[u8; 16], pid: i32) -> [u8; RECORD] {
    let mut r = [0u8; RECORD];
    r[..4].copy_from_slice(MAGIC);
    r[4..20].copy_from_slice(nonce);
    r[20..].copy_from_slice(&pid.to_ne_bytes());
    r
}

/// (nonce, claimed pid) of a well-formed record.
pub fn read_record(r: &[u8]) -> Option<([u8; 16], i32)> {
    if r.len() != RECORD || &r[..4] != MAGIC {
        return None;
    }
    let mut n = [0u8; 16];
    n.copy_from_slice(&r[4..20]);
    Some((n, i32::from_ne_bytes(r[20..].try_into().ok()?)))
}

pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn unhex16(s: &str) -> Option<[u8; 16]> {
    if s.len() != 32 || !s.bytes().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let mut out = [0u8; 16];
    for (i, o) in out.iter_mut().enumerate() {
        *o = u8::from_str_radix(&s[2 * i..2 * i + 2], 16).ok()?;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_chain_round_trips_and_skips_bad_lines() {
        let a = Entry { nonce: [1; 16], path: "/tmp/a b/s".into() };
        let b = Entry { nonce: [0xab; 16], path: "/x".into() };
        let text = format(&[a.clone(), b.clone()]);
        assert_eq!(parse(&text), (vec![a.clone(), b.clone()], 0));
        let messy = format!("zz /p\n{}\n\n{} \nshort\n", format(&[a.clone()]), hex(&[2; 16]));
        assert_eq!(parse(&messy), (vec![a], 3));
    }

    #[test]
    fn a_record_carries_its_nonce_and_claim_and_nothing_else_parses() {
        let r = record(&[7; 16], 4242);
        assert_eq!(read_record(&r), Some(([7; 16], 4242)));
        let mut bad = r;
        bad[0] = b'X';
        assert_eq!(read_record(&bad), None);
        assert_eq!(read_record(&r[..23]), None);
    }
}
