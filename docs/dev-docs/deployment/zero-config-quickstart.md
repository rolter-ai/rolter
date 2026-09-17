# Zero-config quickstart: no keys, add providers at runtime

rolter starts with **zero LLM credentials** and serves the built-in `fake-llm`
model out of the box. Real providers, models, and upstream API keys are added
later — at runtime, over the management API, persisted in Postgres, and
picked up by the gateway without a restart.

## 1. Start with no credentials

```bash
rolter easy-up
```

That's it. No provider keys, no database, no config file (one is created from
the bundled example on first run). The gateway answers immediately:

```bash
curl http://localhost:4000/v1/chat/completions \
  -H "Authorization: Bearer sk-rolter-dev" \
  -H "Content-Type: application/json" \
  -d '{"model": "fake-llm", "messages": [{"role": "user", "content": "hi"}]}'
```

`sk-rolter-dev` is the local-dev virtual key from the generated
`rolter.toml`; delete the `[[virtual_keys]]` section to run open, or replace
it before exposing the gateway anywhere.

## 2. Switch on runtime management (Postgres mode)

Runtime CRUD over providers/models/keys needs the database-backed control
plane:

```bash
export ROLTER_ADMIN_TOKEN="$(openssl rand -hex 24)"   # protects the management API
export ROLTER_KEK="$(openssl rand -hex 32)"           # encrypts provider keys at rest

rolter easy-up --database-url postgres://user:pass@localhost:5432/rolter
```

`easy-up` migrates, seeds a `default` org/team/project, imports the bootstrap
toml, and starts both planes. The gateway port (4000) now also serves the
management API: `/admin/*` proxies to the control plane's `/api/v1/*`.

Two deployment secrets matter here:

- **`ROLTER_ADMIN_TOKEN`** — bearer token required on the management API and
  the internal snapshot endpoint. Without it those endpoints are **open**
  (fine on localhost; a startup warning reminds you).
- **`ROLTER_KEK`** — key-encryption key. Provider API keys submitted over the
  API are sealed with AES-256-GCM before they reach Postgres; the KEK never
  leaves the process environment. Set the same value on the control plane and
  gateway (with `easy-up` it is one process, so one export). Without a KEK,
  requests that include an `api_key` are rejected — there is no plaintext
  fallback.

## 3. Add your first real provider — with its key — via curl

```bash
BASE=http://localhost:4000/admin

# ids seeded by easy-up
ORG=$(curl -s $BASE/orgs -H "Authorization: Bearer $ROLTER_ADMIN_TOKEN" | jq -r '.[] | select(.name=="default") | .id')
TEAM=$(curl -s $BASE/orgs/$ORG/teams -H "Authorization: Bearer $ROLTER_ADMIN_TOKEN" | jq -r '.[0].id')
PROJECT=$(curl -s $BASE/teams/$TEAM/projects -H "Authorization: Bearer $ROLTER_ADMIN_TOKEN" | jq -r '.[0].id')

# provider + upstream credential (sealed at rest; never returned by the API)
PROVIDER=$(curl -s -X POST $BASE/orgs/$ORG/providers \
  -H "Authorization: Bearer $ROLTER_ADMIN_TOKEN" -H "Content-Type: application/json" \
  -d '{"name": "openai", "kind": "openai", "api_base": "https://api.openai.com", "api_key": "sk-..."}' \
  | jq -r .id)

# public model name + target
ROUTE=$(curl -s -X POST $BASE/projects/$PROJECT/routes \
  -H "Authorization: Bearer $ROLTER_ADMIN_TOKEN" -H "Content-Type: application/json" \
  -d '{"model": "gpt-4o", "strategy": "round_robin"}' | jq -r .id)

curl -s -X POST $BASE/routes/$ROUTE/targets \
  -H "Authorization: Bearer $ROLTER_ADMIN_TOKEN" -H "Content-Type: application/json" \
  -d "{\"provider_id\": \"$PROVIDER\", \"upstream_model\": \"gpt-4o\"}"
```

Within the snapshot poll interval (instantly with `--redis-url`) the gateway
serves the new model — no restart:

```bash
curl http://localhost:4000/v1/models -H "Authorization: Bearer sk-rolter-dev"
```

## 4. Rotate or remove a credential

`PUT /admin/providers/{id}` updates a provider in place. For `api_key`,
`api_key_env`, and `egress_proxy`: omit the field to leave it unchanged, send
an empty string to clear it, send a value to set/rotate it.

```bash
# rotate
curl -X PUT $BASE/providers/$PROVIDER \
  -H "Authorization: Bearer $ROLTER_ADMIN_TOKEN" -H "Content-Type: application/json" \
  -d '{"api_key": "sk-new-key"}'

# remove the stored key (falls back to api_key_env, if set)
curl -X PUT $BASE/providers/$PROVIDER \
  -H "Authorization: Bearer $ROLTER_ADMIN_TOKEN" -H "Content-Type: application/json" \
  -d '{"api_key": ""}'
```

## How the pieces fit

- **Persistence** — providers/routes/keys live in Postgres; credentials in
  `provider_keys`, AES-256-GCM-sealed with the `ROLTER_KEK`-derived key.
- **Propagation** — every write bumps `config_version` (a database trigger,
  transactional with the write). Gateways poll
  `GET /internal/snapshot?version=N` and hot-swap their routing snapshot;
  with Redis configured the control plane also publishes a bump for instant
  refetch. See [Config & hot reload](../architecture/config-and-hot-reload.md).
- **Two surfaces, one API** — `/admin/*` on the gateway is a thin reverse
  proxy to the control plane's `/api/v1/*` (enable on a standalone gateway
  with `--admin-url http://control:4001`); pointing tooling at either works
  identically. Authentication is enforced by the control plane in both cases.
- **Config file still wins** — anything declared in the bootstrap
  `rolter.toml` is a read-only "config model" (LiteLLM-style): the API
  rejects runtime mutations to it with `409`.
- **Reads never leak secrets** — `GET /api/v1/config` (the dashboard read)
  redacts `api_key`; only the token-guarded snapshot endpoint carries
  decrypted keys, because the gateway needs them to call upstreams.
