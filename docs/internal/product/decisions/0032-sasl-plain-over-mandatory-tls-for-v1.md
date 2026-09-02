# 0032. `SASL/PLAIN`, mandatorily over TLS, is the one v1 mechanism

Status: accepted
Date: 2026-09-02
Requirements: FR-4, FR-40, NFR-12

## Context

`M9.md`'s own "Decisions required first" table names "SASL mechanisms for v1"
and points at doc 15 §9 for background — but doc 15 §9 only establishes that
tenant identity comes from SASL credentials at all ("shared bootstrap
endpoint, tenant identified by SASL credentials"), never which mechanism.
Grepping the whole research corpus for `sasl|scram` finds no mechanism-level
research anywhere: doc 02 §1.4/§1.7 covers only wire framing
(`SaslHandshakeRequest`/`SaslAuthenticateRequest`, keys 17/36, and that
`SaslHandshakeRequest` is permanently non-flexible) and which optional APIs a
given mechanism would pull in — nothing about which mechanism a v1 broker
should actually implement.

`M9.md` task 3 asks for "at least one real mechanism end to end, with
credentials from configuration" — singular, deliberately. Tenancy in this
design falls out of authentication (doc 15 §9), so this decision is not
cosmetic: it sets the shape every client's connection string takes and what a
tenant's credential-provisioning story looks like for v1.

## Decision

**`SASL/PLAIN`, and only `SASL/PLAIN`, for v1 — refused outright unless the
connection is already TLS-terminated** (`M9` task 4, `ADR-0012`'s `ring`
default build). A principal authenticates with a username/password pair
issued out of band; the broker never accepts a `SaslAuthenticate` exchange on
a connection that has not completed a TLS handshake first, so the one
security property PLAIN itself lacks — credential confidentiality in
transit — is supplied by the layer immediately below it rather than by the
mechanism.

Every Kafka client library in wide use (librdkafka, kafka-java, Sarama,
franz-go, confluent-kafka) supports `SASL/PLAIN` natively with no additional
dependency, which matters for a broker whose whole value proposition is
protocol compatibility — a v1 mechanism a real client cannot reach without
extra tooling is not really shipped.

## Alternatives considered

**`SCRAM-SHA-256`/`SCRAM-SHA-512`**, rejected for v1: a real security
improvement (no cleartext credential ever crosses the wire, salted
challenge-response resists a passive capture even without TLS), but it needs
a credential store shaped around a salt/iteration-count/stored-key/server-key
tuple rather than a plain secret, and doc 02 §1.7 confirms it pulls in two
more wire APIs no other mechanism needs
(`DescribeUserScramCredentials`/`AlterUserScramCredentials`, 50–51). TLS
already closes SCRAM's one advantage over PLAIN here, since `M9` task 4 makes
TLS mandatory regardless of mechanism. Worth reconsidering once credential
rotation or an offline-attack threat model is in scope — SCRAM's real
advantage is resisting a compromised broker's credential store, not the wire.

**`OAUTHBEARER`**, rejected for v1: requires a token-issuing identity
provider this project has neither built nor integrated with, and Kafka's own
reference implementation historically shipped an unsecured default
`OAuthBearerUnsecuredLoginCallbackHandler` precisely because most deployments
never wire a real IdP — the mechanism's whole value is moot without one. A
real candidate once there is an actual IdP story (SSO, an admin console),
neither of which exists before `M12`.

**mTLS (client-certificate authentication)**, rejected for v1: needs a
certificate-issuance and rotation pipeline — a CA hierarchy, per-tenant cert
provisioning, revocation — that is a materially different, larger, and
entirely unresearched question from "shared bootstrap, tenant identified by
SASL credentials" (doc 15 §9's own working hypothesis, which this decision
keeps rather than reopens). Combining it with the shared-bootstrap model
doc 15 assumes would mean deciding that model over again first.

**GSSAPI/Kerberos**, rejected outright: an on-premises, KDC-dependent
mechanism aimed at enterprise Active Directory environments — the wrong fit
for a cloud-native, self-serve multi-tenant broker, and nothing in this
project's mission or requirements calls for it.

## Consequences

**Easy**: every real client library works against v1 with no extra
configuration beyond a username and password; the credential model is a flat
secret per principal, which keeps `oqueue-core`'s `Principal` type (`M9` task
1) simple — no salt, no iteration count, no per-mechanism variant to carry.

**Hard**: PLAIN's credential confidentiality is entirely borrowed from TLS,
so refusing `SaslAuthenticate` on a non-TLS connection is load-bearing
security, not an optional hardening step — `M9`'s test suite must assert this
refusal explicitly (FR-44's log-scan gate does not catch a credential that
never should have crossed the wire in the first place; that is a distinct,
earlier check).

**Forecloses nothing FR-4/FR-40 need**: adding SCRAM or OAUTHBEARER later is
additive — `SaslHandshakeRequest`'s mechanism-negotiation shape exists
precisely so a broker can advertise more than one and a client picks. This
ADR picks the first one, not the only one ever.
