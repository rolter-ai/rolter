# Security Policy

## Supported versions

rolter is in early development (pre-1.0). Security fixes are applied to the
latest `master` and the most recent tagged release. Pin a released version for
production and upgrade promptly when advisories are published.

## Reporting a vulnerability

**Please do not open public issues for security vulnerabilities.**

Report privately via one of:

- **GitHub Security Advisories** (preferred): open a private report at
  <https://github.com/ormeilu/rolter/security/advisories/new>.
- **Email**: lubenets.ilya.igorevich@gmail.com — include "rolter security" in
  the subject.

Please include:

- A description of the issue and its impact.
- Steps to reproduce (proof-of-concept, affected endpoint/config if relevant).
- Affected version/commit and your environment.

### What to expect

- Acknowledgement within **72 hours**.
- An initial assessment and severity within **7 days**.
- Coordinated disclosure: we'll agree on a timeline and credit you in the
  advisory unless you prefer to remain anonymous.

## Scope

In scope: the gateway (data plane), control plane, and their handling of
secrets, virtual keys, auth/RBAC, and upstream forwarding. Out of scope:
vulnerabilities in third-party providers/upstreams, and issues requiring
privileged local access already granted by the operator.

## Hardening & operational guidance

See [`docs/architecture/security.md`](docs/architecture/security.md) for the
threat model, secret handling (envelope encryption, `ROLTER_MASTER_KEY`),
transport, and tenant isolation. Key reminders:

- Set a strong `ROLTER_MASTER_KEY` (`openssl rand -hex 32`) and rotate provider
  keys periodically.
- Prefer `api_key_env` over inline `api_key` in the bootstrap config.
- Run the control plane on a private network; expose only the gateway publicly.
- Terminate TLS at the gateway or a fronting ingress.
