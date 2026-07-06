//! Command-line interface for agentskillpack.
//!
//! Subcommands:
//!   Container:  `pack`, `unpack`, `verify`, `info`
//!   Manifest:   `manifest validate`, `schema`
//!   Signing:    `keygen`, `sign`, `verify --pubkey`
//!   Registry:   `registry add|list|resolve|remove`
//!   Resolve:    `lock`
//!
//! Run with no arguments or `--help` for usage. Exit code 0 on success;
//! non-zero on any error or failed integrity/signature check (so `verify` works
//! as a CI gate). `--json` is honored where a machine-readable form is useful.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use agentskillpack as asp;
use asp::manifest::Manifest;
use asp::registry::Registry;
use asp::resolve::{resolve, Catalog};
use asp::sign::{self, KeyPair, SignatureFile};
use semver::VersionReq;

const USAGE: &str = "\
agentskillpack — pack, sign, verify, resolve and register AI-agent skills

USAGE:
    agentskillpack <COMMAND> [OPTIONS]

CONTAINER COMMANDS:
    pack    <skill-dir> -o <out.skillpack> [--validate]   Pack a skill directory
    unpack  <archive>   -o <dir>                          Unpack an archive
    verify  <archive> [--pubkey <key>] [--sig <file>]     Integrity (+signature) gate
    info    <archive> [--json]                            Print archive metadata

MANIFEST COMMANDS:
    manifest validate <dir|skill.json> [--json]           Validate a manifest
    schema                                                Print the skill JSON Schema

SIGNING COMMANDS:
    keygen  -o <dir> [--name <n>]                         Generate an ed25519 keypair
    sign    <archive> --key <priv.key> [-o <sig>]         Detached-sign an archive

REGISTRY COMMANDS:
    registry add     <archive> --registry <dir>           Install an archive
    registry list    --registry <dir> [--json]            List installed skills
    registry resolve <name> --req <semver> --registry <dir>  Resolve name -> path
    registry remove  <name> --version <v> --registry <dir>   Uninstall a version

RESOLVE COMMANDS:
    lock    <dir|skill.json> --registry <dir> [-o <lock>] Write skillpack.lock

    help                                                  Show this message

EXAMPLES:
    agentskillpack pack examples/hello-skill -o hello.skillpack --validate
    agentskillpack keygen -o keys --name author
    agentskillpack sign hello.skillpack --key keys/author.key
    agentskillpack verify hello.skillpack --pubkey keys/author.pub
    agentskillpack registry add hello.skillpack --registry ./reg
    agentskillpack registry resolve hello-skill --req '^1.0' --registry ./reg
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: &[String]) -> asp::Result<ExitCode> {
    let cmd = match args.first() {
        Some(c) => c.as_str(),
        None => {
            print!("{USAGE}");
            return Ok(ExitCode::SUCCESS);
        }
    };
    match cmd {
        "help" | "-h" | "--help" => {
            print!("{USAGE}");
            Ok(ExitCode::SUCCESS)
        }
        "pack" => cmd_pack(&args[1..]),
        "unpack" => cmd_unpack(&args[1..]),
        "verify" => cmd_verify(&args[1..]),
        "info" => cmd_info(&args[1..]),
        "manifest" => cmd_manifest(&args[1..]),
        "schema" => cmd_schema(&args[1..]),
        "keygen" => cmd_keygen(&args[1..]),
        "sign" => cmd_sign(&args[1..]),
        "registry" => cmd_registry(&args[1..]),
        "lock" => cmd_lock(&args[1..]),
        other => {
            eprintln!("error: unknown command '{other}'\n");
            print!("{USAGE}");
            Ok(ExitCode::FAILURE)
        }
    }
}

// ---------------------------------------------------------------------------
// Tiny argument helpers (kept dependency-free and explicit).
// ---------------------------------------------------------------------------

/// Parsed flags: named `--flag value` options, boolean switches, positionals.
struct Args {
    named: std::collections::BTreeMap<String, String>,
    switches: std::collections::BTreeSet<String>,
    positional: Vec<String>,
}

