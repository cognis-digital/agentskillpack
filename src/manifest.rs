//! Typed, versioned model for a skill manifest (`skill.json`).
//!
//! The base container ([`crate::Header`]) only needs a name, version, and file
//! list. This module adds a *rich* manifest: a strongly-typed description of
//! what a skill is, what it needs, and what it depends on, so a host can make
//! informed allow/deny decisions **before** it ever runs a line of skill code.
//!
//! The model here is the source of truth for the JSON Schema shipped at
//! `docs/skill.schema.json`; [`json_schema`] regenerates that document, and the
//! test suite asserts the two stay in sync and that every bundled example
//! manifest validates against the rules in [`Manifest::validate`].
//!
//! # Why a typed manifest
//!
//! Shipping agent skills as loose files (or unsigned zips) gives a host no way
//! to answer three questions up front:
//!
//! 1. *What version is this, and is it compatible with my runtime?* — [`version`]
//!    is a real semver, and [`Compat::engine`] declares the runtime range.
//! 2. *What will it try to touch?* — [`Manifest::capabilities`] is an explicit,
//!    typed list ([`Capability`]) of the powers the skill requests (filesystem,
//!    network, subprocess, environment). Nothing here grants anything; it lets a
//!    policy engine decide.
//! 3. *What else does it need?* — [`Manifest::dependencies`] names other skills
//!    by semver requirement, which the resolver (`crate::resolve`) can pin.
//!
//! [`version`]: Manifest::version

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};

use crate::{Error, Result, MANIFEST_NAME};

/// Schema version of the manifest model itself (distinct from the *skill's*
/// version and from the container `FORMAT_VERSION`). Bumped only on a
/// breaking change to the manifest shape.
pub const MANIFEST_SCHEMA_VERSION: u32 = 1;

/// Canonical `$id` for the generated JSON Schema.
pub const SCHEMA_ID: &str = "https://cognis.digital/schemas/skill.schema.json";

/// A declared capability: a power the skill asks the host to grant.
///
/// A capability is *declarative*. Listing `Net` does not open a socket; it tells
/// a host "this skill intends to use the network" so the host can allow, deny,
/// or prompt. Absent capabilities are an assertion the skill will not use them,
/// which a sandbox can enforce.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum CapabilityKind {
    /// Read files from the host filesystem.
    #[serde(rename = "fs.read")]
    FsRead,
    /// Write or create files on the host filesystem.
    #[serde(rename = "fs.write")]
    FsWrite,
    /// Open network connections (inbound or outbound).
    #[serde(rename = "net")]
    Net,
    /// Spawn subprocesses / execute other programs.
    #[serde(rename = "exec")]
    Exec,
    /// Read host environment variables.
    #[serde(rename = "env")]
    Env,
}

impl CapabilityKind {
    /// The stable wire string for this capability (matches the `serde` rename).
    pub fn as_str(self) -> &'static str {
        match self {
            CapabilityKind::FsRead => "fs.read",
            CapabilityKind::FsWrite => "fs.write",
            CapabilityKind::Net => "net",
            CapabilityKind::Exec => "exec",
            CapabilityKind::Env => "env",
        }
    }

    /// Every capability kind, for schema generation and validation.
    pub fn all() -> [CapabilityKind; 5] {
        [
            CapabilityKind::FsRead,
            CapabilityKind::FsWrite,
            CapabilityKind::Net,
            CapabilityKind::Exec,
            CapabilityKind::Env,
        ]
    }
}

/// A capability plus an optional list of scope strings that narrow it.
///
/// For example `fs.read` scoped to `["./data", "./config"]`, or `net` scoped to
/// `["api.example.com:443"]`. Scopes are advisory hints to the host policy; an
/// empty scope list means "unscoped / broad".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capability {
    /// Which power is requested.
    pub kind: CapabilityKind,
    /// Optional scope strings that narrow the request (paths, hosts, var names).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scope: Vec<String>,
}

impl Capability {
    /// Construct an unscoped capability.
    pub fn new(kind: CapabilityKind) -> Self {
        Capability {
            kind,
            scope: Vec::new(),
        }
    }
}

/// A dependency on another skill, by name and semver requirement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Dependency {
    /// The depended-on skill's name.
    pub name: String,
    /// A semver requirement string, e.g. `^1.2`, `>=1.0, <2.0`.
    pub req: String,
}

