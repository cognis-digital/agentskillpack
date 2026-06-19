//! agentskillpack — a portable container format for AI-agent "skills".
//!
//! A *skill* is a directory of files described by a `skill.json` manifest.
//! This library packs such a directory into a single `.skillpack` archive,
//! unpacks it, verifies its integrity, and reports its metadata.
//!
//! # Container layout (the `.skillpack` format)
//!
//! All multi-byte integers are **big-endian unsigned**. The byte stream is:
//!
//! ```text
//! +--------------------------------------------------------------+
//! | MAGIC        | 8 bytes  | ASCII "SKILLPAK"                    |
//! | FORMAT_VER   | 2 bytes  | u16, currently 1                    |
//! | HEADER_LEN   | 4 bytes  | u32, length in bytes of HEADER_JSON |
//! | HEADER_JSON  | N bytes  | UTF-8 JSON object (see Header)       |
//! +--------------------------------------------------------------+
//! | repeated, once per file, in the order listed in HEADER_JSON: |
//! |   BLOB_LEN   | 8 bytes  | u64, length of this file's bytes    |
//! |   BLOB_DATA  | L bytes  | raw file contents                   |
//! +--------------------------------------------------------------+
//! ```
//!
//! `HEADER_JSON` is a [`Header`]: it carries the skill name/version plus a
//! list of [`FileEntry`] records (relative path, byte size, and SHA-256 hex
//! digest). The file blobs follow the header in the exact order of
//! `Header::files`, so an unpacker can stream them out without seeking.
//!
//! Integrity is checked at two levels: each blob's SHA-256 must match the
//! digest recorded in its [`FileEntry`], and the blob's length must match the
//! recorded size. The format is self-describing and version-tagged.

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Magic bytes at the start of every archive.
pub const MAGIC: &[u8; 8] = b"SKILLPAK";
/// Current container format version.
pub const FORMAT_VERSION: u16 = 1;
/// Conventional manifest filename inside a skill directory.
pub const MANIFEST_NAME: &str = "skill.json";

/// One file recorded in the archive header.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileEntry {
    /// Forward-slash relative path within the skill directory.
    pub path: String,
    /// Size of the file in bytes.
    pub size: u64,
    /// Lowercase hex SHA-256 of the file contents.
    pub sha256: String,
}

/// The JSON header stored near the front of the archive.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Header {
    /// Container format version (mirrors the binary `FORMAT_VER` field).
    pub format_version: u16,
    /// Skill name (from the manifest, falls back to the directory name).
    pub name: String,
    /// Skill version string (from the manifest, falls back to `"0.0.0"`).
    pub version: String,
    /// Optional human description carried over from the manifest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Files contained in the archive, in storage order.
    pub files: Vec<FileEntry>,
}

/// Errors produced by pack / unpack / verify operations.
#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    Json(serde_json::Error),
    /// The byte stream is not a recognizable `.skillpack` archive.
    BadMagic,
    /// The container format version is newer than this build understands.
    UnsupportedVersion(u16),
    /// The archive ended before all declared data was read.
    Truncated(&'static str),
    /// A file's hash or size did not match the header.
    Integrity(String),
    /// A manifest path tried to escape the skill directory.
    UnsafePath(String),
    /// The skill directory or manifest was malformed.
    Manifest(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Io(e) => write!(f, "io error: {e}"),
            Error::Json(e) => write!(f, "json error: {e}"),
            Error::BadMagic => write!(f, "not a skillpack archive (bad magic bytes)"),
            Error::UnsupportedVersion(v) => {
                write!(f, "unsupported skillpack format version {v}")
            }
            Error::Truncated(what) => write!(f, "archive truncated while reading {what}"),
            Error::Integrity(m) => write!(f, "integrity check failed: {m}"),
            Error::UnsafePath(p) => write!(f, "unsafe path in archive: {p}"),
            Error::Manifest(m) => write!(f, "manifest error: {m}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Error::Io(e)
    }
}
impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Error::Json(e)
    }
}

