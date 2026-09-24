# SecOps

**Security engineering: reviews and hardens, rarely changes routing.** Wants to
prove, to themselves and to an auditor, who can read what, that secrets stay out
of the logs, and that a leaver's access ends. Today that needs **superadmin**,
because every setting they review is deployment-wide and nothing narrower
reaches it — which also hands them every tenant's traffic.

**Goal:** "I can show who reads prompts, that credentials never land in a log or
a response, and that access ends when it should — without being able to change
the routing I am reviewing."

**Dogfood account:** `dev@rolter.local` (superadmin), plus the other personas to
test what _they_ can read. See [dogfood data](../user-journeys.md#dogfood-data).

**The observer watches:** `audit_log` — every step here either reads the audit
trail or should leave a row in it.

## S0 — the role

| #    | step                                             | where | expect                                                                         | status      |
| ---- | ------------------------------------------------ | ----- | ------------------------------------------------------------------------------ | ----------- |
| S0.1 | review security posture without being superadmin | —     | a read-only security-auditor role: settings, guardrails, audit, no tenant data | gap — #1834 |

## S1 — the perimeter

| #    | step                                            | where                                                                       | expect                                                                                          | status          |
| ---- | ----------------------------------------------- | --------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------- | --------------- |
| S1.1 | confirm nothing runs open                       | `rolter check`; the dashboard banner                                        | an admin token set; open mode refused on any non-loopback bind                                  | works           |
| S1.2 | confirm an anonymous caller gets nothing        | the #1820 sweep, or `curl` every GET in `/openapi.json` without credentials | 401 from everything the document does not mark public                                           | verified        |
| S1.3 | decide whether the redacted config stays public | `GET /api/v1/config` without credentials                                    | no secrets, but every upstream `api_base`, route and group                                      | partial — #1840 |
| S1.4 | restrict browser origins and ingress            | **Settings → Security**; CORS                                               | the dashboard's origin allowed, nothing else                                                    | works           |
| S1.5 | bound outbound requests                         | the egress policy                                                           | a provider, connector or MCP server cannot be pointed at an internal address the policy forbids | works           |

## S2 — identity

| #    | step                                        | where                                                | expect                                                                      | status      |
| ---- | ------------------------------------------- | ---------------------------------------------------- | --------------------------------------------------------------------------- | ----------- |
| S2.1 | single sign-on only, second factor required | **Governance → Single Sign-On → Org sign-in policy** | one button; accounts without a factor enrol at next sign-in                 | works       |
| S2.2 | throttle password guessing                  | repeated wrong passwords for one account             | HTTP 429 with `Retry-After` once the account's allowance is spent; audited  | works       |
| S2.3 | a leaver loses everything                   | SCIM deprovision (platform-admin A1-d)               | sessions gone at once; personal keys refused at the gateway                 | bug — #1841 |
| S2.4 | break-glass for a lost second factor        | `rolter mfa reset --email … --reason …` on the host  | factor and sessions cleared, the reason audited; not reachable over the API | works       |

## S3 — prove secrets stay out of the logs

| #    | step                                             | where                                         | expect                                                                                                       | status           |
| ---- | ------------------------------------------------ | --------------------------------------------- | ------------------------------------------------------------------------------------------------------------ | ---------------- |
| S3.1 | payload capture is off unless someone chose it   | **Observability → Logs Settings**             | off by default; the dogfood profile turns it on deliberately                                                 | verified         |
| S3.2 | known credential fields never reach storage      | `payload_capture_redact_fields`               | the value of a matching JSON key replaced before the write, recursively                                      | works            |
| S3.3 | a key pasted into a prompt never reaches storage | —                                             | a secret inside free text redacted by pattern                                                                | gap — #1835      |
| S3.4 | see what the redaction does before trusting it   | —                                             | a dry run against a sample body                                                                              | gap — #1835      |
| S3.5 | who reads the bodies that are stored             | sign in as each persona; **LLM Logs** → a row | members and admins of the project read them; viewers get "hidden for your role" unless the project allows it | verified (#1820) |
| S3.6 | erase one subject's logs on request              | —                                             | a subject's rows and payloads deleted, audited                                                               | gap — #1085      |

## S4 — inspection in the request path

| #    | step                                                   | where                  | expect                                                                               | status |
| ---- | ------------------------------------------------------ | ---------------------- | ------------------------------------------------------------------------------------ | ------ |
| S4.1 | block a known-bad pattern before it reaches a provider | **Guardrails → Rules** | the request refused with `guardrail_blocked: <rule>`; the rejected text never stored | works  |
| S4.2 | hand content to an external de-identification service  | the PII sanitizer      | the provider sees the substituted text; optionally reversed on the way back          | works  |

## S5 — keys and recovery

| #    | step                                               | where                                                        | expect                                                                         | status |
| ---- | -------------------------------------------------- | ------------------------------------------------------------ | ------------------------------------------------------------------------------ | ------ |
| S5.1 | verify a restored database still opens its secrets | `rolter kek verify`                                          | every sealed column opens with the current KEK, or the report names what fails | works  |
| S5.2 | rotate the KEK without splitting the fleet         | [backup and restore](../../deployment/backup-and-restore.md) | the documented rotation: no node is left unable to open a secret mid-way       | works  |

## S6 — the trail

| #    | step                                | where                                          | expect                                                   | status   |
| ---- | ----------------------------------- | ---------------------------------------------- | -------------------------------------------------------- | -------- |
| S6.1 | who changed what, when              | **Governance → Audit Logs** (admin at the org) | every mutation in this session's scripts, with its actor | works    |
| S6.2 | a project was opened to its viewers | the audit log, `project.settings.update`       | the row, naming the value it was set to                  | verified |
