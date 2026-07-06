//! A local, filesystem-backed skill registry.
//!
//! The registry is a plain directory you can inspect, diff, back up, or ship on
//! a USB stick — no database, no server. Layout:
//!
//! ```text
//! <root>/
//!   index.json                     # the [`Index`]: what is installed
//!   skills/
//!     <name>/
//!       <version>/
//!         skill.skillpack          # the archive
//! ```
//!
//! Every `add` **verifies the archive's internal integrity** (per-file SHA-256
//! plus structural checks) before installing it, and records the archive's
//! overall SHA-256 in the index. Every `resolve` re-verifies the on-disk archive
//! hash against the index before handing back a path, so a tampered store is
//! caught.
//!
//! The index is sorted deterministically so the file is stable across runs.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};

use crate::resolve::{Available, Catalog};
use crate::{sha256_hex, Error, Header, Result};

/// Index filename at the registry root.
pub const INDEX_NAME: &str = "index.json";
/// Subdirectory holding installed skill archives.
pub const SKILLS_DIR: &str = "skills";
/// Filename of the stored archive within each version directory.
pub const ARCHIVE_NAME: &str = "skill.skillpack";
/// Index schema version.
pub const INDEX_VERSION: u32 = 1;

/// One installed skill build recorded in the index.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// Skill name.
    pub name: String,
    /// Exact version (semver string).
    pub version: String,
    /// SHA-256 (hex) of the stored archive bytes.
    pub sha256: String,
    /// Number of files inside the archive (from its header).
    pub files: usize,
    /// Archive size in bytes.
    pub size: u64,
    /// Relative path (from the registry root) to the archive.
    pub path: String,
}

/// The registry index (`index.json`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Index {
    /// Index schema version.
    #[serde(default)]
    pub index_version: u32,
    /// Installed entries, sorted by (name, version).
    pub entries: Vec<Entry>,
}

impl Index {
    fn to_json(&self) -> String {
        let mut s = serde_json::to_string_pretty(self).expect("index serializes");
        s.push('\n');
        s
    }
}

/// A local registry rooted at a directory.
pub struct Registry {
    root: PathBuf,
}

impl Registry {
    /// Open (or create) a registry at `root`.
    pub fn open(root: &Path) -> Result<Registry> {
        fs::create_dir_all(root.join(SKILLS_DIR))?;
        let reg = Registry {
            root: root.to_path_buf(),
        };
        if !reg.index_path().exists() {
            reg.write_index(&Index {
                index_version: INDEX_VERSION,
                entries: Vec::new(),
            })?;
        }
        Ok(reg)
    }

    /// The registry root directory.
    pub fn root(&self) -> &Path {
        &self.root
    }

    fn index_path(&self) -> PathBuf {
        self.root.join(INDEX_NAME)
    }

    /// Load the current index.
    pub fn index(&self) -> Result<Index> {
        let raw = fs::read(self.index_path())?;
        serde_json::from_slice(&raw)
            .map_err(|e| Error::Manifest(format!("registry index is invalid: {e}")))
    }

    fn write_index(&self, index: &Index) -> Result<()> {
        fs::write(self.index_path(), index.to_json())?;
        Ok(())
    }

    /// Install an archive (given its bytes) into the registry.
    ///
    /// The archive is verified for internal integrity first; its name/version
    /// come from the archive header. Installing the same name+version again
    /// replaces it (idempotent) but errors if the bytes differ from an existing
    /// entry unless `force` is set.
    pub fn add(&self, archive_bytes: &[u8], force: bool) -> Result<Entry> {
        // Full internal verification before we trust anything.
        let report = crate::verify(archive_bytes)?;
        if !report.ok() {
            return Err(Error::Integrity(format!(
                "refusing to add an archive that fails verification ({} problem(s))",
                report.problems.len()
            )));
        }
        let (header, _) = crate::read_header(archive_bytes)?;
        // The version must be real semver to live in the registry.
        Version::parse(&header.version).map_err(|e| {
            Error::Manifest(format!(
                "archive version '{}' is not valid semver: {e}",
                header.version
            ))
        })?;

        let sha = sha256_hex(archive_bytes);
        let rel = format!(
            "{SKILLS_DIR}/{}/{}/{ARCHIVE_NAME}",
            header.name, header.version
        );
        let dest = self.root.join(&rel);

        let mut index = self.index()?;
        if let Some(existing) = index
            .entries
            .iter()
            .find(|e| e.name == header.name && e.version == header.version)
        {
            if existing.sha256 != sha && !force {
                return Err(Error::Integrity(format!(
                    "{} {} already installed with different content (use force to replace)",
                    header.name, header.version
                )));
            }
        }

        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&dest, archive_bytes)?;

