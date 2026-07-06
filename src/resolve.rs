//! Dependency resolution and version pinning.
//!
//! A skill declares dependencies as `(name, semver-requirement)` pairs
//! ([`crate::manifest::Dependency`]). Given a *catalog* of which versions of
//! each skill are available (and the sha256 of each one's archive), the
//! resolver picks, for every requirement, the **highest** available version
//! that satisfies it. The result is a deterministic [`Lockfile`] that pins each
//! dependency to an exact version and archive hash.
//!
//! The algorithm is intentionally simple and total: it is a per-requirement
//! "highest compatible" selection (like a flat, non-transitive `npm`/`cargo`
//! minimal-work resolve), not a full SAT solver. It never mutates inputs and
//! never touches the network. Conflicts (a requirement with no satisfying
//! version) are reported, not papered over.

use std::collections::BTreeMap;

use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};

use crate::manifest::{Dependency, Manifest};
use crate::{Error, Result};

/// Filename of the emitted lockfile.
pub const LOCKFILE_NAME: &str = "skillpack.lock";
/// Lockfile schema version.
pub const LOCK_VERSION: u32 = 1;

/// One available build of a skill in the resolution catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Available {
    /// Exact version of this build.
    pub version: Version,
    /// SHA-256 (hex) of the archive for this build.
    pub sha256: String,
}

/// A catalog mapping skill name -> the versions available for it.
///
/// This is what a [`crate::registry::Registry`] can produce, but the resolver
/// takes the plain map so it stays testable and offline.
#[derive(Debug, Default, Clone)]
pub struct Catalog {
    inner: BTreeMap<String, Vec<Available>>,
}

impl Catalog {
    /// A new, empty catalog.
    pub fn new() -> Self {
        Catalog::default()
    }

    /// Record that `name` version `version` (with the given archive hash) is
    /// available.
    pub fn add(&mut self, name: &str, version: Version, sha256: impl Into<String>) {
        self.inner
            .entry(name.to_string())
            .or_default()
            .push(Available {
                version,
                sha256: sha256.into(),
            });
    }

    /// All available builds of `name`, or an empty slice.
    pub fn versions(&self, name: &str) -> &[Available] {
        self.inner.get(name).map(|v| v.as_slice()).unwrap_or(&[])
    }

    /// Select the highest available version of `name` satisfying `req`.
    pub fn best_match(&self, name: &str, req: &VersionReq) -> Option<&Available> {
        self.versions(name)
            .iter()
            .filter(|a| req.matches(&a.version))
            .max_by(|a, b| a.version.cmp(&b.version))
    }
}

/// One pinned dependency in a lockfile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pin {
    /// Dependency skill name.
    pub name: String,
    /// The original requirement string that was resolved.
    pub req: String,
    /// The exact version chosen.
    pub version: String,
    /// SHA-256 (hex) of the resolved archive.
    pub sha256: String,
}

/// A resolved, pinned set of dependencies.
///
/// Serialized to `skillpack.lock` as JSON. Pins are sorted by name for
/// deterministic output (byte-identical across runs given identical inputs).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lockfile {
    /// Lockfile schema version.
    pub lock_version: u32,
    /// The name of the root skill whose dependencies were resolved.
    pub root: String,
    /// The root skill's own version.
    pub root_version: String,
    /// Pinned dependencies, sorted by name.
    pub pins: Vec<Pin>,
}

impl Lockfile {
    /// Serialize to deterministic pretty JSON (with trailing newline).
    pub fn to_json(&self) -> String {
        let mut s = serde_json::to_string_pretty(self).expect("lockfile serializes");
        s.push('\n');
        s
    }

    /// Parse a lockfile from JSON bytes.
    pub fn from_slice(raw: &[u8]) -> Result<Lockfile> {
        serde_json::from_slice(raw)
            .map_err(|e| Error::Manifest(format!("{LOCKFILE_NAME} is not valid: {e}")))
    }
}

/// A resolution conflict: a requirement that nothing in the catalog satisfies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    /// The unsatisfiable dependency.
    pub dependency: Dependency,
    /// Human explanation (missing skill vs. no matching version).
    pub reason: String,
}

impl std::fmt::Display for Conflict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} {} — {}",
            self.dependency.name, self.dependency.req, self.reason
        )
    }
}