/// Parse args where `named_keys` take a following value, `switch_keys` are
/// standalone booleans, and everything else is positional. `-o` aliases
/// `--output`, `--key`/`--pubkey`/`--sig`/`--registry`/`--req`/`--version`/`--name`
/// are recognized value-flags by the callers that pass them.
fn parse_args(args: &[String], value_flags: &[&str], switch_flags: &[&str]) -> asp::Result<Args> {
    let mut named = std::collections::BTreeMap::new();
    let mut switches = std::collections::BTreeSet::new();
    let mut positional = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        // Normalize -o to --output for lookup.
        let key = if a == "-o" { "--output" } else { a };
        if value_flags.contains(&key) {
            let v = args
                .get(i + 1)
                .ok_or_else(|| asp::Error::Manifest(format!("{a} requires a value")))?;
            named.insert(key.trim_start_matches("--").to_string(), v.clone());
            i += 2;
        } else if switch_flags.contains(&key) {
            switches.insert(key.trim_start_matches("--").to_string());
            i += 1;
        } else if a.starts_with("--") {
            return Err(asp::Error::Manifest(format!("unknown flag {a}")));
        } else {
            positional.push(a.to_string());
            i += 1;
        }
    }
    Ok(Args {
        named,
        switches,
        positional,
    })
}

impl Args {
    fn get(&self, key: &str) -> Option<&str> {
        self.named.get(key).map(|s| s.as_str())
    }
    fn require(&self, key: &str, what: &str) -> asp::Result<&str> {
        self.get(key)
            .ok_or_else(|| asp::Error::Manifest(format!("missing {what} (--{key})")))
    }
    fn has(&self, key: &str) -> bool {
        self.switches.contains(key)
    }
    fn pos(&self, idx: usize, what: &str) -> asp::Result<&str> {
        self.positional
            .get(idx)
            .map(|s| s.as_str())
            .ok_or_else(|| asp::Error::Manifest(format!("missing {what}")))
    }
}

// ---------------------------------------------------------------------------
// Container commands.
// ---------------------------------------------------------------------------

fn cmd_pack(args: &[String]) -> asp::Result<ExitCode> {
    let a = parse_args(args, &["--output"], &["--validate"])?;
    let dir = a.pos(0, "<skill-dir>")?;
    let out = a.require("output", "-o <out.skillpack>")?;

    let bytes = if a.has("validate") {
        asp::pack_dir_validated(Path::new(dir))?
    } else {
        asp::pack_dir(Path::new(dir))?
    };
    std::fs::write(out, &bytes)?;

    let (header, _) = asp::read_header(&bytes)?;
    println!(
        "packed '{}' v{} — {} file(s), {} bytes -> {}",
        header.name,
        header.version,
        header.files.len(),
        bytes.len(),
        out
    );
    Ok(ExitCode::SUCCESS)
}

fn cmd_unpack(args: &[String]) -> asp::Result<ExitCode> {
    let a = parse_args(args, &["--output"], &[])?;
    let archive = a.pos(0, "<archive>")?;
    let out = a.require("output", "-o <dir>")?;
    let data = asp::read_file(Path::new(archive))?;
    let n = asp::unpack_to(&data, Path::new(out))?;
    println!("unpacked {n} file(s) into {out}");
    Ok(ExitCode::SUCCESS)
}

fn cmd_verify(args: &[String]) -> asp::Result<ExitCode> {
    let a = parse_args(args, &["--pubkey", "--sig"], &[])?;
    let archive = a.pos(0, "<archive>")?;
    let data = asp::read_file(Path::new(archive))?;

    // 1) Internal integrity (always).
    let report = asp::verify(&data)?;
    if !report.ok() {
        eprintln!(
            "FAILED: {} problem(s) across {} file(s):",
            report.problems.len(),
            report.files_checked
        );
        for p in &report.problems {
            eprintln!("  - {p}");
        }
        return Ok(ExitCode::FAILURE);
    }

    // 2) Optional signature verification.
    if let Some(pubkey_path) = a.get("pubkey") {
        let pubkey = std::fs::read_to_string(pubkey_path)?;
        let sig_path = a
            .get("sig")
            .map(PathBuf::from)
            .unwrap_or_else(|| sign::sig_path_for(Path::new(archive)));
        let sig_raw = asp::read_file(&sig_path)?;
        let sig = SignatureFile::from_slice(&sig_raw)?;
        match sig.verify(&data, Some(pubkey.trim())) {
            Ok(()) => {
                println!(
                    "OK: {} file(s) verified; signature valid ({})",
                    report.files_checked,
                    &sig.public_key[..16.min(sig.public_key.len())]
                );
                return Ok(ExitCode::SUCCESS);
            }
            Err(e) => {
                eprintln!("FAILED: {e}");
                return Ok(ExitCode::FAILURE);
            }
        }
    }

    println!("OK: {} file(s) verified", report.files_checked);
    Ok(ExitCode::SUCCESS)
}