        let entry = Entry {
            name: header.name.clone(),
            version: header.version.clone(),
            sha256: sha,
            files: header.files.len(),
            size: archive_bytes.len() as u64,
            path: rel,
        };

        index
            .entries
            .retain(|e| !(e.name == entry.name && e.version == entry.version));
        index.entries.push(entry.clone());
        Self::sort_entries(&mut index.entries);
        index.index_version = INDEX_VERSION;
        self.write_index(&index)?;
        Ok(entry)
    }

    /// List all installed entries (sorted by name, then version).
    pub fn list(&self) -> Result<Vec<Entry>> {
        Ok(self.index()?.entries)
    }

    /// Resolve `name` + semver `req` to the highest matching installed entry,
    /// re-verifying the on-disk archive hash against the index first.
    pub fn resolve(&self, name: &str, req: &VersionReq) -> Result<(Entry, PathBuf)> {
        let index = self.index()?;
        let chosen = index
            .entries
            .iter()
            .filter(|e| e.name == name)
            .filter_map(|e| Version::parse(&e.version).ok().map(|v| (v, e)))
            .filter(|(v, _)| req.matches(v))
            .max_by(|a, b| a.0.cmp(&b.0))
            .map(|(_, e)| e.clone());

        let entry = chosen
            .ok_or_else(|| Error::Manifest(format!("no installed '{name}' satisfies '{req}'")))?;

        let path = self.root.join(&entry.path);
        let bytes = crate::read_file(&path)?;
        let actual = sha256_hex(&bytes);
        if actual != entry.sha256 {
            return Err(Error::Integrity(format!(
                "stored archive for {} {} was modified on disk (hash mismatch)",
                entry.name, entry.version
            )));
        }
        Ok((entry, path))
    }

    /// Remove a specific name+version. Returns `true` if something was removed.
    pub fn remove(&self, name: &str, version: &str) -> Result<bool> {
        let mut index = self.index()?;
        let before = index.entries.len();
        let removed: Vec<Entry> = index
            .entries
            .iter()
            .filter(|e| e.name == name && e.version == version)
            .cloned()
            .collect();
        index
            .entries
            .retain(|e| !(e.name == name && e.version == version));
        for e in &removed {
            let p = self.root.join(&e.path);
            fs::remove_file(&p).ok();
            // Prune now-empty version and name directories.
            if let Some(vdir) = p.parent() {
                fs::remove_dir(vdir).ok();
                if let Some(ndir) = vdir.parent() {
                    fs::remove_dir(ndir).ok();
                }
            }
        }
        self.write_index(&index)?;
        Ok(index.entries.len() != before)
    }

    /// Build a resolver [`Catalog`] from the current index, so the `resolve`
    /// module can pin dependencies against what is installed.
    pub fn catalog(&self) -> Result<Catalog> {
        let mut cat = Catalog::new();
        for e in self.index()?.entries {
            if let Ok(v) = Version::parse(&e.version) {
                cat.add(&e.name, v, e.sha256);
            }
        }
        Ok(cat)
    }

    /// Read the header of a stored archive (helper for callers/tests).
    pub fn header_of(&self, entry: &Entry) -> Result<Header> {
        let bytes = crate::read_file(&self.root.join(&entry.path))?;
        Ok(crate::read_header(&bytes)?.0)
    }

    fn sort_entries(entries: &mut [Entry]) {
        entries.sort_by(|a, b| {
            a.name.cmp(&b.name).then_with(|| {
                let av = Version::parse(&a.version).ok();
                let bv = Version::parse(&b.version).ok();
                match (av, bv) {
                    (Some(x), Some(y)) => x.cmp(&y),
                    _ => a.version.cmp(&b.version),
                }
            })
        });
    }
}

/// Group a catalog's availabilities by version for display/testing.
pub fn availables_of(cat: &Catalog, name: &str) -> Vec<Available> {
    cat.versions(name).to_vec()
}

