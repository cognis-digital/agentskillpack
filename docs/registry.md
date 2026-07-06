# Registry usage

The registry is a local, filesystem-backed store of installed skills. It is a
plain directory you can inspect, diff, back up, or copy — no server, no
database. See the layout in
[ARCHITECTURE.md](ARCHITECTURE.md#5-registry-layout).

## Add a skill

```sh
agentskillpack registry add my-skill.skillpack --registry ./reg
```

The archive is **fully verified** before it is stored, and its overall SHA-256
is recorded in `reg/index.json`. Adding the same name+version with different
bytes is refused unless you pass `--force`.

## List installed skills

```sh
agentskillpack registry list --registry ./reg
```

```
hello-skill              1.0.0        91805ef91d42a833  3 file(s)
research-skill           1.4.2        efb49156bf085b91  4 file(s)
```

Use `--json` for machine-readable output.

## Resolve a version

```sh
agentskillpack registry resolve hello-skill --req '^1.0' --registry ./reg
```

Resolves the **highest installed version** satisfying the semver requirement,
re-verifies the stored archive's hash against the index (catching on-disk
tampering), and prints its path:

```
hello-skill v1.0.0 -> ./reg/skills/hello-skill/1.0.0/skill.skillpack
```

`--req` defaults to `*` (any version). Add `--json` for structured output.

## Remove a version

```sh
agentskillpack registry remove hello-skill --version 1.0.0 --registry ./reg
```

Deletes the archive and prunes now-empty directories, then rewrites the index.

## Locking dependencies against a registry

The registry doubles as the resolver's catalog. Given a skill that declares
dependencies, `lock` pins them to exactly what is installed:

```sh
agentskillpack lock my-skill --registry ./reg -o skillpack.lock
```

The resulting `skillpack.lock` is deterministic and records each dependency's
exact version and archive SHA-256. Commit it so downstream consumers install the
same bytes you resolved. See the resolver algorithm in
[ARCHITECTURE.md](ARCHITECTURE.md#3-resolver-and-lockfile).

## Inspecting the store by hand

Because it is just files, you can audit it directly:

```sh
cat reg/index.json
ls -R reg/skills
agentskillpack verify reg/skills/hello-skill/1.0.0/skill.skillpack
```