fn cmd_info(args: &[String]) -> asp::Result<ExitCode> {
    let a = parse_args(args, &[], &["--json"])?;
    let archive = a.pos(0, "<archive>")?;
    let data = asp::read_file(Path::new(archive))?;
    let (header, _) = asp::read_header(&data)?;

    if a.has("json") {
        println!("{}", serde_json::to_string_pretty(&header)?);
    } else {
        println!("name:           {}", header.name);
        println!("version:        {}", header.version);
        if let Some(d) = &header.description {
            println!("description:    {d}");
        }
        println!("format_version: {}", header.format_version);
        println!("files:          {}", header.files.len());
        for f in &header.files {
            println!("  {}  {:>10}  {}", &f.sha256[..16], f.size, f.path);
        }
    }
    Ok(ExitCode::SUCCESS)
}

// ---------------------------------------------------------------------------
// Manifest commands.
// ---------------------------------------------------------------------------

fn cmd_manifest(args: &[String]) -> asp::Result<ExitCode> {
    let sub = args
        .first()
        .map(|s| s.as_str())
        .ok_or_else(|| asp::Error::Manifest("manifest requires a subcommand (validate)".into()))?;
    match sub {
        "validate" => cmd_manifest_validate(&args[1..]),
        other => Err(asp::Error::Manifest(format!(
            "unknown manifest subcommand '{other}'"
        ))),
    }
}

fn cmd_manifest_validate(args: &[String]) -> asp::Result<ExitCode> {
    let a = parse_args(args, &[], &["--json"])?;
    let target = a.pos(0, "<dir|skill.json>")?;
    let manifest = Manifest::load(Path::new(target))?;
    match manifest.validate() {
        Ok(()) => {
            if a.has("json") {
                println!(
                    "{}",
                    serde_json::json!({"valid": true, "name": manifest.name, "version": manifest.version})
                );
            } else {
                println!(
                    "OK: '{}' v{} is a valid manifest ({} capability, {} dependency)",
                    manifest.name,
                    manifest.version,
                    manifest.capabilities.len(),
                    manifest.dependencies.len()
                );
            }
            Ok(ExitCode::SUCCESS)
        }
        Err(issues) => {
            if a.has("json") {
                let arr: Vec<_> = issues
                    .iter()
                    .map(|i| serde_json::json!({"field": i.field, "message": i.message}))
                    .collect();
                println!("{}", serde_json::json!({"valid": false, "issues": arr}));
            } else {
                eprintln!("INVALID: {} problem(s):", issues.len());
                for i in &issues {
                    eprintln!("  - {i}");
                }
            }
            Ok(ExitCode::FAILURE)
        }
    }
}

fn cmd_schema(_args: &[String]) -> asp::Result<ExitCode> {
    println!("{}", asp::manifest::json_schema());
    Ok(ExitCode::SUCCESS)
}

// ---------------------------------------------------------------------------
// Signing commands.
// ---------------------------------------------------------------------------

fn cmd_keygen(args: &[String]) -> asp::Result<ExitCode> {
    let a = parse_args(args, &["--output", "--name"], &[])?;
    let dir = a.require("output", "-o <dir>")?;
    let name = a.get("name").unwrap_or("skillpack");
    let kp = KeyPair::generate();
    let (priv_path, pub_path) = sign::write_keypair(&kp, Path::new(dir), name)?;
    println!("generated ed25519 keypair:");
    println!("  private: {}", priv_path.display());
    println!("  public:  {}", pub_path.display());
    println!("  pubkey:  {}", kp.public_hex());
    println!("keep the private key secret; distribute the public key to verifiers.");
    Ok(ExitCode::SUCCESS)
}

fn cmd_sign(args: &[String]) -> asp::Result<ExitCode> {
    let a = parse_args(args, &["--key", "--output"], &[])?;
    let archive = a.pos(0, "<archive>")?;
    let key_path = a.require("key", "--key <priv.key>")?;
    let data = asp::read_file(Path::new(archive))?;
    // Refuse to sign something that isn't even a valid archive.
    let report = asp::verify(&data)?;
    if !report.ok() {
        return Err(asp::Error::Integrity(
            "refusing to sign an archive that fails verification".into(),
        ));
    }
    let key_hex = std::fs::read_to_string(key_path)?;
    let kp = KeyPair::from_private_hex(&key_hex)?;
    let sig = kp.sign_archive(&data);
    let out = a
        .get("output")
        .map(PathBuf::from)
        .unwrap_or_else(|| sign::sig_path_for(Path::new(archive)));
    std::fs::write(&out, sig.to_json())?;
    println!(
        "signed {} -> {} (signer {})",
        archive,
        out.display(),
        &kp.public_hex()[..16]
    );
    Ok(ExitCode::SUCCESS)
}

// ---------------------------------------------------------------------------
// Registry commands.
// ---------------------------------------------------------------------------

