# Secure configuration (`rolter init` and `rolter check`)

rolter has two deployment paths, and they are deliberately not converging.

**Local** (`rolter easy-up`) is frictionless and may stay loose on security: no
database, no provider keys, no secrets at all, answering immediately on the
built-in `fake-llm` model. It prompts for nothing, and `rolter check` does not
change that.

**Production** is guided and should be hard to misconfigure. `rolter check`
validates a production deployment *before* the process starts, so a
misconfiguration fails loudly instead of starting degraded.

```bash
rolter init                       # generate a production config and its secrets
rolter check                      # validate the current environment
rolter check --config rolter.toml # also parse and validate the config file
rolter check --strict             # treat warnings as failures too
rolter check --connect            # also probe that the datastores accept a connection
```

The two are a pair. `check` tells an operator what is wrong; `init` tells them
what to write. A test in the tree renders a fresh production environment and
runs the full check suite over it, so what the generator emits is exactly what
the gate accepts — if they ever drift, an operator would follow the documented
path and still be told their deployment is broken.

It exits non-zero when anything fatal is found, which makes it usable as a
container entrypoint step, a Kubernetes init container, or a Helm pre-install
hook.

## Why this exists

The motivating case is worth stating plainly, because it is the shape of every
problem this command is meant to catch.

Provider credentials are encrypted at rest with a key-encryption key read from
`ROLTER_KEK`. When that variable is missing, the control plane's
default-provider seed does not fail — it logs a warning and stores the inline
`api_key` **unsealed**. Nothing goes red. The deployment serves traffic
normally. A credential the operator believed was encrypted at rest simply is
not, and there is no signal saying so.

Until recently several docs pages — including the production deployment guide
and `.env.example` — told operators to set `ROLTER_MASTER_KEY`, a variable
nothing in the codebase has ever read. An operator who followed those
instructions exactly ended up in precisely that degraded state. Those pages are
now corrected, and `rolter check` reports the mistake by name if the old
variable is still set anywhere.

That is the class of failure this gate targets: not a crash, but a deployment
that looks healthy while quietly not doing what it was configured to do.

## What it checks

