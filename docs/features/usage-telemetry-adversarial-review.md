# Usage Analytics Adversarial Review

Date: 2026-08-23  
Scope: [`docs/analytics.md`](../analytics.md) and
[`usage-telemetry.md`](usage-telemetry.md)

> **Historical record.** This review covers the original daily-aggregation,
> Statsig-server-SDK-or-relay design. A 2026-09-19 revision to
> [`usage-telemetry.md`](usage-telemetry.md#revision-2026-09-19-client-key-per-event-ingestion)
> switched to per-event sending with a Statsig Client SDK Key and a rotatable
> Install ID, which changes several findings below (notably "per-command
> events created a local and remote same-day activity history" and the
> Statsig credential concerns). The findings are preserved as written because
> they document what a past review actually found; read them alongside the
> revision note, not as the current design.

Three independent reviews examined the initial proposal:

1. a privacy/security and data-protection review, including privacy-sensitive
   OSS developer, EU privacy officer, and security-researcher personas;
2. a Rust CLI/distributed-systems/observability review, including offline,
   slow-network, Windows, CI/packager, and enterprise OTel personas; and
3. an open-source product/community review, including new-user, skeptical
   contributor, Homebrew packager, accessibility/noninteractive, and maintainer
   personas.

## Blocking findings

The initial proposal was not acceptable to ship because:

- it called a daily identifier and provider-visible requests “anonymous” even
  though they remained pseudonymous and potentially fingerprintable;
- merely showing a notice appeared to enable later collection, while the state
  machine never defined that transition consistently;
- per-command events created a local and remote same-day activity history;
- Statsig privacy, retention, credential, profile, enrichment, and billing
  assumptions were not verified;
- a public ingestion key allowed spoofing, data poisoning, and quota attacks;
- opt-out could race a detached uploader and later transmit queued data;
- local spool permissions, symlink/reparse protection, expiry, crash recovery,
  and cross-platform paths were underspecified;
- CI could inherit consent from an unrelated interactive context;
- the metrics overstated activation, reliability, and end-to-end latency;
- proposed OTel names/units did not follow common conventions, and short-lived
  export lifecycle, TLS/proxy behavior, and credential handling were unresolved.

## Decisions made after review

| Initial proposal | Revised decision |
|---|---|
| Default-on after disclosure | Explicit `wsp analytics enable`; notice is not consent |
| “Anonymous users” | Pseudonymous reports from reporting installations |
| Per-command events | Client-side UTC-day counters and histograms |
| Daily secret-derived identifier | Random per-report ID used only for retry/deduplication |
| Invocation timestamp | UTC date only |
| CI follows ordinary consent | Separate `analytics.include-ci`, default false |
| Config-only control | Status, preview, enable, disable, and purge commands |
| Statsig selected outright | Statsig preferred but blocked on a dated provider ADR |
| Provider endpoint enforces schema | Direct ingestion is explicitly untrusted, or use a validating relay |
| Vague local spool | Owner-only, platform-native, bounded seven-day aggregate store |
| Best-effort disable | Consent generation/tombstone and pre-connect recheck |
| `wsp.command` / milliseconds | `wsp.command.invocations` and `wsp.command.duration` in seconds |
| OTel config with token interpolation | Standard OTel environment conventions and secret redaction |
| Activation/reliability claims | Explicit measurement limitations and sampling caveats |

## Residual risks requiring implementation decisions

The revision intentionally leaves these as launch blockers rather than hiding
them behind optimistic prose:

- whether Statsig can prevent profiles/enrichment and enforce ≤90-day retention;
- direct unauthenticated ingestion versus an operated validating relay;
- provider region, IP handling, legal terms, subprocessors, and hard cost cap;
- exact cross-platform safe-file and detached-process implementation;
- proxy, native trust store, enterprise CA, redirect, and retry behavior;
- whether a short-lived OTel flush can meet both delivery and latency budgets;
- the complete technical/legal privacy notice and project controller contact.

## Persona outcomes after revision

- **Privacy-sensitive developer:** collection is inspectable and explicitly
  opt-in; no command history or cross-day identifier is planned.
- **Enterprise privacy officer:** collection remains unapproved until the
  provider ADR, legal notice, processing region, retention, and CI fleet controls
  are complete.
- **Security researcher:** public ingestion remains forgeable by design unless a
  relay is chosen; dashboards must treat reports as advisory.
- **Offline/slow-network user:** analytics performs no foreground network work;
  expiry and bounded retries prevent indefinite queue growth, subject to
  cross-platform implementation tests.
- **Windows user:** Known Folders, ACLs, reparse points, handle inheritance, and
  Job Objects are now explicit acceptance criteria.
- **Packager/CI administrator:** analytics can be compiled out, CI is separately
  opted in, and effective state is queryable.
- **Enterprise OTel operator:** standard metric semantics and environment
  conventions are selected, but exporter lifecycle/TLS/proxy details remain a
  stability gate.
- **WSP maintainer:** reports can guide reversible prioritization but cannot
  support exact-user, retention, crash, security, or removal conclusions.

## Review conclusion

The original draft should not ship. The revised proposal has a defensible
direction but remains a design, not authorization to collect data. Every open
launch gate in the revised design must be resolved and independently reviewed
before stable analytics can be enabled.