/// Resolve the direct dependencies of `manifest` against `catalog`.
///
/// Returns a pinned [`Lockfile`] on success, or the list of [`Conflict`]s if
/// any requirement cannot be satisfied. Deterministic: pins are sorted by name.
pub fn resolve(
    manifest: &Manifest,
    catalog: &Catalog,
) -> std::result::Result<Lockfile, Vec<Conflict>> {
    let mut pins = Vec::new();
    let mut conflicts = Vec::new();

    for dep in &manifest.dependencies {
        let req = match VersionReq::parse(&dep.req) {
            Ok(r) => r,
            Err(e) => {
                conflicts.push(Conflict {
                    dependency: dep.clone(),
                    reason: format!("invalid requirement: {e}"),
                });
                continue;
            }
        };
        match catalog.best_match(&dep.name, &req) {
            Some(chosen) => pins.push(Pin {
                name: dep.name.clone(),
                req: dep.req.clone(),
                version: chosen.version.to_string(),
                sha256: chosen.sha256.clone(),
            }),
            None => {
                let reason = if catalog.versions(&dep.name).is_empty() {
                    "no such skill in catalog".to_string()
                } else {
                    let have: Vec<String> = catalog
                        .versions(&dep.name)
                        .iter()
                        .map(|a| a.version.to_string())
                        .collect();
                    format!("no version satisfies (available: {})", have.join(", "))
                };
                conflicts.push(Conflict {
                    dependency: dep.clone(),
                    reason,
                });
            }
        }
    }

    if !conflicts.is_empty() {
        return Err(conflicts);
    }

    pins.sort_by(|a, b| a.name.cmp(&b.name));

    Ok(Lockfile {
        lock_version: LOCK_VERSION,
        root: manifest.name.clone(),
        root_version: manifest.version.clone(),
        pins,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap()
    }

    fn manifest_with(deps: &[(&str, &str)]) -> Manifest {
        let raw = serde_json::json!({
            "name": "root",
            "version": "1.0.0",
            "dependencies": deps.iter().map(|(n, r)| {
                serde_json::json!({"name": n, "req": r})
            }).collect::<Vec<_>>(),
        });
        Manifest::from_slice(raw.to_string().as_bytes()).unwrap()
    }

    fn sample_catalog() -> Catalog {
        let mut c = Catalog::new();
        c.add("alpha", v("1.0.0"), "a100");
        c.add("alpha", v("1.2.0"), "a120");
        c.add("alpha", v("1.5.3"), "a153");
        c.add("alpha", v("2.0.0"), "a200");
        c.add("beta", v("0.9.0"), "b090");
        c
    }

    #[test]
    fn picks_highest_compatible() {
        let cat = sample_catalog();
        let best = cat
            .best_match("alpha", &VersionReq::parse("^1.0").unwrap())
            .unwrap();
        assert_eq!(best.version, v("1.5.3"));
        assert_eq!(best.sha256, "a153");
    }

    #[test]
    fn range_requirement_excludes_major_bump() {
        let cat = sample_catalog();
        let best = cat
            .best_match("alpha", &VersionReq::parse(">=1.0, <2.0").unwrap())
            .unwrap();
        assert_eq!(best.version, v("1.5.3"));
    }

    #[test]
    fn resolve_produces_sorted_pins() {
        let cat = sample_catalog();
        let m = manifest_with(&[("beta", "^0.9"), ("alpha", "^1")]);
        let lock = resolve(&m, &cat).unwrap();
        assert_eq!(lock.pins.len(), 2);
        assert_eq!(lock.pins[0].name, "alpha");
        assert_eq!(lock.pins[0].version, "1.5.3");
        assert_eq!(lock.pins[1].name, "beta");
        assert_eq!(lock.pins[1].version, "0.9.0");
    }

    #[test]
    fn resolve_is_deterministic() {
        let cat = sample_catalog();
        let m = manifest_with(&[("alpha", "^1"), ("beta", "^0.9")]);
        let a = resolve(&m, &cat).unwrap().to_json();
        let b = resolve(&m, &cat).unwrap().to_json();
        assert_eq!(a, b);
    }

    #[test]
    fn lockfile_roundtrips() {
        let cat = sample_catalog();
        let m = manifest_with(&[("alpha", "^1")]);
        let lock = resolve(&m, &cat).unwrap();
        let parsed = Lockfile::from_slice(lock.to_json().as_bytes()).unwrap();
        assert_eq!(lock, parsed);
    }

    #[test]
    fn missing_skill_is_a_conflict() {
        let cat = sample_catalog();
        let m = manifest_with(&[("ghost", "^1")]);
        let conflicts = resolve(&m, &cat).unwrap_err();
        assert_eq!(conflicts.len(), 1);
        assert!(conflicts[0].reason.contains("no such skill"));
    }

    #[test]
    fn no_matching_version_is_a_conflict() {
        let cat = sample_catalog();
        let m = manifest_with(&[("beta", "^2.0")]);
        let conflicts = resolve(&m, &cat).unwrap_err();
        assert_eq!(conflicts.len(), 1);
        assert!(conflicts[0].reason.contains("no version satisfies"));
        assert!(conflicts[0].reason.contains("0.9.0"));
    }

    #[test]
    fn no_dependencies_resolves_empty() {
        let cat = sample_catalog();
        let m = manifest_with(&[]);
        let lock = resolve(&m, &cat).unwrap();
        assert!(lock.pins.is_empty());
        assert_eq!(lock.root, "root");
    }
}