impl Dependency {
    /// Parse the requirement into a [`semver::VersionReq`], surfacing a clear
    /// error tied to this dependency's name.
    pub fn parsed_req(&self) -> Result<VersionReq> {
        VersionReq::parse(&self.req).map_err(|e| {
            Error::Manifest(format!(
                "dependency '{}' has invalid version requirement '{}': {e}",
                self.name, self.req
            ))
        })
    }
}

/// Compatibility / engine constraints for the skill's runtime.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Compat {
    /// Optional semver requirement on the agent runtime/engine, e.g. `>=0.4`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine: Option<String>,
    /// Optional human label for the engine family, e.g. `"cognis-agent"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine_name: Option<String>,
}

/// An optional named input or output declaration (a lightweight schema hint).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IoField {
    /// Field name.
    pub name: String,
    /// A type label — free-form but conventionally one of
    /// `string|number|boolean|object|array|file`.
    #[serde(rename = "type")]
    pub ty: String,
    /// Optional human description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Whether this field is required (defaults to `false`).
    #[serde(default)]
    pub required: bool,
}

/// The full, typed skill manifest.
///
/// Only `name` and `version` are strictly required; everything else is optional
/// but validated when present. Unknown JSON keys are rejected (`deny_unknown_fields`)
/// so typos surface loudly rather than being silently ignored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    /// Manifest schema version. Defaults to [`MANIFEST_SCHEMA_VERSION`].
    #[serde(default = "default_schema_version")]
    pub manifest_version: u32,
    /// Skill name. Must be a non-empty, lowercase-friendly identifier.
    pub name: String,
    /// Skill version — must parse as semver.
    pub version: String,
    /// Human-readable description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// SPDX-ish license label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    /// Authors (name and/or `name <email>` strings).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub authors: Vec<String>,
    /// Relative path (within the skill) to the entrypoint file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entrypoint: Option<String>,
    /// Free-form tags for discovery.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// Declared capabilities this skill requests from the host.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<Capability>,
    /// Other skills this skill depends on, by semver requirement.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dependencies: Vec<Dependency>,
    /// Optional declared inputs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<IoField>,
    /// Optional declared outputs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outputs: Vec<IoField>,
    /// Optional runtime/engine compatibility constraints.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compat: Option<Compat>,
}

fn default_schema_version() -> u32 {
    MANIFEST_SCHEMA_VERSION
}

/// A single validation problem, with the offending field path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationIssue {
    /// Dotted field path, e.g. `dependencies[1].req`.
    pub field: String,
    /// Human-readable message.
    pub message: String,
}

impl std::fmt::Display for ValidationIssue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.field, self.message)
    }
}

impl Manifest {
    /// Parse a manifest from raw JSON bytes. This enforces the *shape*
    /// (unknown-field rejection, required fields); call [`validate`](Self::validate)
    /// for the semantic rules.
    pub fn from_slice(raw: &[u8]) -> Result<Manifest> {
        serde_json::from_slice(raw)
            .map_err(|e| Error::Manifest(format!("{MANIFEST_NAME} is not a valid manifest: {e}")))
    }

    /// Load and parse a `skill.json` from a directory or a direct file path.
    ///
    /// If `path` is a directory, `skill.json` inside it is read.
    pub fn load(path: &Path) -> Result<Manifest> {
        let file = if path.is_dir() {
            path.join(MANIFEST_NAME)
        } else {
            path.to_path_buf()
        };
        if !file.is_file() {
            return Err(Error::Manifest(format!(
                "no manifest found at {}",
                file.display()
            )));
        }
        let raw = fs::read(&file)?;
        Manifest::from_slice(&raw)
    }

    /// Parse the skill's own [`semver::Version`].
    pub fn semver(&self) -> Result<Version> {
        Version::parse(&self.version).map_err(|e| {
            Error::Manifest(format!(
                "version '{}' is not valid semver: {e}",
                self.version
            ))
        })
    }

