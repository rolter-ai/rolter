# Backup, restore and KEK rotation

The control-plane database holds the things an operator cannot regenerate:
provider credentials sealed under the KEK, virtual keys, routes, budgets, RBAC
assignments and the audit log. This page is the runbook for getting them back.

Backing up Postgres is a solved problem and is not the interesting part. The
interesting part is that **the KEK is not in the dump**, and a restore onto the
wrong one fails silently.

## What has to be in the backup set

| Item | Where it lives | What its loss costs |
|---|---|---|
| the control-plane database | Postgres | everything below, plus routes, budgets, RBAC and the audit log |
| `ROLTER_KEK` | your secret manager, **not** the database | every sealed secret in that database, permanently |
| `ROLTER_KEY_PEPPER` | your secret manager | every virtual key hash stops matching; keys must be reissued |
| the gateway's `rolter.toml`, if it has one | config management | reconstructible, but not from the dump |

A backup of the database alone is not a backup. The KEK is stored separately by
design — that is the whole point of encrypting at rest — which means the one
thing a `pg_dump` cannot carry is the one thing the restore needs most.

## Backup

```bash
pg_dump "$ROLTER_DATABASE_URL" --format=custom --file rolter-$(date +%F).dump
```

`--format=custom` is worth the flag: it compresses, and it lets a restore be
parallelised and selective. Nothing in the schema requires a special dump
option; there are no large objects and no extensions beyond `pgcrypto`.

Verify the KEK is in your secret manager under a name that ties it to *this*
database, not to "production" in general. Two deployments with two KEKs and one
label is the most common way to restore onto the wrong key.

## Restore

Restore into a database migrated from scratch rather than over a live one, so a
restore cannot resurrect a schema the binary no longer expects:

```bash
createdb rolter_restored
pg_restore --dbname "$RESTORED_URL" --no-owner --no-privileges rolter-2026-09-02.dump
```

Then, **before starting the control plane**, prove the KEK matches:

```bash
ROLTER_DATABASE_URL=$RESTORED_URL ROLTER_KEK=... rolter kek verify
```

```text
ROLTER_KEK opens all 12 sampled secrets across 3 columns
```

`rolter check --connect` runs the same audit as part of pre-boot validation, so
a deployment that gates on `rolter check` gets this for free.

### When the KEK does not match

```text
error  ROLTER_KEK does not match the one this store was sealed with
       ROLTER_KEK does not open secrets already in this database. This is what a
       restore onto the wrong KEK looks like: the dump carried the ciphertext,
       the KEK was not in the dump, and nothing else fails until an upstream
       call does.
       - provider_keys.ciphertext holds upstream provider credentials — 3 of 3
         sampled rows could not be decrypted
```

There is no recovery path that does not involve the original KEK. AES-256-GCM
is doing exactly what it was chosen to do. Your options, in order:

1. **Find the original KEK.** Check the secret manager version history, the
   deployment that wrote the dump, and any sealed-secret manifest in the
   cluster.
2. **Re-enter the credentials.** The rest of the database — routes, keys,
   budgets, RBAC, audit log — is unaffected. Clear the unreadable rows and add
   each provider credential again through the dashboard. Everything else
   survives.
3. Never step three. Do not start the control plane against a store it cannot
   read and hope; that is how an unreadable credential becomes an upstream
   outage attributed to the provider.

The reason this check exists is that without it the failure has **no startup
symptom at all**: the process boots, the dashboard renders every provider, and
the first sign of trouble is a 401 from an upstream hours later.

## KEK rotation

Rotation reseals every stored secret from the old KEK to a new one. It is a
whole-database operation: a partially rotated store is readable by neither key,
so the entire pass runs in one transaction and a single row the old KEK cannot
open aborts all of it.

```bash
# 1. take a backup first — rotation is not reversible without the old KEK
pg_dump "$ROLTER_DATABASE_URL" --format=custom --file pre-rotation.dump

# 2. stop the writers. The control plane, and any `rolter-seed` run
kubectl scale deployment/rolter-control --replicas 0

# 3. dry run: what would be resealed, and does the old KEK still open it
export ROLTER_KEK_OLD="$(current KEK)"
export ROLTER_KEK="$(openssl rand -hex 32)"
rolter kek rotate

# 4. apply
rolter kek rotate --apply

# 5. publish the new KEK to every process, then start them
kubectl scale deployment/rolter-control --replicas 3
```

Both KEKs are read from the environment rather than from flags, because a KEK
on a command line lands in shell history, in `ps` output, and in whatever
collects a container's arguments.

Between step 4 and step 5 the fleet is split: a gateway still holding the old
value can no longer read a stored credential. Keep the window short, and roll
the gateways after the control plane so `/internal/snapshot` is already serving
secrets the new KEK sealed.

Rotation does not touch `ROLTER_KEY_PEPPER`. Virtual-key hashes are not
reversible, so a pepper change is a reissue, not a rotation.

## What is audited

`rolter kek verify` samples every sealed column in the schema:

| Table | Holds |
|---|---|
| `provider_keys` | upstream provider credentials |
| `sso_providers` | SSO client secrets |
| `alert_channels` | alert channel webhook secrets |
| `security_settings` | the dashboard's own upstream credential |
| `observability_connectors` | observability connector credentials |
| `user_totp_factors` | TOTP second-factor shared secrets |
| `mcp_servers` | MCP OAuth client secrets |
| `mcp_oauth_login_states` | in-flight MCP OAuth PKCE verifiers |
| `mcp_oauth_sessions` | MCP access and refresh tokens |

The inventory lives in `crates/rolter-store/src/postgres/kek_audit.rs`. A test
asks the schema for every `*_ciphertext` column and fails when one is missing
from the list, so a new sealed column cannot quietly escape the audit — but the
row in the maintenance matrix is still the thing to read when adding one.

## Restore drills

A backup nobody has restored is a hypothesis. The property this page depends on
is covered by tests in `kek_audit.rs`: a seeded store is dumped, restored into a
freshly migrated database, and the sealed credential is asserted to still
decrypt — then asserted to be *caught*, not booted into, under a different KEK.

Run the same drill against your own deployment on the cadence your recovery
objective implies, and make step "does `rolter kek verify` pass" part of it.