| Check | Severity | Why |
|---|---|---|
| `ROLTER_KEK` present | error | Without it provider credentials are stored unsealed, with only a warning |
| `ROLTER_KEK` length ≥ 16 | error | The KEK is stretched through SHA-256, so a weak secret still yields a valid — and brute-forceable — key |
| `ROLTER_ADMIN_TOKEN` present | error | Otherwise the management API and `/internal/snapshot` are unauthenticated |
| `ROLTER_DATABASE_URL` present and `postgres://` | error | Otherwise the control plane falls back to an in-memory store and loses all config on restart |
| No example values survive | error | The example database credentials and the e2e throwaway KEK are published in the repository |
| `ROLTER_REDIS_URL` present | warning | Without it rate limits and budgets are enforced per replica, not per deployment |
| Control plane not bound to `0.0.0.0` | warning | The management API should not be reachable on every interface |
| `ROLTER_KEY_PEPPER` present | warning | Without it a leaked virtual-key digest is usable as-is against this deployment |
| CORS does not allow `*` | error | The dashboard is served same-origin; a wildcard lets any page a logged-in operator visits drive the management API as them |
| Datastores accept a connection (with `--connect`) | error / warning | A URL that parses but resolves to nothing fails at first use rather than at rollout |
| `ROLTER_KEK` opens the store (with `--connect`, postgres builds) | error | Every other KEK rule reads the environment alone, so all of them pass on a database restored under a *different* KEK — a failure with no startup symptom at all |
| Config file parses (with `--config`) | error | A config that fails to load leaves the gateway on whatever it last had |
| Every key in the config file is recognised (with `--config`) | warning | An unknown key is ignored rather than rejected, so a typo is silence and a default rather than an error. Also logged as a `WARN` by every process that loads the file — see [below](#the-same-keys-are-reported-at-startup-1434) |

`--connect` is opt-in because a check that opens sockets cannot be the default
for a command meant to run offline and without side effects. It is a TCP
connect, not a protocol handshake: it needs no database driver and answers the
question that actually goes wrong during a rollout — the name does not resolve,
or nothing is listening. A datastore that accepts TCP but rejects the
credentials is a different failure, and one the process reports loudly on its
own.

The KEK-against-the-store check is the one exception to "TCP connect, not a
protocol handshake": it opens a one-connection pool, samples up to 25 rows per
sealed column and tries to decrypt them. It never runs migrations — a check
asked to inspect a database must not change it — and it reports counts, never
plaintext. A store that holds no sealed secrets yet is reported as neither
verified nor broken, because an empty store would let any KEK look correct.
[The backup and restore runbook](backup-and-restore.md) covers what to do when
it fires, and `rolter kek verify` runs the same audit on its own.

Redis and the bind address are warnings rather than errors because a
single-replica deployment behind an ingress is legitimately fine without either.
Use `--strict` in an environment where they are not.

Anything resembling credentials in a URL is redacted before printing, since this
output routinely lands in CI logs.

## Unrecognised keys in `rolter.toml` (#1424)

None of rolter's config types carry serde's `deny_unknown_fields`, and that is a
deliberate, load-bearing choice rather than an oversight. It is what makes the
configuration file forward *and* backward compatible: a `rolter.toml` written
against a newer build stays loadable by an older one, so rolling a release back
does not turn into an outage over a key the previous binary has never heard of.
Nothing in this section changes that — the gateway and the control plane still
accept and ignore every key they do not know.

The price of that promise is silence. `connect_sesc = 3` under `[timeouts]` is
not an error and not a log line; the default applies and the operator is left
debugging a timeout they believe they configured. A key that names a credential
is worse: a misspelled `api_key_env` produces a provider with no credential at
all, and the first symptom is a 401 from the upstream.

So `rolter check --config rolter.toml` reports them, with the path to each and,
where one is close enough to be a plausible typo, the key it probably meant:

```
warn   unrecognised config key providers[0].api_key_evn
       `providers[0].api_key_evn` is not a key rolter reads. It is ignored rather
       than rejected, so the setting it looks like it configures is silently at
       its default. Did you mean `api_key_env`?
```

Nested tables and arrays of tables are covered, so the path is the full one —
`routes[0].targets[0].wieght`, not `wieght`.

These are **warnings**, always. The file is still valid and the process will
still start, so an unknown key may never block a boot; that would be
`deny_unknown_fields` by another name. `--strict` promotes them to failures,
which is what a CI job that lints a config in review wants:

```bash
rolter check --config rolter.toml --strict
```

The detection lives in `crates/rolter-core/src/config_lint.rs` and works by
deserializing the document a second time through `serde_ignored`, which reports
every key the real `Deserialize` impls dropped. It is deliberately not a list of
valid keys: a hand-maintained list drifts within a release, and it drifts in the
direction that reports a brand-new key as a typo. The suggestion comes from a
plain Levenshtein distance against the sibling keys the type actually has.

### The same keys are reported at startup (#1434)

`rolter check` is opt-in, and an operator who mistypes a key does not run it
first — they restart the process and read the log. So every path that loads a
`rolter.toml` logs the same sentence as a `WARN` line before it starts:

- `rolter gateway --config rolter.toml`
- `rolter control --config rolter.toml`
- `rolter easy-up`
- `rolter-seed --import rolter.toml`, before the first row is written

```
WARN rolter_core::config_lint: unrecognised config key: `providers[0].api_key_evn` is not a
     key rolter reads. It is ignored rather than rejected, so the setting it looks like it
     configures is silently at its default. Did you mean `api_key_env`?
     config=rolter.toml key=providers[0].api_key_evn
```

The wiring hangs off `GatewayConfig::load` rather than off each binary, so a new
load path cannot be added without it. It stays **non-fatal everywhere**: the
warning never stops a process from starting and never aborts an import. Only
`rolter check --strict` turns an unknown key into a non-zero exit, and that is a
lint run, not a boot.

Each file is reported once per process. `rolter easy-up` seeds the database,
starts the control plane and starts the gateway from one `rolter.toml`, and one
typo printed three times reads like three problems.

The config the gateway pulls from the control plane's `/internal/snapshot` needs
none of this: that document is JSON assembled from database rows, every column
of which is already constrained by the schema and by the CRUD API that wrote it.
There is no hand-typed key in it to misspell.

## Generating the configuration (`rolter init`)

`rolter check` failing on a missing `ROLTER_KEK` and suggesting `openssl rand
-hex 32` still leaves an operator to invent the rest of the file around it.
`rolter init` writes it.

```bash
rolter init                                  # rolter.toml + .env, production profile
rolter init --profile local                  # the loose defaults, for development
rolter init --print                          # stdout only, for piping into a secret manager
rolter init --env-only --database-url ...    # just the environment
```

It generates four distinct secrets — the KEK, the admin token, the internal
token and the virtual-key pepper — each 256 bits from the system CSPRNG. They
are distinct by construction and a test asserts it: reusing one value in two
roles would mean a leaked admin token also decrypts every stored credential.

**Nothing here is interactive.** A generator that prompts cannot run in the
Dockerfile, the Helm hook or the CI job where a production deployment is
actually assembled, and an operator who has to answer six questions will
hand-write a `.env` instead. Flags carry the choices and the defaults are the
secure ones.

**It will not overwrite without `--force`**, and the refusal says why:
regenerating `ROLTER_KEK` strands every credential encrypted under the old one.
That failure is total, delayed, and looks like data corruption.

On unix the generated files are created `0600` at creation time rather than
chmod'ed afterwards — a `.env` holding the KEK is a credential, and fixing the
mode after the fact leaves a window.

### What the profiles differ on

| | `--profile production` (default) | `--profile local` |
|---|---|---|
| `ROLTER_CONTROL_HOST` | `127.0.0.1` — the management plane is an admin surface | `0.0.0.0` |
| `[server] require_auth` | `true` — revoking the last key closes the data plane | unset, defers to the deployment shape |
| Passes `rolter check` | yes, cleanly | no, and deliberately so |

The data plane binds `0.0.0.0` under both: it is the public surface.

## In a container

Run it as a pre-start step so the container never reaches a serving state while
misconfigured:

```dockerfile
CMD ["sh", "-c", "rolter check --strict && rolter control"]
```

### Docker Compose

The bundled `docker/docker-compose.yml` is the *local* stack and is
deliberately loose — example postgres credentials, no KEK, management plane wide
open. Gating local bring-up on production rules would break the one path that is
meant to have no friction, so the check lives behind a profile and never runs on
`docker compose up`:

```bash
docker compose -f docker/docker-compose.yml --profile preflight \
               run --rm --env-file /path/to/production.env preflight
```

### Kubernetes and Helm

The chart ships the init container already, enabled by default:

```yaml
preflight:
  enabled: true
  strict: true   # a warning means the deployment works but is not what you meant
  connect: false # opt in to also probe the datastores
```

It renders with the **exact** env block the workload container gets — both come
from one shared template, because three deployment paths that each re-implement
"is this configured safely" will drift, and the one that drifts is the one
nobody notices until a credential was stored unencrypted. A `helm chart` CI job
lints the chart and renders every branch of that template.

If you are writing your own manifests, prefer an init container so the failure
is visible as a distinct pod status rather than a crash-looping main container:

```yaml
initContainers:
  - name: preflight
    image: ghcr.io/rolter-ai/rolter:latest
    command: ["/usr/local/bin/rolter", "check", "--strict"]
    envFrom:
      - secretRef:
          name: rolter-secrets
```
