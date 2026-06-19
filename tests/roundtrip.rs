//! End-to-end tests: pack a real directory, round-trip it through unpack,
//! verify integrity, detect tampering, and exercise info/header reads.

use std::fs;
use std::path::{Path, PathBuf};

use agentskillpack as asp;

/// Create a unique scratch directory under the OS temp dir.
fn scratch(tag: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("asp_test_{tag}_{nanos}"));
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// Build a small skill directory with a manifest, nested files and binary data.
fn make_skill(root: &Path) {
    fs::create_dir_all(root.join("scripts")).unwrap();
    fs::write(
        root.join("skill.json"),
        br#"{"name":"sample-skill","version":"2.5.0","description":"test skill"}"#,
    )
    .unwrap();
    fs::write(root.join("README.md"), b"# Sample\nHello.\n").unwrap();
    fs::write(root.join("scripts/run.sh"), b"#!/bin/sh\necho hi\n").unwrap();
    // A binary-ish file with all byte values to prove lossless storage.
    let bin: Vec<u8> = (0u16..=255).map(|b| b as u8).collect();
    fs::write(root.join("scripts/data.bin"), &bin).unwrap();
}

/// Read every file under a dir into a sorted (relpath, bytes) list.
fn snapshot(root: &Path) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in fs::read_dir(&d).unwrap() {
            let e = e.unwrap();
            let p = e.path();
            if e.file_type().unwrap().is_dir() {
                stack.push(p);
            } else {
                let rel = p
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                out.push((rel, fs::read(&p).unwrap()));
            }
        }
    }
    out.sort();
    out
}

#[test]
fn pack_unpack_roundtrip_is_lossless() {
    let base = scratch("roundtrip");
    let src = base.join("src");
    let dst = base.join("dst");
    make_skill(&src);

    let bytes = asp::pack_dir(&src).unwrap();
    let written = asp::unpack_to(&bytes, &dst).unwrap();
    assert_eq!(written, 4, "should write all four files");

    assert_eq!(
        snapshot(&src),
        snapshot(&dst),
        "unpacked tree must equal source tree byte-for-byte"
    );

    fs::remove_dir_all(&base).ok();
}

#[test]
fn verify_passes_on_clean_archive() {
    let base = scratch("verify_ok");
    let src = base.join("src");
    make_skill(&src);

    let bytes = asp::pack_dir(&src).unwrap();
    let report = asp::verify(&bytes).unwrap();
    assert!(report.ok());
    assert_eq!(report.files_checked, 4);

    fs::remove_dir_all(&base).ok();
}

#[test]
fn verify_fails_on_tamper() {
    let base = scratch("verify_tamper");
    let src = base.join("src");
    make_skill(&src);

    let mut bytes = asp::pack_dir(&src).unwrap();
    // Corrupt a byte in the blob region (well past the header).
    let target = bytes.len() - 5;
    bytes[target] ^= 0x01;

    let report = asp::verify(&bytes).unwrap();
    assert!(!report.ok(), "tampered archive must not verify");
    assert!(!report.problems.is_empty());

    fs::remove_dir_all(&base).ok();
}

#[test]
fn info_header_reports_metadata() {
    let base = scratch("info");
    let src = base.join("src");
    make_skill(&src);

    let bytes = asp::pack_dir(&src).unwrap();
    let (header, offset) = asp::read_header(&bytes).unwrap();

    assert_eq!(header.name, "sample-skill");
    assert_eq!(header.version, "2.5.0");
    assert_eq!(header.description.as_deref(), Some("test skill"));
    assert_eq!(header.format_version, asp::FORMAT_VERSION);
    assert_eq!(header.files.len(), 4);
    assert!(offset > 0 && offset < bytes.len());

    // Every entry carries a full-length hex digest.
    for f in &header.files {
        assert_eq!(f.sha256.len(), 64);
        assert!(f.sha256.chars().all(|c| c.is_ascii_hexdigit()));
    }

    fs::remove_dir_all(&base).ok();
}

#[test]
fn truncated_archive_is_rejected() {
    let base = scratch("trunc");
    let src = base.join("src");
    make_skill(&src);

    let bytes = asp::pack_dir(&src).unwrap();
    let chopped = &bytes[..bytes.len() - 10];
    assert!(asp::read_archive(chopped).is_err());

    fs::remove_dir_all(&base).ok();
}
