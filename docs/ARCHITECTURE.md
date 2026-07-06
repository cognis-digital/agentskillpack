# Architecture

`agentskillpack` is four small, composable layers built on one byte-oriented
container format. Each layer is a separate Rust module with its own tests, and
each is usable independently.

```
                      +-------------------------------+
   CLI (src/main.rs)  |  pack unpack verify info      |
                      |  manifest schema keygen sign  |
                      |  registry lock                |
                      +---------------+---------------+
                                      |
   library (src/lib.rs) ------------- + ----------------------------
        container  | manifest  |  resolve   |  sign    |  registry
        (.skillpack)| (skill.json)| (semver)  | (ed25519)| (fs store)
```

- **container** (`lib.rs`) — the `.skillpack` binary format: pack/unpack/verify,
  SHA-256 per file, path-traversal defenses.
- **manifest** (`manifest.rs`) — the typed `skill.json` model, capability
  declarations, validation, and the JSON Schema generator.
- **resolve** (`resolve.rs`) — semver dependency resolution and the deterministic
  `skillpack.lock` pinning format.
- **sign** (`sign.rs`) — detached ed25519 signatures and their trust model.
- **registry** (`registry.rs`) — an inspectable, filesystem-backed skill store.

---

## 1. Container format (`.skillpack`)

A `.skillpack` archive is a single byte stream. All multi-byte integers are
**big-endian, unsigned**, so the format is identical on every platform.

```text
+--------------------------------------------------------------+
| MAGIC        | 8 bytes  | ASCII "SKILLPAK"                    |
| FORMAT_VER   | 2 bytes  | u16, currently 1                    |
| HEADER_LEN   | 4 bytes  | u32, byte length of HEADER_JSON     |
| HEADER_JSON  | N bytes  | UTF-8 JSON object (the Header)      |
+--------------------------------------------------------------+
| repeated once per file, in HEADER_JSON.files order:          |
|   BLOB_LEN   | 8 bytes  | u64, byte length of this file       |
|   BLOB_DATA  | L bytes  | raw file contents                   |
+--------------------------------------------------------------+
```

`HEADER_JSON` is:

```json
{
  "format_version": 1,
  "name": "research-skill",
  "version": "1.4.2",
  "description": "optional",
  "files": [
    { "path": "skill.json", "size": 456, "sha256": "<64 hex chars>" }
  ]
}
```

File blobs follow the header **in the exact order** of `files`, so a reader can
stream them out sequentially without seeking. Each entry records the relative
`path` (forward-slash normalized; absolute paths, drive prefixes and `..`
traversal are rejected on both pack and unpack), the `size`, and the lowercase
hex `sha256` of the contents.

### Integrity model

`verify` (and `unpack`) recompute the SHA-256 of every blob and compare it to
the recorded digest, and check that each blob length matches its recorded size.
Structural checks reject bad magic, an unsupported `format_version`, truncated
data, and trailing bytes after the final blob. Any failure makes `verify` exit
non-zero — so it works as a CI gate.

### Versioning

The format version is recorded twice — the binary `FORMAT_VER` field and
`HEADER_JSON.format_version` — and both must equal the version this build
understands. A future incompatible layout bumps this number so old readers
refuse new archives cleanly rather than misparse them.

---

## 2. Manifest model (`skill.json`)

The container needs only a name, version, and file list. The **manifest** adds a
typed, versioned description of the skill so a host can reason about it before
running any code. See [`skill.schema.json`](skill.schema.json) for the machine
form (JSON Schema 2020-12, generated from the Rust model and kept in sync by a
test).

Key fields:

| Field          | Meaning                                                        |
|----------------|----------------------------------------------------------------|
| `name`         | lowercase identifier `[a-z0-9._-]+`, no leading/trailing sep   |
| `version`      | semver 2.0.0                                                   |
| `capabilities` | typed list of requested powers (see below)                     |
| `dependencies` | `{name, req}` where `req` is a semver requirement              |
| `inputs`/`outputs` | typed field declarations                                   |
| `compat.engine`| semver requirement on the agent runtime                        |

