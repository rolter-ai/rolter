# LLM / ML / software engineer

**Member at a project.** Calls models from code, notebooks and coding agents;
wants a key without filing a ticket, and — when something breaks — wants to find
out _where_ without waiting for an admin. Has no business with providers,
budgets, people or policy, and the dashboard should not put those in the way.

Two things make this persona sharper than "a user with a key":

- Their MCP servers and models are often **local** — an MCP server a coding agent
  launched over stdio, a model on their workstation — so "where is the problem?"
  means their machine, rolter, or the upstream, and they need to tell which.
- They read **their own request bodies** to debug. A member of the project reads
  captured bodies by default (#1820); a viewer does not.

**Goal:** "I call models through rolter with my own key, and when a call fails I
can see why."

**Dogfood account:** `engineer@rolter.local`, member of project `default/default`
(the fleet's project). `engineer2@rolter.local` is a member of
`research/sandbox` and should see none of it. See
[dogfood data](../user-journeys.md#dogfood-data).

**The observer watches:** the time from first sign-in to the first successful call
(`ui_events` screen views → the first `request_logs` row on the engineer's key),
and every screen the engineer opens while debugging E5 — the order they look in
is the finding.

## E1 — arrive

| #    | step                       | where                          | expect                                                                                   | status   |
| ---- | -------------------------- | ------------------------------ | ---------------------------------------------------------------------------------------- | -------- |
| E1.1 | sign in                    | login screen (password or SSO) | the dashboard, scope `default/default`; admin-only controls disabled with the role named | verified |
| E1.2 | find out what models exist | **Models → Model Catalog**     | the project's routes: `gpt-4o`, `llama-3.1-8b`, … with their capabilities                | verified |

## E2 — get a key

| #    | step                                  | where                                                                                                | expect                                                     | status   |
| ---- | ------------------------------------- | ---------------------------------------------------------------------------------------------------- | ---------------------------------------------------------- | -------- |
| E2.1 | mint a personal key                   | **Settings → My Virtual Keys → Generate virtual key** · `POST /api/v1/me/projects/{id}/virtual-keys` | the key once, a name, an expiry; a viewer would be refused | verified |
| E2.2 | narrow it to the models the job needs | the mint form's model list                                                                           | `/v1/models` with that key lists only those                | verified |
| E2.3 | rotate or delete it                   | the key card · `POST /api/v1/me/virtual-keys/{id}/rotate`                                            | a new secret; the old one refused                          | verified |

## E3 — make the first call

| #    | step                                             | where                                                           | expect                                          | status   |
| ---- | ------------------------------------------------ | --------------------------------------------------------------- | ----------------------------------------------- | -------- |
| E3.1 | from the Playground, with no key handling at all | **Playground** (mints a 30-minute session key for the project)  | an answer, streamed                             | verified |
| E3.2 | from code, OpenAI dialect                        | OpenAI SDK with `base_url=<gateway>/v1`, `api_key=<key>`        | the same answer; `x-request-id` on the response | verified |
| E3.3 | from code, Anthropic dialect                     | Anthropic SDK with `base_url=<gateway>`, the key as `x-api-key` | the same model family through `/v1/messages`    | verified |
| E3.4 | streaming, embeddings                            | `stream: true`; `/v1/embeddings` on `text-embedding`            | SSE chunks; vectors                             | verified |

## E4 — point a coding agent at rolter

| #    | step                                 | where                                                                 | expect                                                                   | status |
| ---- | ------------------------------------ | --------------------------------------------------------------------- | ------------------------------------------------------------------------ | ------ |
| E4.1 | repoint Claude Code, Codex or Cursor | the third prompt in [agent setup](../../../user-docs/agent-setup.mdx) | the agent's own traffic appears in **LLM Logs** under the engineer's key | works  |

## E5 — "my calls fail: is it me, rolter, or the upstream?"

The debugging loop, in the order an engineer should be able to walk it.

| #    | step                                | where                                                                                                                                                      | expect                                                                                                                                                                                                                                                                                                                                                                                                                                                          | status                                       |
| ---- | ----------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------- |
| E5.1 | read the error the client got       | the response body                                                                                                                                          | OpenAI-style JSON whose `type` and `code` name the cause: 401 `authentication_error`; 403 `model_not_allowed` / `route_not_allowed` / `provider_not_allowed` / `required_header_missing`; 400 `guardrail_blocked` / `plugin_blocked`; 404 `model_not_found` (no such route); 402; 429 `rate_limit_exceeded` with `Retry-After`, or `queue_full` / `queue_timeout`; 503 `queue_dropped`; 502 `pii_sanitizer_unavailable`; other 5xx (no target, or the upstream) | verified (401, 404 here; 403 in P1.2)        |
| E5.2 | find the request                    | `GET /api/v1/analytics/invocations?request_id=<x-request-id>` (#1863); **LLM Logs**: the request-id field (#1861), or filtered by status = error and model | the row: status, target, provider, error text, latency, retries, and the captured bodies                                                                                                                                                                                                                                                                                                                                                                        | verified (#1861, #2400)                      |
| E5.3 | follow it into a trace              | the row's `trace_id`, copied from the **LLM Logs** drawer (#1983), in SigNoz                                                                               | where the time went: queue, upstream, stream                                                                                                                                                                                                                                                                                                                                                                                                                    | works                                        |
| E5.4 | check whether the upstream is sick  | **Circuit Breaker**, provider health                                                                                                                       | the provider behind the route: breaker state, uptime, recent failures                                                                                                                                                                                                                                                                                                                                                                                           | gap — #1833 (health needs an org-level role) |
| E5.5 | a call the client gave up on        | **LLM Logs**, status 499                                                                                                                                   | which target the client was waiting on                                                                                                                                                                                                                                                                                                                                                                                                                          | verified (P5.6 walks it)                     |
| E5.6 | a call that is slow only under load | concurrent calls to one route                                                                                                                              | latency stays flat as concurrency rises                                                                                                                                                                                                                                                                                                                                                                                                                         | verified (P5.2 walks it)                     |

## E6 — "my MCP tool call failed"

| #    | step                                   | where                                       | expect                                         | status                  |
| ---- | -------------------------------------- | ------------------------------------------- | ---------------------------------------------- | ----------------------- |
| E6.1 | connect an MCP server that needs OAuth | **MCP Gateway → MCP Catalog**, then consent | a session of their own under **Auth Sessions** | works                   |
| E6.2 | call a tool through the gateway        | the agent, via `<gateway>/mcp/{server}`     | the tool answers                               | works                   |
| E6.3 | see the failed tool call               | **Observability → MCP Logs**                | the engineer's own tool-call rows              | verified (#1831, #2397) |
| E6.4 | a session expired                      | **Auth Sessions**                           | the session's state and a way to renew it      | works                   |

## E7 — "my local MCP server misbehaves"

| #    | step                                                      | where | expect                                                                                 | status      |
| ---- | --------------------------------------------------------- | ----- | -------------------------------------------------------------------------------------- | ----------- |
| E7.1 | see the raw JSON-RPC between the agent and a stdio server | —     | every request, response and notification, live — the job tools like mcp-snoop do today | gap — #1832 |

## E8 — "try my local model through rolter"

| #    | step                                                      | where                               | expect                                                                                          | status                            |
| ---- | --------------------------------------------------------- | ----------------------------------- | ----------------------------------------------------------------------------------------------- | --------------------------------- |
| E8.1 | point rolter at `http://localhost:11434` for personal use | —                                   | a route only the engineer's keys reach                                                          | gap — #1839 (an idea, not a plan) |
| E8.2 | the workaround                                            | `rolter easy-up` on the workstation | a personal gateway with the same API and logs, pointed at the local model through `rolter.toml` | works                             |

## E9 — "what have I spent?"

| #    | step                             | where                                               | expect                              | status      |
| ---- | -------------------------------- | --------------------------------------------------- | ----------------------------------- | ----------- |
| E9.1 | usage per key over the last week | **Settings → My Virtual Keys** (`/api/v1/me/usage`) | requests and cost per key           | verified    |
| E9.2 | how much of my allowance is left | —                                                   | a personal budget and its remainder | gap — #1830 |

## E10 — make the dashboard mine

| #     | step                                        | where                                                                 | expect                                                                                                             | status                                 |
| ----- | ------------------------------------------- | --------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------ | -------------------------------------- |
| E10.1 | set a display name and a short bio          | **Settings → My Virtual Keys → Profile** · `PATCH /api/v1/me/profile` | other people see the name instead of an e-mail                                                                     | verified (#1823, #2433; V2.1 walks it) |
| E10.2 | language, default project, Playground model | **Settings → Preferences**; the language picker; the scope switcher   | kept on the server, so a new browser opens on the same language, scope and model — and on the same chart time zone | done (#2448)                           |
| E10.3 | save "my errors this week" as a view        | **LLM Logs → Saved views** · `POST /api/v1/me/saved-views`            | a named filter on LLM Logs                                                                                         | verified (#1825, #2451; V5.1 walks it) |
| E10.4 | protect the account with a second factor    | **Settings → My Virtual Keys → Two-factor authentication**            | TOTP and recovery codes                                                                                            | verified                               |
