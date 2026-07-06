# CI verification

`agentskillpack verify` exits **non-zero** on any integrity or signature
failure, so it drops straight into a CI pipeline as a gate. Nothing downstream
runs if a skill archive is corrupt, tampered, or signed by the wrong key.

## Verify integrity in CI

```yaml
# GitHub Actions
- name: Verify skill archive
  run: agentskillpack verify dist/my-skill.skillpack
```

## Verify integrity AND provenance

Pin the trusted public key in the repo (or a secret) and require a valid
signature:

```yaml
- name: Verify signed skill archive
  run: |
    agentskillpack verify dist/my-skill.skillpack \
      --pubkey trusted/author.pub
```

A failed signature, a tampered byte, or a signature from any key other than
`author.pub` all fail the step.

## Validate manifests on every push

Catch manifest regressions (bad semver, undeclared-but-used fields, malformed
capabilities) before packing:

```yaml
- name: Validate manifest
  run: agentskillpack manifest validate skills/my-skill
```

## Enforce a lockfile is current

Regenerate the lock and fail if it drifts from what is committed — the lockfile
is deterministic, so any diff means the dependency set changed:

```yaml
- name: Check lockfile is up to date
  run: |
    agentskillpack lock skills/my-skill --registry ./reg -o /tmp/skillpack.lock
    diff -u skillpack.lock /tmp/skillpack.lock
```

## Exit codes

| Code | Meaning                                              |
|------|------------------------------------------------------|
| `0`  | success / all checks passed                          |
| non-0| integrity failure, signature failure, invalid input, or a bad manifest |

Because every failure path is non-zero and every success is zero, you can chain
commands with `&&` or rely on a job step's own failure semantics.

## A full gate

```sh
set -e
agentskillpack manifest validate skills/my-skill
agentskillpack pack skills/my-skill -o build/my-skill.skillpack --validate
agentskillpack sign build/my-skill.skillpack --key "$SIGNING_KEY_FILE"
agentskillpack verify build/my-skill.skillpack --pubkey trusted/author.pub
```
