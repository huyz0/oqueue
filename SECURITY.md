# Security Policy

## Supported versions

None yet. There is no released version and no implementation — see the status
notice in [README.md](README.md). This policy exists so that the reporting route
is in place before there is anything to report.

## Reporting a vulnerability

Use GitHub's private vulnerability reporting:
**[Report a vulnerability](https://github.com/huyz0/oqueue/security/advisories/new)**

Please do not open a public issue for a suspected vulnerability.

Include what you would want to receive: what you did, what happened, what you
expected, and the smallest reproduction you have. A proof of concept is helpful
but not required — a clear description of the mechanism is enough to start.

This is a single-maintainer project, so expect an acknowledgement within a week
rather than within a day.

## Scope, once there is code

The parts of a broker like this that warrant the most scrutiny, and where
reports are most valuable:

- **The wire protocol decoder.** It parses attacker-controlled, length-prefixed
  bytes from any client that can reach the port. Anything sized by a
  client-supplied number needs a bound, and a malformed frame must never take
  down a node serving other connections.
- **Credential handling.** Nothing that resolves a secret may reach a log, span,
  metric label, error variant, or admin response.
- **Multi-tenant isolation.** A tenant must not be able to observe, address, or
  exhaust another tenant's resources — including through metadata responses and
  through shared object-storage prefixes.
- **`unsafe` code.** Confined by policy to three crates (`oqueue-buf`,
  `oqueue-codec`, `oqueue-checksum`). Soundness issues there, or any route by
  which safe API misuse reaches them, are the highest-severity class.

## Out of scope

- Denial of service achieved by simply sending more traffic than the deployment
  is provisioned for.
- Vulnerabilities in dependencies that have no exploitable path through oqueue.
  Report those upstream; a note here is still welcome if we should pin around it.
- Anything requiring an attacker who already has filesystem or credential access
  to the broker host.