    /// Run every semantic validation rule, collecting *all* problems rather
    /// than stopping at the first. Returns `Ok(())` only when the list is empty.
    ///
    /// Rules:
    /// - `name` non-empty, `[a-z0-9._-]+`, no leading/trailing separators.
    /// - `version` valid semver.
    /// - `manifest_version` known.
    /// - every dependency `req` a valid semver requirement, no self-dependency,
    ///   no duplicate dependency names.
    /// - every capability scope string non-empty; no duplicate capability kinds.
    /// - `compat.engine` (if present) a valid semver requirement.
    /// - `entrypoint` (if present) a safe relative path.
    /// - io field `type` from the known set.
    pub fn validate(&self) -> std::result::Result<(), Vec<ValidationIssue>> {
        let mut issues = Vec::new();

        if self.manifest_version == 0 || self.manifest_version > MANIFEST_SCHEMA_VERSION {
            issues.push(ValidationIssue {
                field: "manifest_version".into(),
                message: format!(
                    "unsupported manifest_version {} (this build supports 1..={})",
                    self.manifest_version, MANIFEST_SCHEMA_VERSION
                ),
            });
        }

        validate_name(&self.name, &mut issues);

        if let Err(e) = Version::parse(&self.version) {
            issues.push(ValidationIssue {
                field: "version".into(),
                message: format!("'{}' is not valid semver: {e}", self.version),
            });
        }

        // Dependencies.
        let mut dep_names: BTreeSet<&str> = BTreeSet::new();
        for (i, dep) in self.dependencies.iter().enumerate() {
            if dep.name.trim().is_empty() {
                issues.push(ValidationIssue {
                    field: format!("dependencies[{i}].name"),
                    message: "dependency name must not be empty".into(),
                });
            }
            if dep.name == self.name {
                issues.push(ValidationIssue {
                    field: format!("dependencies[{i}].name"),
                    message: "a skill may not depend on itself".into(),
                });
            }
            if !dep_names.insert(dep.name.as_str()) {
                issues.push(ValidationIssue {
                    field: format!("dependencies[{i}].name"),
                    message: format!("duplicate dependency '{}'", dep.name),
                });
            }
            if let Err(e) = VersionReq::parse(&dep.req) {
                issues.push(ValidationIssue {
                    field: format!("dependencies[{i}].req"),
                    message: format!("'{}' is not a valid version requirement: {e}", dep.req),
                });
            }
        }

        // Capabilities.
        let mut seen_caps: BTreeSet<CapabilityKind> = BTreeSet::new();
        for (i, cap) in self.capabilities.iter().enumerate() {
            if !seen_caps.insert(cap.kind) {
                issues.push(ValidationIssue {
                    field: format!("capabilities[{i}].kind"),
                    message: format!("duplicate capability '{}'", cap.kind.as_str()),
                });
            }
            for (j, s) in cap.scope.iter().enumerate() {
                if s.trim().is_empty() {
                    issues.push(ValidationIssue {
                        field: format!("capabilities[{i}].scope[{j}]"),
                        message: "scope entry must not be empty".into(),
                    });
                }
            }
        }

        // Compat engine requirement.
        if let Some(compat) = &self.compat {
            if let Some(engine) = &compat.engine {
                if let Err(e) = VersionReq::parse(engine) {
                    issues.push(ValidationIssue {
                        field: "compat.engine".into(),
                        message: format!("'{engine}' is not a valid version requirement: {e}"),
                    });
                }
            }
        }

        // Entrypoint path safety (reuse the container's normalizer).
        if let Some(ep) = &self.entrypoint {
            if crate::safe_relative(Path::new(ep)).is_err() {
                issues.push(ValidationIssue {
                    field: "entrypoint".into(),
                    message: format!("'{ep}' is not a safe relative path"),
                });
            }
        }

        // I/O field types.
        const IO_TYPES: &[&str] = &["string", "number", "boolean", "object", "array", "file"];
        for (label, fields) in [("inputs", &self.inputs), ("outputs", &self.outputs)] {
            for (i, field) in fields.iter().enumerate() {
                if field.name.trim().is_empty() {
                    issues.push(ValidationIssue {
                        field: format!("{label}[{i}].name"),
                        message: "field name must not be empty".into(),
                    });
                }
                if !IO_TYPES.contains(&field.ty.as_str()) {
                    issues.push(ValidationIssue {
                        field: format!("{label}[{i}].type"),
                        message: format!(
                            "'{}' is not a known type ({})",
                            field.ty,
                            IO_TYPES.join("|")
                        ),
                    });
                }
            }
        }

        if issues.is_empty() {
            Ok(())
        } else {
            Err(issues)
        }
    }

    /// Convenience: parse, then validate, returning a combined error suitable
    /// for CLI display.
    pub fn parse_and_validate(raw: &[u8]) -> Result<Manifest> {
        let m = Manifest::from_slice(raw)?;
        if let Err(issues) = m.validate() {
            let joined = issues
                .iter()
                .map(|i| format!("  - {i}"))
                .collect::<Vec<_>>()
                .join("\n");
            return Err(Error::Manifest(format!(
                "{} manifest validation problem(s):\n{joined}",
                issues.len()
            )));
        }
        Ok(m)
    }
}