/// Result alias for this crate.
pub type Result<T> = std::result::Result<T, Error>;

/// Minimal view of a skill manifest (`skill.json`). Unknown keys are ignored.
#[derive(Debug, Default, Deserialize)]
struct Manifest {
    name: Option<String>,
    version: Option<String>,
    description: Option<String>,
}

/// Compute the lowercase hex SHA-256 of a byte slice.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut out = String::with_capacity(64);
    for b in digest {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

/// Normalize a relative path to forward slashes and reject anything that
/// escapes the skill root (absolute paths, `..`, drive prefixes, etc.).
fn safe_relative(path: &Path) -> Result<String> {
    let mut parts = Vec::new();
    for comp in path.components() {
        match comp {
            Component::Normal(c) => {
                let s = c
                    .to_str()
                    .ok_or_else(|| Error::UnsafePath(path.display().to_string()))?;
                parts.push(s.to_string());
            }
            Component::CurDir => {}
            _ => return Err(Error::UnsafePath(path.display().to_string())),
        }
    }
    if parts.is_empty() {
        return Err(Error::UnsafePath(path.display().to_string()));
    }
    Ok(parts.join("/"))
}

/// Recursively collect every file under `root`, returning (relative-path, abs-path)
/// pairs sorted by relative path for deterministic archive ordering.
fn collect_files(root: &Path) -> Result<Vec<(String, PathBuf)>> {
    let mut found: BTreeMap<String, PathBuf> = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            let abs = entry.path();
            let ft = entry.file_type()?;
            if ft.is_dir() {
                stack.push(abs);
            } else if ft.is_file() {
                let rel = abs
                    .strip_prefix(root)
                    .map_err(|_| Error::UnsafePath(abs.display().to_string()))?;
                let rel = safe_relative(rel)?;
                found.insert(rel, abs);
            }
            // symlinks and other node types are skipped intentionally.
        }
    }
    Ok(found.into_iter().collect())
}

/// Pack the skill directory at `skill_dir` into a `.skillpack` byte vector.
///
/// The directory must contain a `skill.json` manifest (used for name/version);
/// if fields are missing they fall back to the directory name and `"0.0.0"`.
pub fn pack_dir(skill_dir: &Path) -> Result<Vec<u8>> {
    if !skill_dir.is_dir() {
        return Err(Error::Manifest(format!(
            "{} is not a directory",
            skill_dir.display()
        )));
    }

    let files = collect_files(skill_dir)?;
    if files.is_empty() {
        return Err(Error::Manifest("skill directory contains no files".into()));
    }

    // Read the manifest if present, for metadata. It is also packed as a file.
    let manifest_path = skill_dir.join(MANIFEST_NAME);
    let manifest: Manifest = if manifest_path.is_file() {
        let raw = fs::read(&manifest_path)?;
        serde_json::from_slice(&raw)
            .map_err(|e| Error::Manifest(format!("{MANIFEST_NAME} is not valid JSON: {e}")))?
    } else {
        Manifest::default()
    };

    let default_name = skill_dir
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("skill")
        .to_string();

    // Read each file, compute entries, and hold the bytes for serialization.
    let mut entries = Vec::with_capacity(files.len());
    let mut blobs: Vec<Vec<u8>> = Vec::with_capacity(files.len());
    for (rel, abs) in &files {
        let bytes = fs::read(abs)?;
        entries.push(FileEntry {
            path: rel.clone(),
            size: bytes.len() as u64,
            sha256: sha256_hex(&bytes),
        });
        blobs.push(bytes);
    }

    let header = Header {
        format_version: FORMAT_VERSION,
        name: manifest.name.unwrap_or(default_name),
        version: manifest.version.unwrap_or_else(|| "0.0.0".into()),
        description: manifest.description,
        files: entries,
    };

    serialize(&header, &blobs)
}

