# An explicit per-provider opt-out from a hosted kind's host pin

**Status:** Accepted · **Date:** 2 Sep 2026 · **Issues:** [#925](https://github.com/rolter-ai/rolter/issues/925), [#924](https://github.com/rolter-ai/rolter/issues/924)
**Relates:** ADR-0022 (config-vs-DB entity tiering)

## Context

`ProviderKind::Openrouter` pinned `api_base` to one literal string:

```rust
if provider.api_base.trim_end_matches('/') != "https://openrouter.ai/api/v1" {
```

Paired with the rule that an openrouter provider must source its key from
`api_key_env`, the pin closes a real exfiltration path. A hosted kind names one
operator, and the credential belongs to that operator. Without the pin, a single
edited line in a config file — or a row in the providers table — would send a
live OpenRouter key to whatever host the edit named, over TLS, with no error and
no signal that anything changed.

The pin also rules out three things operators legitimately do:

- put a corporate egress proxy or internal gateway in front of the vendor
- point at an endpoint that re-implements the vendor's surface
- **test the dialect at all** — which is how this surfaced. The #924 dogfooding
  fleet could not declare `kind = "openrouter"` against a local fake, so it
  declared the endpoint `openai_compatible` instead and the OpenRouter adapter
  went unexercised end to end.

So this is a decision about how an operator opts out, not a bug fix. The
question the options have to answer is what stops the opt-out from quietly
becoming the shape everyone copies off a blog post.

## Options considered

1. **Allow loopback and private-range hosts unconditionally.** Enough to test
   with, no help for a corporate proxy, and it makes the rule depend on where
   rolter happens to run rather than on what the operator intended.
2. **An explicit `allow_custom_api_base = true` on the provider.** The safe
   default is unchanged; the unsafe case is one reviewable line in the
   configuration, in the same diff as the base URL it enables.
3. **Drop the pin and rely on the `api_key_env` rule alone.** That rule keeps
   the secret out of config files and database rows, but it does nothing about
   where the secret is *sent* — which is the exfiltration path the pin closes.

## Decision

Option 2. `ProviderConfig` gains `allow_custom_api_base: bool`, defaulting to
`false`:

- Unset, every hosted kind keeps the exact validation it has today.
- Set, the host pin is skipped for that provider and nothing else is relaxed.
  In particular the `api_key_env` requirement still applies, so the credential
  cannot be inlined next to the redirected base URL.
- The rejection message names the opt-out, so the operator who hits the pin
  learns the supported way past it instead of inventing one.
- `rolter check` reports every provider that sets it as a warning, and
  `rolter check --strict` fails on it. The opt-out is legitimate; being
  invisible is what would make it dangerous.

Only file-configured providers can set it today. Providers stored in Postgres
keep the safe default: the column, the CRUD surface and the dashboard control
are a separate change, tracked as its own issue.

## Consequences

- The dogfooding fleet declares `kind = "openrouter"` against `127.0.0.1:18002`,
  so the OpenRouter dialect is exercised by a running fleet for the first time.
- An operator behind an egress proxy can use the hosted kind rather than
  downgrading to `openai_compatible` and losing the dialect's behaviour.
- A deployment that does not set the flag is bit-for-bit unaffected.
- The flag is per provider, not global, so enabling it for a test endpoint
  cannot silently unpin a production one beside it.
- A stored provider cannot opt out until the DB surface lands, which is a
  visible asymmetry between the two configuration tiers for as long as it
  stands.