/// A convenience map view of installed skills: name -> sorted version strings.
pub fn version_map(index: &Index) -> BTreeMap<String, Vec<String>> {
    let mut m: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for e in &index.entries {
        m.entry(e.name.clone()).or_default().push(e.version.clone());
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let d = std::env::temp_dir().join(format!("asp_reg_{tag}_{nanos}"));
        fs::create_dir_all(&d).unwrap();
        d
    }

    /// Build a minimal skill archive in memory with a given name/version.
    fn make_archive(name: &str, version: &str) -> Vec<u8> {
        let dir = scratch(&format!("mk_{name}_{version}"));
        fs::write(
            dir.join("skill.json"),
            format!(r#"{{"name":"{name}","version":"{version}"}}"#),
        )
        .unwrap();
        fs::write(dir.join("README.md"), b"hi").unwrap();
        let bytes = crate::pack_dir(&dir).unwrap();
        fs::remove_dir_all(&dir).ok();
        bytes
    }

    #[test]
    fn add_list_resolve_roundtrip() {
        let root = scratch("roundtrip");
        let reg = Registry::open(&root).unwrap();

        reg.add(&make_archive("alpha", "1.0.0"), false).unwrap();
        reg.add(&make_archive("alpha", "1.2.0"), false).unwrap();
        reg.add(&make_archive("beta", "0.3.0"), false).unwrap();

        let list = reg.list().unwrap();
        assert_eq!(list.len(), 3);
        // Sorted by name then version.
        assert_eq!(list[0].name, "alpha");
        assert_eq!(list[0].version, "1.0.0");
        assert_eq!(list[1].version, "1.2.0");

        let (entry, path) = reg
            .resolve("alpha", &VersionReq::parse("^1.0").unwrap())
            .unwrap();
        assert_eq!(entry.version, "1.2.0");
        assert!(path.exists());

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn resolve_missing_errors() {
        let root = scratch("missing");
        let reg = Registry::open(&root).unwrap();
        reg.add(&make_archive("alpha", "1.0.0"), false).unwrap();
        assert!(reg
            .resolve("alpha", &VersionReq::parse("^2").unwrap())
            .is_err());
        assert!(reg
            .resolve("ghost", &VersionReq::parse("*").unwrap())
            .is_err());
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn remove_deletes_entry_and_file() {
        let root = scratch("remove");
        let reg = Registry::open(&root).unwrap();
        reg.add(&make_archive("alpha", "1.0.0"), false).unwrap();
        assert!(reg.remove("alpha", "1.0.0").unwrap());
        assert!(reg.list().unwrap().is_empty());
        assert!(!reg.remove("alpha", "1.0.0").unwrap());
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn add_rejects_conflicting_content_without_force() {
        let root = scratch("conflict");
        let reg = Registry::open(&root).unwrap();
        reg.add(&make_archive("alpha", "1.0.0"), false).unwrap();

        // A different archive claiming the same name+version.
        let dir = scratch("alt");
        fs::write(
            dir.join("skill.json"),
            br#"{"name":"alpha","version":"1.0.0"}"#,
        )
        .unwrap();
        fs::write(dir.join("EXTRA.txt"), b"different content").unwrap();
        let other = crate::pack_dir(&dir).unwrap();
        fs::remove_dir_all(&dir).ok();

        assert!(reg.add(&other, false).is_err());
        // With force it replaces.
        assert!(reg.add(&other, true).is_ok());
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn resolve_detects_on_disk_tamper() {
        let root = scratch("tamper");
        let reg = Registry::open(&root).unwrap();
        let entry = reg.add(&make_archive("alpha", "1.0.0"), false).unwrap();

        // Corrupt the stored archive on disk.
        let p = root.join(&entry.path);
        let mut bytes = fs::read(&p).unwrap();
        let n = bytes.len();
        bytes[n - 1] ^= 0xff;
        fs::write(&p, &bytes).unwrap();

        let err = reg
            .resolve("alpha", &VersionReq::parse("*").unwrap())
            .unwrap_err();
        assert!(matches!(err, Error::Integrity(_)));
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn index_persists_and_reopens() {
        let root = scratch("persist");
        {
            let reg = Registry::open(&root).unwrap();
            reg.add(&make_archive("alpha", "1.0.0"), false).unwrap();
        }
        let reg2 = Registry::open(&root).unwrap();
        assert_eq!(reg2.list().unwrap().len(), 1);
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn catalog_reflects_registry() {
        let root = scratch("catalog");
        let reg = Registry::open(&root).unwrap();
        reg.add(&make_archive("alpha", "1.0.0"), false).unwrap();
        reg.add(&make_archive("alpha", "1.4.0"), false).unwrap();
        let cat = reg.catalog().unwrap();
        assert_eq!(availables_of(&cat, "alpha").len(), 2);
        let best = cat
            .best_match("alpha", &VersionReq::parse("^1").unwrap())
            .unwrap();
        assert_eq!(best.version.to_string(), "1.4.0");
        fs::remove_dir_all(&root).ok();
    }
}