/// Serialize a header plus its ordered blobs into the container byte stream.
fn serialize(header: &Header, blobs: &[Vec<u8>]) -> Result<Vec<u8>> {
    let header_json = serde_json::to_vec(header)?;
    let mut out = Vec::new();
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&FORMAT_VERSION.to_be_bytes());
    out.extend_from_slice(&(header_json.len() as u32).to_be_bytes());
    out.extend_from_slice(&header_json);
    for blob in blobs {
        out.extend_from_slice(&(blob.len() as u64).to_be_bytes());
        out.extend_from_slice(blob);
    }
    Ok(out)
}

/// A fully decoded archive: its header and each file's raw bytes (in order).
pub struct Archive {
    pub header: Header,
    pub blobs: Vec<Vec<u8>>,
}

/// Read just the header from an archive byte stream, returning it and the
/// offset at which the file blobs begin.
pub fn read_header(data: &[u8]) -> Result<(Header, usize)> {
    let mut pos = 0usize;

    let magic = take(data, &mut pos, 8).ok_or(Error::Truncated("magic"))?;
    if magic != MAGIC.as_slice() {
        return Err(Error::BadMagic);
    }

    let ver_bytes = take(data, &mut pos, 2).ok_or(Error::Truncated("version"))?;
    let version = u16::from_be_bytes([ver_bytes[0], ver_bytes[1]]);
    if version != FORMAT_VERSION {
        return Err(Error::UnsupportedVersion(version));
    }

    let len_bytes = take(data, &mut pos, 4).ok_or(Error::Truncated("header length"))?;
    let header_len =
        u32::from_be_bytes([len_bytes[0], len_bytes[1], len_bytes[2], len_bytes[3]]) as usize;

    let header_json = take(data, &mut pos, header_len).ok_or(Error::Truncated("header"))?;
    let header: Header = serde_json::from_slice(header_json)?;

    if header.format_version != FORMAT_VERSION {
        return Err(Error::UnsupportedVersion(header.format_version));
    }

    Ok((header, pos))
}

/// Decode an entire archive (header + all file blobs) from bytes.
pub fn read_archive(data: &[u8]) -> Result<Archive> {
    let (header, mut pos) = read_header(data)?;
    let mut blobs = Vec::with_capacity(header.files.len());
    for entry in &header.files {
        let len_bytes = take(data, &mut pos, 8).ok_or(Error::Truncated("blob length"))?;
        let blob_len = u64::from_be_bytes([
            len_bytes[0],
            len_bytes[1],
            len_bytes[2],
            len_bytes[3],
            len_bytes[4],
            len_bytes[5],
            len_bytes[6],
            len_bytes[7],
        ]) as usize;
        let blob = take(data, &mut pos, blob_len).ok_or(Error::Truncated("blob data"))?;
        // Defensive: the declared size in the header must match what we read.
        if blob.len() as u64 != entry.size {
            return Err(Error::Integrity(format!(
                "{}: blob length {} != header size {}",
                entry.path,
                blob.len(),
                entry.size
            )));
        }
        blobs.push(blob.to_vec());
    }
    if pos != data.len() {
        return Err(Error::Integrity(format!(
            "{} trailing byte(s) after last blob",
            data.len() - pos
        )));
    }
    Ok(Archive { header, blobs })
}

/// Advance `pos` by `n`, returning the slice consumed, or `None` if the
/// buffer is too short.
fn take<'a>(data: &'a [u8], pos: &mut usize, n: usize) -> Option<&'a [u8]> {
    let end = pos.checked_add(n)?;
    if end > data.len() {
        return None;
    }
    let slice = &data[*pos..end];
    *pos = end;
    Some(slice)
}

/// Result of verifying an archive.
#[derive(Debug)]
pub struct VerifyReport {
    pub files_checked: usize,
    pub problems: Vec<String>,
}

impl VerifyReport {
    pub fn ok(&self) -> bool {
        self.problems.is_empty()
    }
}

