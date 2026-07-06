# Signing workflow

Signing gives a `.skillpack` **integrity** (any change to the bytes invalidates
the signature) and **provenance** (a verifier holding your public key can
confirm the archive came from you). It uses detached **ed25519** signatures over
the whole archive byte stream.

Read the honest trust model first: this is **TOFU / bring-your-own-key**, not a
PKI. See [ARCHITECTURE.md](ARCHITECTURE.md#4-signing-and-trust-model).

## 1. Generate a keypair

```sh
agentskillpack keygen -o keys --name author
```

Produces:

- `keys/author.key` — the **private** signing key (64 hex chars). Keep secret.
- `keys/author.pub` — the **public** verifying key (64 hex chars). Distribute.

## 2. Sign an archive

```sh
agentskillpack sign my-skill.skillpack --key keys/author.key
```

This writes `my-skill.skillpack.sig` next to the archive (override with `-o`).
The sidecar is small JSON:

```json
{ "algorithm": "ed25519", "public_key": "<64 hex>", "signature": "<128 hex>" }
```

`sign` refuses to sign an archive that fails internal verification, so you never
put a signature on a corrupt file.

## 3. Verify

```sh
# integrity only
agentskillpack verify my-skill.skillpack

# integrity AND signature against a pinned public key
agentskillpack verify my-skill.skillpack --pubkey keys/author.pub
```

With `--pubkey`, verification:

1. checks internal integrity (per-file SHA-256 + structure), then
2. requires the sidecar's public key to equal the pinned key, then
3. validates the ed25519 signature over the exact archive bytes.

All three must hold. Exit code is non-zero on any failure — so this is a CI
gate.

## Threats this defeats

| Attack                                        | Result                        |
|-----------------------------------------------|-------------------------------|
| Flip a byte in the archive                    | signature invalid → rejected  |
| Swap the archive for a different signed one   | pinned-key mismatch → rejected|
| Re-sign with an attacker key, keep the sidecar key claim | signature invalid → rejected |
| Present a valid signature from an untrusted key | pinned-key mismatch → rejected |

## Key management

You are responsible for distributing and pinning trusted public keys out of
band (e.g. bundling trusted `*.pub` files with your agent runtime, or listing
their hex in a config). agentskillpack deliberately does not fetch keys, trust
keys on first use automatically, or maintain a revocation list — those are
policy decisions for the host.