fn cmd_registry(args: &[String]) -> asp::Result<ExitCode> {
    let sub = args.first().map(|s| s.as_str()).ok_or_else(|| {
        asp::Error::Manifest("registry requires a subcommand (add|list|resolve|remove)".into())
    })?;
    match sub {
        "add" => cmd_registry_add(&args[1..]),
        "list" => cmd_registry_list(&args[1..]),
        "resolve" => cmd_registry_resolve(&args[1..]),
        "remove" => cmd_registry_remove(&args[1..]),
        other => Err(asp::Error::Manifest(format!(
            "unknown registry subcommand '{other}'"
        ))),
    }
}

fn cmd_registry_add(args: &[String]) -> asp::Result<ExitCode> {
    let a = parse_args(args, &["--registry"], &["--force"])?;
    let archive = a.pos(0, "<archive>")?;
    let reg_dir = a.require("registry", "--registry <dir>")?;
    let data = asp::read_file(Path::new(archive))?;
    let reg = Registry::open(Path::new(reg_dir))?;
    let entry = reg.add(&data, a.has("force"))?;
    println!(
        "added {} v{} ({} file(s), {} bytes) to {}",
        entry.name, entry.version, entry.files, entry.size, reg_dir
    );
    Ok(ExitCode::SUCCESS)
}

fn cmd_registry_list(args: &[String]) -> asp::Result<ExitCode> {
    let a = parse_args(args, &["--registry"], &["--json"])?;
    let reg_dir = a.require("registry", "--registry <dir>")?;
    let reg = Registry::open(Path::new(reg_dir))?;
    let list = reg.list()?;
    if a.has("json") {
        println!("{}", serde_json::to_string_pretty(&list)?);
    } else if list.is_empty() {
        println!("(registry is empty)");
    } else {
        for e in &list {
            println!(
                "{:<24} {:<12} {}  {} file(s)",
                e.name,
                e.version,
                &e.sha256[..16],
                e.files
            );
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn cmd_registry_resolve(args: &[String]) -> asp::Result<ExitCode> {
    let a = parse_args(args, &["--registry", "--req"], &["--json"])?;
    let name = a.pos(0, "<name>")?;
    let reg_dir = a.require("registry", "--registry <dir>")?;
    let req_str = a.get("req").unwrap_or("*");
    let req = VersionReq::parse(req_str)
        .map_err(|e| asp::Error::Manifest(format!("bad --req '{req_str}': {e}")))?;
    let reg = Registry::open(Path::new(reg_dir))?;
    let (entry, path) = reg.resolve(name, &req)?;
    if a.has("json") {
        println!(
            "{}",
            serde_json::json!({"name": entry.name, "version": entry.version, "sha256": entry.sha256, "path": path.display().to_string()})
        );
    } else {
        println!("{} v{} -> {}", entry.name, entry.version, path.display());
    }
    Ok(ExitCode::SUCCESS)
}

fn cmd_registry_remove(args: &[String]) -> asp::Result<ExitCode> {
    let a = parse_args(args, &["--registry", "--version"], &[])?;
    let name = a.pos(0, "<name>")?;
    let reg_dir = a.require("registry", "--registry <dir>")?;
    let version = a.require("version", "--version <v>")?;
    let reg = Registry::open(Path::new(reg_dir))?;
    if reg.remove(name, version)? {
        println!("removed {name} v{version}");
        Ok(ExitCode::SUCCESS)
    } else {
        eprintln!("no such entry: {name} v{version}");
        Ok(ExitCode::FAILURE)
    }
}

// ---------------------------------------------------------------------------
// Resolve / lock command.
// ---------------------------------------------------------------------------

fn cmd_lock(args: &[String]) -> asp::Result<ExitCode> {
    let a = parse_args(args, &["--registry", "--output"], &[])?;
    let target = a.pos(0, "<dir|skill.json>")?;
    let manifest = Manifest::load(Path::new(target))?;

    // Catalog comes from the registry if given, else an empty catalog (which
    // resolves only the no-dependency case).
    let catalog = match a.get("registry") {
        Some(dir) => Registry::open(Path::new(dir))?.catalog()?,
        None => Catalog::new(),
    };

    match resolve(&manifest, &catalog) {
        Ok(lock) => {
            let out = a
                .get("output")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(asp::resolve::LOCKFILE_NAME));
            std::fs::write(&out, lock.to_json())?;
            println!(
                "locked {} dependency/ies for '{}' v{} -> {}",
                lock.pins.len(),
                lock.root,
                lock.root_version,
                out.display()
            );
            Ok(ExitCode::SUCCESS)
        }
        Err(conflicts) => {
            eprintln!("FAILED to resolve {} dependency/ies:", conflicts.len());
            for c in &conflicts {
                eprintln!("  - {c}");
            }
            Ok(ExitCode::FAILURE)
        }
    }
}
