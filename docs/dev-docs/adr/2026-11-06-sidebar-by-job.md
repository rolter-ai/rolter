# The dashboard sidebar is grouped by job

**Status:** Accepted (6 Nov 2026) · **Date:** 6 Nov 2026 · **Issues:** [#3656](https://github.com/rolter-ai/rolter/issues/3656), [#3640](https://github.com/rolter-ai/rolter/issues/3640)

## Context

The rail held 58 screens under 14 top-level entries, grouped by the subsystem
that implements each screen. The second live review of the dashboard (#3640)
found what that costs a visitor:

- Budgets and limits, Provider health and Model prices sat under Models.
  Connectors, Log settings and Log erasure sat under Observability.
- Seven entries were some kind of "Settings": Logs Settings, Model Settings,
  MCP Settings, Client Settings, the adaptive router's Settings, the account's
  Preferences, and the Settings group itself.
- Account & keys and Preferences appeared both in the Settings group and in
  the user menu.
- "Providers" named both the model providers and the guardrail providers.
  "Channels", "History" and "Policies" only made sense inside their group,
  which the folded rail, the command palette and a screen reader do not say.
- Model Catalog and Routing Rules both list routes and both add one, as a
  table and as cards. Adaptive and complexity routing lived apart from the
  rest of routing, and the prompt and skills repositories were top level.

`PRODUCT.md` names the jobs people bring: an operator mid-incident wants the
fleet's state in one look, an admin manages access and money, and a developer
tries models and reads their own logs.

## Decision

The rail has seven groups, each named for a job, and nothing at the top level
beside them. `NAV` in `ui/src/lib/nav.tsx` is the record; the order is the
rail's.

| Group         | Screens                                                                                                                                                                           |
| ------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Observability | Dashboard, LLM Logs, MCP Logs, Alert Rules, Alert Channels, Alert History, Cluster Config                                                                                         |
| Traffic       | Playground, Model Catalog, Routing Rules, Model Providers, Provider Groups, Complexity Router, Adaptive Routing, Adaptive Policy, Provider Health                                 |
| Spend         | Budgets & Limits, Model Prices, Savings, Business Units, Customers                                                                                                                |
| Access        | Virtual Keys, Users, Teams, Access Profiles, Roles & Permissions, API Tokens, Single Sign-On, LDAP, User Provisioning, Audit Logs                                                 |
| Safety        | Guardrail Rules, Guardrail Policies, Guardrail Providers, Log Erasure                                                                                                             |
| MCP Gateway   | MCP Catalog, MCP Library, Tool Groups, Auth Sessions, OAuth Grants, MCP Settings                                                                                                  |
| Platform      | Client Settings, Compatibility, Performance Tuning, Security, Feature Flags, Effective config, Logging, Model Defaults, Connectors, Plugins, Prompt Repository, Skills Repository |

- **The account's own screens live in the user menu only.** Account & keys and
  Preferences are `ACCOUNT_SCREENS`, routed like any leaf and found by the
  command palette, but not in the rail, which is about the deployment.
- **A leaf's label says what it is on its own.** Channels, History and Policies
  became Alert Channels, Alert History and Guardrail Policies. The guardrail
  Providers became Guardrail Providers. The adaptive router's Telemetry and
  Settings became Adaptive Routing and Adaptive Policy. The rail label and the
  screen header agree.
- **Fewer "Settings".** Logs Settings became Logging and Model Settings became
  Model Defaults. The Settings group is gone. MCP Settings and Client Settings
  keep the word, since their groups and nouns already say whose settings they
  are.
- **Business units and customers sit under Spend.** They are the dimensions a
  key's spend is attributed to, and their screens are read for cost. **Plugins
  sit under Platform:** they are gateway pipeline plugins, not MCP.
- **Routes get one home.** Model Catalog and Routing Rules stay two entries in
  this change, side by side under Traffic. Folding Routing Rules into the
  catalog (its cards, per-route traffic and target health as a view of the
  catalog's routes, one Add route) is a screen redesign of its own, tracked
  in [#3685](https://github.com/rolter-ai/rolter/issues/3685).

URLs do not change. A leaf's key is its path (`/<key>`) and no key moved, so
bookmarks, links from other screens and the documentation's deep links keep
working. A group's key only names its label.

## Consequences

- Documentation that spells out a rail path (`Governance → Users`) names the
  new group (`Access → Users`). The account's screens are reached from the user
  menu.
- `ui/src/lib/nav.test.ts` holds two new invariants: no account screen is a
  rail leaf, and every top-level entry is a group of leaves, two levels deep.
- A role that reads nothing in a group loses the group, as before (#1183): a
  viewer or an admin has no Safety group, since guardrails and log erasure are
  deployment-scoped.
- The command palette hints each screen with its new group's name.

## Rejected

- **Keeping the subsystem groups and renaming the ambiguous leaves.** It fixes
  the labels and leaves budgets under Models and personal screens in the rail.
- **A flat, searchable list of every screen.** The palette and the rail's
  search already serve someone who knows the name; the groups are for someone
  who knows the job.
