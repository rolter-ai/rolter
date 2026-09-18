# Competitive study: an open-source gateway worth learning from

A read-only study of [`experientiallabs/experiential`](https://github.com/experientiallabs/experiential)
(Apache-2.0), carried out to find gaps in rolter. It is a _comparison_, not a plan
of record: the ideas it recommends are tracked as GitHub issues, and the ones it
rejects are recorded here so they do not get re-proposed every quarter.

The study was done against commit `a3da051` (2026-09-17), reading the public
README, `SETUP.md`, `docs/`, and the package layout. Nothing was copied.

## What it is

A Python-packaged gateway and router for agent workflows, installed with
`pip install experiential` and driven by one CLI (`exp`). Its three claims:

1. one OpenAI-compatible API over hosted, BYOK and local models;
2. per-identity control of which models may be used and how much may be spent;
3. turning production traffic into a router (or a fine-tuned model) optimised
   for quality, speed and cost.

Shape of the thing:

- **Data plane** — a compiled native (Rust) component, `exp-gateway-native`,
  published as a wheel. It binds loopback only, accounts into SQLite, and serves
  Chat Completions, Responses (including a Responses-over-WebSocket transport for
  the Codex CLI), Anthropic Messages plus `count_tokens`, Embeddings, Image
  generations, and a typed decision endpoint of their own design.
- **Control surface** — Python. Identities, virtual keys, grants, aliases, exact
  model pools, monthly limits, guardrail policies, and the optimiser.
- **Optimiser** — the distinctive half. Ingest traces, build a simulation, fit a
  routing policy against it, evaluate on held-out evidence, emit a report. An
  optional path fine-tunes an open-weights model through a third-party service.
- **Hosted platform** — a managed gateway with credits, instant email signup, and
  an `llms.txt` contract so a coding agent performs the whole onboarding.
- **Scale posture** — single-node, loopback, SQLite. Their own research note
  documents a shared-SQLite durability ceiling and tells operators to scale
  horizontally instead.

The single sharpest observation from the whole exercise: they treat **onboarding
as a machine-consumed API**. `SETUP.md` is four self-contained prompts a user
pastes into Claude Code or Cursor; the agent then creates the account, stores the
key, wires the SDK, and repoints the user's other agents. Adoption cost is one
paste. rolter's adoption cost is reading a quickstart.

## Feature-by-feature

| Area                 | Them                                                                                                                    | rolter                                                                                                        | Verdict                                                                                                |
| -------------------- | ----------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------ |
| OpenAI surface       | Chat Completions, Responses (+WebSocket), Embeddings, Images, decisions                                                 | Chat Completions, Responses, Embeddings, Messages, audio, realtime, MCP                                       | rolter ahead on breadth; they have the Codex WebSocket transport and `count_tokens` (tracked in #1016) |
| Anthropic surface    | Messages + `count_tokens` (estimated, with a disclosure header)                                                         | Messages                                                                                                      | Gap: `count_tokens` (already in #1016)                                                                 |
| Provider kinds       | OpenAI, Anthropic, Gemini, Azure, Bedrock, Vertex, Fireworks, OpenRouter, openai-compatible                             | Substantially more, plus self-hosted engines, TEI, llama.cpp, Ollama                                          | rolter ahead                                                                                           |
| BYOK                 | Keys stored in a user-data file outside the repo; catalog stays secret-free                                             | KEK-sealed in Postgres, rotation and a restore audit, env-var indirection                                     | rolter well ahead                                                                                      |
| Model catalog        | Secret-free `models.toml`; every alias declares capabilities and prices explicitly                                      | Dashboard catalog, pricing overrides, model defaults                                                          | Comparable; price _import_ still manual in rolter (#966)                                               |
| Routing              | Ordered exact-model pools with per-rung conditional failover by failure class                                           | Strategies (weighted, cache-aware, adaptive, complexity, disaggregated), circuit breakers, retries, cooldowns | rolter far ahead on live routing; **behind on conditional failover**                                   |
| Cost intelligence    | Router fitted offline from real traffic, with held-out evaluation and a report                                          | Adaptive blend uses live cost as a scoring signal; #1468 plans an evaluation harness                          | Gap: no offline "what would this have cost" analysis                                                   |
| Model recommendation | Yes — the fitted router _is_ the recommendation                                                                         | No                                                                                                            | Gap                                                                                                    |
| Trace ingest         | Nine declared sources: OTLP, OTel GenAI, PostHog, Braintrust, Langfuse, LangSmith, Mastra, Phoenix, chat-json           | None — rolter only sees traffic it served                                                                     | Gap                                                                                                    |
| Observability        | Content-free SQLite accounting, loopback usage view, OTLP in                                                            | ClickHouse request logs, MCP logs, cost attribution, alerting, tenant telemetry destinations, dashboards      | rolter far ahead                                                                                       |
| Guardrails           | Identity-scoped, default-off, input+output chains, pluggable classifiers                                                | Guardrail rules and guardrail providers as first-class resources                                              | Comparable; rolter has the richer admin surface                                                        |
| Budgets              | Monthly integer nano-USD enforcement; a _cost preflight_ that refuses an expensive operation before it dials a provider | Budgets, limits, cost attribution; reservations planned in #1464                                              | Comparable, converging                                                                                 |
| AuthZ                | Identities, grants, virtual keys                                                                                        | Full RBAC matrix, custom roles, access profiles, SSO/OIDC, SCIM, LDAP, TOTP, audit log                        | rolter far ahead                                                                                       |
| Dashboard            | Hosted platform only; local is a terminal UI                                                                            | 60+ screens, i18n, Storybook, capability gating, loading/empty/error contracts                                | rolter far ahead                                                                                       |
| Onboarding           | Interactive first-run wizard, then agent-pasteable prompts and `llms.txt`                                               | `easy-up`, `fake-llm`, a written quickstart; no wizard, no `llms.txt`                                         | **Behind**                                                                                             |
| Deployment           | pip install, loopback, SQLite, or their hosted platform                                                                 | Docker, Helm, Kubernetes, air-gapped, cluster mode, backup/restore/KEK rotation                               | rolter far ahead                                                                                       |
| Docs                 | ~17 dense reference pages, very precise, contract-flavoured; no tutorials                                               | mdBook developer docs + Mintlify user docs, 32 ADRs, nav gates in CI                                          | rolter ahead on breadth and navigation; their per-page precision is worth imitating                    |
| Marketplace          | Hosted model marketplace with credits and instant signup                                                                | None, and none wanted                                                                                         | Out of scope by design                                                                                 |

### Where rolter is plainly ahead

Worth stating so the study is not read as a list of deficiencies. rolter is a
multi-tenant, horizontally scalable, operator-owned gateway; theirs is a
single-node developer tool with a hosted upsell. rolter wins outright on
multi-tenancy and RBAC, storage and the control/data-plane split, the dashboard,
MCP as a governed surface, semantic and cache-aware routing, alerting, audit,
air-gapped and Kubernetes deployment, key sealing and rotation, API stability
guarantees, and the ADR trail behind all of it. None of the ideas below trade any
of that away.

## Ideas worth stealing, ranked

Each is filed as a GitHub issue (#1581 through #1586); the ranking is value
against effort.

1. **A savings advisor over rolter's own request logs** (#1581). rolter already stores
   every invocation with model, tokens, latency and price. It has never turned
   that into a sentence an operator can act on: _"38% of traffic on this route
   was short, tool-free and single-turn; the same requests on the cheaper model
   in the pool project to $X/month."_ This is their central claim, achievable
   in rolter without any of their machinery, because rolter's logs are richer
   than the traces they have to import. Highest value, moderate effort.
2. **`llms.txt` plus agent-pasteable setup prompts** (#1582). Make a coding agent able
   to stand rolter up: one machine-readable contract served by the docs site and
   a handful of self-contained prompts. Very low effort for a real drop in
   adoption cost, and it composes with the agent-CLI routing epic (#1012).
3. **Import LLM traces from external observability tools** (#1583). OTLP first, then the
   common vendor exports. It makes the advisor useful on day zero — before a
   single request has gone through rolter — which is exactly when a prospect is
   deciding. Depends on (1) to be worth anything.
4. **Per-target conditional failover** (#1584). rolter's fallback order is positional and
   its retry class is global. Letting a target declare _which failure classes it
   may take over_ (throttling only; refusals only; a named refusal category only)
   turns a fallback pool into a policy, and is the one routing idea they have
   that rolter does not.
5. **A first-run setup wizard in the dashboard** (#1585). Connect a provider, create a
   route, mint a key, make the first call — with the built-in `fake-llm` as a
   zero-credential first step. rolter's zero-config story is genuinely good and
   the dashboard does not tell it.
6. **Disclose altered request parameters on the response** (#1586). When the gateway
   drops or rewrites a request field for compatibility, say so in a response
   header instead of silently succeeding. Small, and it removes a class of
   "why did my `top_k` do nothing" support question.

## Ideas to reject, and why

- **Fine-tuning open-weights models from traffic.** A different product with a
  different operational burden (GPU budgets, model hosting, dataset governance).
  rolter routes; it does not train.
- **Simulation and world-model machinery.** Their router fitting is built on a
  simulated environment mined from traces. The evidence-first framing in #1468 is
  the right depth for rolter; the simulator is not.
- **A hosted marketplace with credits and instant email signup.** rolter is
  self-hosted by design, and instant account creation over an unauthenticated
  endpoint is a posture rolter should not adopt.
- **Anonymous product telemetry on by default.** They send aggregate PostHog
  events unless disabled. rolter ships air-gapped and tenant-scoped telemetry
  destinations; default-on phone-home would contradict both.
- **A typed decision endpoint of their own invention.** Non-standard surface
  area. rolter's compatibility promise is that clients written for OpenAI and
  Anthropic work unchanged.
- **Single-node SQLite accounting.** Their own benchmark note names the
  durability ceiling. rolter already made the other choice.
- **Portable project bundles.** rolter's config export/import round trip covers
  the real need and is already documented.

## Licensing and attribution

Their code is Apache-2.0. Ours is not theirs to relicense, and theirs is not ours
to absorb.

- **No code is copied**, in whole or in part, translated or paraphrased. Nothing
  in this study was derived by reading an implementation file; it is all from the
  public README and reference docs.
- **No wording is copied.** Their documentation prose, error strings, CLI help
  and product copy stay theirs. Every string that ships in rolter is written
  fresh.
- **Naming.** The project is named in this study and in issue bodies, which is
  the correct place for it. It is not named in `ui/src`, in any user-facing
  string, in `docs/user-docs/`, or in any shipped artefact.
- **Ideas are not the code that expresses them.** "Fit routing from recorded
  traffic" and "declare which failure classes a fallback target accepts" are
  approaches, and rolter will express them in its own design against its own
  data model.
- If any concrete artefact of theirs is ever wanted verbatim — a schema, a test
  vector, a fixture — it is an Apache-2.0 intake decision with a NOTICE
  obligation, taken deliberately in its own PR and not inside a feature branch.

## A note on placement

This page lives under `development/` rather than a `research/` section because
`docs/research/` is not a tracked directory in this repository — it is local
scratch. If a research section is ever created, this page moves with it.
