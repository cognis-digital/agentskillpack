//! Command-line interface for agentskillpack.
//!
//! Subcommands: `pack`, `unpack`, `verify`, `info`. Run with no arguments or
//! `--help` for usage. Exit code 0 on success; non-zero on any error or on a
//! failed integrity check (so `verify` works as a CI gate).

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use agentskillpack as asp;

const USAGE: &str = "\
agentskillpack — pack, unpack, verify and inspect AI-agent skill archives

USAGE:
    agentskillpack <COMMAND> [OPTIONS]

COMMANDS:
    pack    <skill-dir> -o <out.skillpack>   Pack a skill directory into an archive
    unpack  <archive>   -o <dir>             Unpack an archive into a directory
    verify  <archive>                        Check archive integrity (CI gate)
    info    <archive> [--json]               Print archive metadata
    help                                     Show this message

EXAMPLES:
    agentskillpack pack examples/hello-skill -o hello.skillpack
    agentskillpack verify hello.skillpack
    agentskillpack info hello.skillpack --json
    agentskillpack unpack hello.skillpack -o out/
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
        other => {
            eprintln!("error: unknown command '{other}'\n");
            print!("{USAGE}");
            Ok(ExitCode::FAILURE)
        }
    }
}

/// Pull the value of a `-o`/`--output` flag, leaving positional args behind.
fn take_output(args: &[String]) -> asp::Result<(Option<PathBuf>, Vec<String>)> {
    let mut out = None;
    let mut positional = Vec::new();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-o" | "--output" => {
                let v = args
                    .get(i + 1)
                    .ok_or_else(|| asp::Error::Manifest("-o requires a value".into()))?;
                out = Some(PathBuf::from(v));
                i += 2;
            }
            other => {
                positional.push(other.to_string());
                i += 1;
            }
        }
    }
    Ok((out, positional))
}

fn cmd_pack(args: &[String]) -> asp::Result<ExitCode> {
    let (out, positional) = take_output(args)?;
    let dir = positional
        .first()
        .ok_or_else(|| asp::Error::Manifest("pack requires a <skill-dir>".into()))?;
    let out = out.ok_or_else(|| asp::Error::Manifest("pack requires -o <out.skillpack>".into()))?;

    let bytes = asp::pack_dir(Path::new(dir))?;
    std::fs::write(&out, &bytes)?;

    // Re-read the header for an accurate summary.
    let (header, _) = asp::read_header(&bytes)?;
    println!(
        "packed '{}' v{} — {} file(s), {} bytes -> {}",
        header.name,
        header.version,
        header.files.len(),
        bytes.len(),
        out.display()
    );
    Ok(ExitCode::SUCCESS)
}

fn cmd_unpack(args: &[String]) -> asp::Result<ExitCode> {
    let (out, positional) = take_output(args)?;
    let archive = positional
        .first()
        .ok_or_else(|| asp::Error::Manifest("unpack requires an <archive>".into()))?;
    let out = out.ok_or_else(|| asp::Error::Manifest("unpack requires -o <dir>".into()))?;

    let data = asp::read_file(Path::new(archive))?;
    let n = asp::unpack_to(&data, &out)?;
    println!("unpacked {} file(s) into {}", n, out.display());
    Ok(ExitCode::SUCCESS)
}

fn cmd_verify(args: &[String]) -> asp::Result<ExitCode> {
    let archive = args
        .first()
        .ok_or_else(|| asp::Error::Manifest("verify requires an <archive>".into()))?;
    let data = asp::read_file(Path::new(archive))?;
    let report = asp::verify(&data)?;
    if report.ok() {
        println!("OK: {} file(s) verified", report.files_checked);
        Ok(ExitCode::SUCCESS)
    } else {
        eprintln!(
            "FAILED: {} problem(s) across {} file(s):",
            report.problems.len(),
            report.files_checked
        );
        for p in &report.problems {
            eprintln!("  - {p}");
        }
        Ok(ExitCode::FAILURE)
    }
}

fn cmd_info(args: &[String]) -> asp::Result<ExitCode> {
    let mut json = false;
    let mut positional = Vec::new();
    for a in args {
        if a == "--json" {
            json = true;
        } else {
            positional.push(a.clone());
        }
    }
    let archive = positional
        .first()
        .ok_or_else(|| asp::Error::Manifest("info requires an <archive>".into()))?;
    let data = asp::read_file(Path::new(archive))?;
    let (header, _) = asp::read_header(&data)?;

    if json {
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