Unknown keys are **rejected** (`deny_unknown_fields`) so typos surface loudly.
`manifest validate` collects *all* problems in one pass rather than stopping at
the first.

### Capabilities

A capability is a **declaration of intent**, not a grant. Listing `net` does not
open a socket; it tells a host the skill intends to use the network. Absent
capabilities are an assertion the skill will *not* use them, which a sandbox can
enforce.

| Kind       | Meaning                              |
|------------|--------------------------------------|
| `fs.read`  | read files from the host filesystem  |
| `fs.write` | write/create files                   |
| `net`      | open network connections            |
| `exec`     | spawn subprocesses                   |
| `env`      | read environment variables           |

Each capability can carry a `scope` list narrowing it (paths, hosts, var names).
See [capabilities.md](capabilities.md) for how a host turns these into policy.

---

## 3. Resolver and lockfile

Dependencies are declared as `(name, semver-requirement)` pairs. Given a
**catalog** of available versions (typically produced from a registry), the
resolver picks, for every requirement, the **highest available version** that
satisfies it:

```
for each dependency (name, req):
    candidates = catalog.versions(name).filter(|v| req.matches(v))
    pick = candidates.max_by(version)          # highest compatible
    if pick is None: record a Conflict
emit Lockfile { pins: sorted_by_name([...]) }
```

This is a flat, non-transitive "highest compatible" selection (like a minimal
`cargo`/`npm` resolve), **not** a SAT solver. It never touches the network and
never mutates its inputs. Requirements with no satisfying version are reported
as `Conflict`s, not silently dropped.

The output is `skillpack.lock` — deterministic JSON (pins sorted by name),
byte-identical across runs for identical inputs, pinning each dependency to an
exact `version` and archive `sha256`:

```json
{
  "lock_version": 1,
  "root": "research-skill",
  "root_version": "1.4.2",
  "pins": [
    { "name": "hello-skill", "req": "^1.0", "version": "1.0.0", "sha256": "..." }
  ]
}
```

---

## 4. Signing and trust model

Signatures are **detached ed25519** over the *entire archive byte stream*
(magic, version, header, and all blobs). Because the header already binds every
file's SHA-256, signing the container transitively signs the file contents — and
the signature additionally covers the header and structure itself.

### Trust model — read this honestly

This is **TOFU / bring-your-own-key**, not a PKI:

- no certificate authority, no key transparency log, no revocation;
- a signature proves only that *whoever holds the private key* signed *these
  exact bytes*;
- the host decides which public keys it trusts (typically by pinning them out of
  band, e.g. shipping trusted `*.pub` files with the runtime).

What signing buys you over an unsigned archive:

- **Integrity** across the whole container (stronger than, and independent of,
  the per-file hashes).
- **Provenance**: a verifier holding the signer's public key can confirm the
  archive came from that signer and was not swapped.

What it does **not** buy you: it does not establish that the signer is
trustworthy, nor that the private key was not stolen. That is a
key-management problem the host owns.

Keys are stored as 64 lowercase hex characters (32 raw bytes). A signature
sidecar (`<archive>.sig`) is small JSON:

```json
{ "algorithm": "ed25519", "public_key": "<64 hex>", "signature": "<128 hex>" }
```

`verify --pubkey <key>` checks integrity **and** requires the sidecar's key to
equal the pinned key before validating the signature — so signing with the
wrong key, tampering with the bytes, or swapping the signer are all rejected.

---

## 5. Registry layout

The registry is a plain directory — no server, no database. You can inspect,
diff, back it up, or ship it on a USB stick.

```text
<root>/
  index.json                     # the Index: what is installed
  skills/
    <name>/
      <version>/
        skill.skillpack          # the stored archive
```

Every `add` **verifies the archive's internal integrity** before installing and
records the archive's overall SHA-256 in the index. Every `resolve` re-verifies
the on-disk archive hash against the index before returning a path, so a
tampered store is caught. Installing the same name+version with different bytes
is refused unless `--force` is given. `index.json` is sorted deterministically
(by name, then semver) so it is stable across runs.

The registry can produce a resolver `Catalog` (`Registry::catalog`), so `lock`
pins a skill's dependencies against exactly what is installed.