/// Validate a skill/dependency name into the shared issue list.
fn validate_name(name: &str, issues: &mut Vec<ValidationIssue>) {
    if name.is_empty() {
        issues.push(ValidationIssue {
            field: "name".into(),
            message: "name must not be empty".into(),
        });
        return;
    }
    let ok = name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-'));
    if !ok {
        issues.push(ValidationIssue {
            field: "name".into(),
            message: format!("'{name}' must match [a-z0-9._-]+"),
        });
    }
    if name.starts_with(['.', '_', '-']) || name.ends_with(['.', '_', '-']) {
        issues.push(ValidationIssue {
            field: "name".into(),
            message: "name must not start or end with a separator".into(),
        });
    }
}

/// Generate the JSON Schema (draft 2020-12) document for [`Manifest`] as a
/// pretty-printed string. This is the single source of truth for
/// `docs/skill.schema.json`; a test asserts the file matches this output.
pub fn json_schema() -> String {
    let cap_enum: Vec<serde_json::Value> = CapabilityKind::all()
        .iter()
        .map(|k| serde_json::Value::String(k.as_str().into()))
        .collect();

    let schema = serde_json::json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "$id": SCHEMA_ID,
        "title": "Agent Skill Manifest",
        "description": "Typed manifest (skill.json) for an AI-agent skill packaged with agentskillpack.",
        "type": "object",
        "additionalProperties": false,
        "required": ["name", "version"],
        "properties": {
            "manifest_version": {
                "type": "integer",
                "minimum": 1,
                "maximum": MANIFEST_SCHEMA_VERSION,
                "description": "Manifest schema version.",
                "default": MANIFEST_SCHEMA_VERSION
            },
            "name": {
                "type": "string",
                "pattern": "^[a-z0-9]([a-z0-9._-]*[a-z0-9])?$",
                "description": "Skill name; lowercase identifier."
            },
            "version": {
                "type": "string",
                "description": "Semantic version (semver 2.0.0)."
            },
            "description": { "type": "string" },
            "license": { "type": "string" },
            "authors": { "type": "array", "items": { "type": "string" } },
            "entrypoint": {
                "type": "string",
                "description": "Relative path to the skill entrypoint."
            },
            "tags": { "type": "array", "items": { "type": "string" } },
            "capabilities": {
                "type": "array",
                "description": "Declared powers the skill requests from the host.",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["kind"],
                    "properties": {
                        "kind": { "type": "string", "enum": cap_enum },
                        "scope": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Optional scope strings narrowing the capability."
                        }
                    }
                }
            },
            "dependencies": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["name", "req"],
                    "properties": {
                        "name": { "type": "string" },
                        "req": {
                            "type": "string",
                            "description": "Semver requirement, e.g. ^1.2 or >=1.0, <2.0."
                        }
                    }
                }
            },
            "inputs": { "$ref": "#/$defs/ioFieldArray" },
            "outputs": { "$ref": "#/$defs/ioFieldArray" },
            "compat": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "engine": {
                        "type": "string",
                        "description": "Semver requirement on the agent runtime."
                    },
                    "engine_name": { "type": "string" }
                }
            }
        },
        "$defs": {
            "ioFieldArray": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["name", "type"],
                    "properties": {
                        "name": { "type": "string" },
                        "type": {
                            "type": "string",
                            "enum": ["string", "number", "boolean", "object", "array", "file"]
                        },
                        "description": { "type": "string" },
                        "required": { "type": "boolean", "default": false }
                    }
                }
            }
        }
    });

    serde_json::to_string_pretty(&schema).expect("schema serializes")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal() -> Manifest {
        Manifest {
            manifest_version: 1,
            name: "demo-skill".into(),
            version: "1.2.3".into(),
            description: None,
            license: None,
            authors: vec![],
            entrypoint: None,
            tags: vec![],
            capabilities: vec![],
            dependencies: vec![],
            inputs: vec![],
            outputs: vec![],
            compat: None,
        }
    }

    #[test]
    fn minimal_manifest_validates() {
        assert!(minimal().validate().is_ok());
    }

    #[test]
    fn parses_full_manifest_from_json() {
        let raw = br#"{
            "name": "researcher",
            "version": "2.0.1",
            "description": "web research skill",
            "license": "COCL-1.0",
            "authors": ["Cognis Digital"],
            "entrypoint": "scripts/run.py",
            "tags": ["research"],
            "capabilities": [
                {"kind": "net", "scope": ["api.example.com:443"]},
                {"kind": "fs.write", "scope": ["./out"]}
            ],
            "dependencies": [{"name": "hello-skill", "req": "^1.0"}],
            "inputs": [{"name": "query", "type": "string", "required": true}],
            "outputs": [{"name": "report", "type": "file"}],
            "compat": {"engine": ">=0.4, <1.0", "engine_name": "cognis-agent"}
        }"#;
        let m = Manifest::parse_and_validate(raw).unwrap();
        assert_eq!(m.name, "researcher");
        assert_eq!(m.capabilities.len(), 2);
        assert_eq!(m.capabilities[0].kind, CapabilityKind::Net);
        assert_eq!(m.dependencies[0].req, "^1.0");
    }

    #[test]
    fn rejects_unknown_field() {
        let raw = br#"{"name":"x","version":"1.0.0","bogus":true}"#;
        assert!(Manifest::from_slice(raw).is_err());
    }

    #[test]
    fn rejects_bad_semver_version() {
        let mut m = minimal();
        m.version = "1.two.3".into();
        let issues = m.validate().unwrap_err();
        assert!(issues.iter().any(|i| i.field == "version"));
    }

    #[test]
    fn rejects_bad_name() {
        let mut m = minimal();
        m.name = "Bad Name!".into();
        assert!(m.validate().is_err());
        m.name = "-leading".into();
        assert!(m.validate().is_err());
    }

    #[test]
    fn rejects_self_dependency_and_duplicates() {
        let mut m = minimal();
        m.dependencies = vec![
            Dependency {
                name: "demo-skill".into(),
                req: "^1".into(),
            },
            Dependency {
                name: "other".into(),
                req: "^1".into(),
            },
            Dependency {
                name: "other".into(),
                req: "^2".into(),
            },
        ];
        let issues = m.validate().unwrap_err();
        assert!(issues.iter().any(|i| i.message.contains("itself")));
        assert!(issues.iter().any(|i| i.message.contains("duplicate")));
    }

    #[test]
    fn rejects_bad_dependency_req() {
        let mut m = minimal();
        m.dependencies = vec![Dependency {
            name: "x".into(),
            req: "not-a-req!!".into(),
        }];
        let issues = m.validate().unwrap_err();
        assert!(issues.iter().any(|i| i.field == "dependencies[0].req"));
    }

    #[test]
    fn rejects_duplicate_capability_and_empty_scope() {
        let mut m = minimal();
        m.capabilities = vec![
            Capability {
                kind: CapabilityKind::Net,
                scope: vec!["".into()],
            },
            Capability::new(CapabilityKind::Net),
        ];
        let issues = m.validate().unwrap_err();
        assert!(issues.iter().any(|i| i.message.contains("duplicate")));
        assert!(issues.iter().any(|i| i.field.contains("scope")));
    }

    #[test]
    fn rejects_bad_engine_req_and_unsafe_entrypoint() {
        let mut m = minimal();
        m.compat = Some(Compat {
            engine: Some("garbage!!".into()),
            engine_name: None,
        });
        m.entrypoint = Some("../escape.py".into());
        let issues = m.validate().unwrap_err();
        assert!(issues.iter().any(|i| i.field == "compat.engine"));
        assert!(issues.iter().any(|i| i.field == "entrypoint"));
    }

    #[test]
    fn rejects_unknown_io_type() {
        let mut m = minimal();
        m.inputs = vec![IoField {
            name: "q".into(),
            ty: "widget".into(),
            description: None,
            required: false,
        }];
        let issues = m.validate().unwrap_err();
        assert!(issues.iter().any(|i| i.field == "inputs[0].type"));
    }

    #[test]
    fn capability_kind_wire_strings_roundtrip() {
        for k in CapabilityKind::all() {
            let json = serde_json::to_string(&k).unwrap();
            let back: CapabilityKind = serde_json::from_str(&json).unwrap();
            assert_eq!(k, back);
            assert_eq!(json, format!("\"{}\"", k.as_str()));
        }
    }

    #[test]
    fn schema_is_valid_json_with_expected_shape() {
        let schema: serde_json::Value = serde_json::from_str(&json_schema()).unwrap();
        assert_eq!(schema["$id"], SCHEMA_ID);
        assert_eq!(schema["required"][0], "name");
        assert!(schema["properties"]["capabilities"].is_object());
    }
}