/// Verify that every blob in `data` matches the size and SHA-256 recorded in
/// the header, and that the container is structurally sound.
pub fn verify(data: &[u8]) -> Result<VerifyReport> {
    let archive = read_archive(data)?;
    let mut problems = Vec::new();
    for (entry, blob) in archive.header.files.iter().zip(archive.blobs.iter()) {
        if blob.len() as u64 != entry.size {
            problems.push(format!(
                "{}: size {} != recorded {}",
                entry.path,
                blob.len(),
                entry.size
            ));
        }
        let actual = sha256_hex(blob);
        if actual != entry.sha256 {
            problems.push(format!(
                "{}: sha256 {} != recorded {}",
                entry.path, actual, entry.sha256
            ));
        }
    }
    Ok(VerifyReport {
        files_checked: archive.header.files.len(),
        problems,
    })
}

/// Unpack an archive's bytes into `out_dir`, recreating the directory tree.
/// Hashes are verified as files are written; a mismatch aborts with an error.
pub fn unpack_to(data: &[u8], out_dir: &Path) -> Result<usize> {
    let archive = read_archive(data)?;
    fs::create_dir_all(out_dir)?;
    let mut written = 0usize;
    for (entry, blob) in archive.header.files.iter().zip(archive.blobs.iter()) {
        let actual = sha256_hex(blob);
        if actual != entry.sha256 {
            return Err(Error::Integrity(format!(
                "{}: sha256 mismatch on unpack",
                entry.path
            )));
        }
        // Re-validate the path from the (untrusted) header before writing.
        let rel = safe_relative(Path::new(&entry.path))?;
        let dest = out_dir.join(&rel);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut f = fs::File::create(&dest)?;
        f.write_all(blob)?;
        written += 1;
    }
    Ok(written)
}

/// Convenience: read an archive file from disk into memory.
pub fn read_file(path: &Path) -> Result<Vec<u8>> {
    let mut f = fs::File::open(path)?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf)?;
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str, bytes: &[u8]) -> FileEntry {
        FileEntry {
            path: path.into(),
            size: bytes.len() as u64,
            sha256: sha256_hex(bytes),
        }
    }

    #[test]
    fn sha256_known_vector() {
        // SHA-256 of the empty string.
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        // SHA-256 of "abc".
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn safe_relative_rejects_traversal() {
        assert!(safe_relative(Path::new("../escape")).is_err());
        assert!(safe_relative(Path::new("a/../../b")).is_err());
        assert_eq!(safe_relative(Path::new("a/b/c.txt")).unwrap(), "a/b/c.txt");
        assert_eq!(safe_relative(Path::new("./a/b")).unwrap(), "a/b");
    }

    #[test]
    fn serialize_then_read_roundtrips_header() {
        let blobs = vec![b"hello".to_vec(), b"world!!".to_vec()];
        let header = Header {
            format_version: FORMAT_VERSION,
            name: "demo".into(),
            version: "1.2.3".into(),
            description: Some("a demo".into()),
            files: vec![entry("a.txt", &blobs[0]), entry("b/c.txt", &blobs[1])],
        };
        let bytes = serialize(&header, &blobs).unwrap();
        let archive = read_archive(&bytes).unwrap();
        assert_eq!(archive.header, header);
        assert_eq!(archive.blobs, blobs);
    }

    #[test]
    fn verify_detects_tampered_blob() {
        let blobs = vec![b"original".to_vec()];
        let header = Header {
            format_version: FORMAT_VERSION,
            name: "t".into(),
            version: "0.0.0".into(),
            description: None,
            files: vec![entry("f.txt", &blobs[0])],
        };
        let mut bytes = serialize(&header, &blobs).unwrap();
        // Verify passes as-is.
        assert!(verify(&bytes).unwrap().ok());
        // Flip a byte inside the trailing blob (last byte of the stream).
        let last = bytes.len() - 1;
        bytes[last] ^= 0xff;
        let report = verify(&bytes).unwrap();
        assert!(!report.ok(), "tamper should be detected");
        assert_eq!(report.files_checked, 1);
    }

    #[test]
    fn bad_magic_is_rejected() {
        let err = read_header(b"NOTASKILLPACK....").unwrap_err();
        assert!(matches!(err, Error::BadMagic));
    }
}
