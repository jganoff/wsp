# Analytics and Telemetry

> **Status:** This page describes a proposal. Released versions of WSP do not
> currently send analytics or telemetry. WSP will update this notice before any
> collection begins.

> For the short, plain-language version of this page, see
> [`docs/telemetry.md`](telemetry.md). This page is the deeper technical
> account underneath it.

WSP plans to support two independent kinds of measurement:

1. **WSP analytics**: privacy-preserving, pseudonymous usage and health reports
   sent to the WSP project, enabled by default and controlled the same way as
   any other WSP setting.
2. **Your telemetry**: operational metrics sent to an OpenTelemetry collector
   you configure and control. This is opt-in and is never sent to WSP.

The settings and destinations are separate. Enabling, disabling, or configuring
one never changes the other.

## WSP analytics

### Why we want it

Analytics can help the project understand:

- which commands and features reporting installations use, and how many
  distinct reporting installations use them;
- which WSP versions, operating systems, architectures, and installation
  channels are represented;
- which completed commands report success or an ordinary failure;
- which completed commands are slow; and
- typical workspace sizes, expressed only as broad ranges.

This data cannot measure every WSP user, retention, crashes, or complete setup
funnels. People who do not opt in, block uploads, use telemetry-free packages,
or use older versions are absent. Fatal termination can prevent an event from
being sent. Reports and dashboards will describe "reporting installations,"
not all users.

Analytics is only one input into project decisions. WSP will never use it for
security decisions, individual evaluation, or as proof that a feature is safe
to remove.

### Enabled by default

WSP analytics will be on by default, matching Codex, Homebrew, and most peer
CLI dev tools rather than requiring an opt-in step.

An interactive WSP invocation shows a short, plain-text notice once, the
first time it's eligible, explaining analytics and how to turn it off.
Showing the notice doesn't gate anything — collection is already on by
default from the first command; the notice is disclosure, not consent.
Analytics is controlled through the same setting mechanism as everything
else in WSP:

```sh
wsp config set analytics.enabled false   # turn off
wsp config set analytics.enabled true    # turn back on (issues a fresh Install ID)
```

The notice will only appear when WSP is running at an interactive terminal —
never in JSON output, shell completion, or other non-interactive use, the
same way WSP already decides whether to show any other interactive prompt.
That's about where the notice can usefully be printed, not about consent:
**there is no separate rule for CI.** `analytics.enabled` applies the same
way everywhere, whether at a terminal or in a build pipeline — WSP does not
attempt to detect "is this CI," and there is no second setting for it.

Consent is scoped to the current OS user's WSP data directory. Packagers can
compile analytics out entirely; packages built that way cannot enable or send
WSP analytics.

### Per-command events

Unlike an earlier draft of this proposal, WSP will send one small event per
eligible completed command rather than a daily aggregate. This is a deliberate
trade: it is what makes "which reporting installations use what" answerable at
all, and it removes an entire class of local queue/rotation engineering that a
daily-aggregate design required. In exchange, WSP accepts a random, rotatable,
per-install identifier (below) instead of true day-level anonymity.

Each event may contain only:

| Field | Values |
|---|---|
| Install ID | A random ID generated when analytics is enabled; rotated every time it's turned back on after being off; never a hardware, advertising, or stable device identifier |
| Command | A fixed name such as `st`, `new`, or `repo/fetch`; aliases are normalized |
| Outcome | `success` or `observed_failure`; fatal and unobservable failures are not counted |
| Failure category | Only when outcome failed: a broad category (network, conflict, permission, invalid config, not found, other) — never an error message, code, or location |
| Performance | A duration bucket such as 100–249 ms or 1–3 seconds, not an exact duration |
| WSP build | The exact released version (its own number already distinguishes a stable release from a release candidate), plus whether this binary was built from an exact tagged release or not |
| Platform | Operating-system family and CPU architecture |
| Installation | A small fixed set of known install channels (Homebrew, cargo, a GitHub release, ...), otherwise `unknown` — never a raw string passed through from how a package was built |
| Execution context | Interactive or JSON — the same TTY-based signal WSP already uses elsewhere (prompts, color output), included so CI-fleet and scripted traffic can be told apart from a human at a terminal without a separate opt-in or a dedicated CI detector |
| Likely agent-driven | A plain yes/no: whether wsp detects it's running in its own isolated sandbox mode built specifically for AI agents. No environment-marker guessing — only this one real, already-existing signal. Necessarily undercounts agent use outside that mode; never presented as a complete count of agent traffic |
| Shell | A coarse bucket (zsh/bash/fish/powershell/other/unknown) — never the raw shell path or name |
| Size | A broad repository-count bucket such as 3–5 or 11–25, for relevant commands only |
| Feature use | A fixed, documented WSP feature name — for `wsp new`, which of the six real creation paths was used (template, derived from another workspace, from a file, empty, built from listed repos, or copied automatically from the current workspace); never a user-provided value |
| `wsp doctor` result | Which specific check(s) found a warning or error, from doctor's own fixed, small set of checks — not just a count — only on that command, as found before any `--fix` |

There is no invocation timestamp beyond what the receiving service itself
records on arrival, and no per-invocation event history is kept locally after
it is sent. The Install ID is the only thing that lets two events be linked to
the same reporting installation; it identifies an installation, never a
person, and is deleted the moment analytics is turned off.

This is still **pseudonymous telemetry**, not anonymity. A persistent
per-install identifier, by design, lets the project count distinct reporting
installations and see their command history for as long as analytics stays
enabled on that install. WSP will suppress rare public breakdowns and publish
only aggregate cells meeting a minimum-size threshold.

### What WSP analytics never transmits

Beyond the Install ID, WSP analytics will never transmit:

- command arguments or option values;
- workspace, template, registry, repository, remote, organization, branch, or
  user names;
- paths, URLs, current directories, executable locations, or home directories;
- file contents, Git contents, commits, diffs, setup commands, descriptions,
  AGENTS.md files, prompts, or generated text;
- stdout, stderr, Git output, command output, or JSON output;
- error messages, stack traces, panic details, signals, or crash dumps;
- environment variable names or values, including any AI-agent/harness marker
  names or values used to derive the "likely agent-driven" signal above;
- usernames, email addresses, hostnames, locale, timezone, the exact shell
  path/name (a coarse zsh/bash/fish/powershell/other bucket is sent instead,
  see above), or terminal name; or
- hardware, advertising, or stable device identifiers.

WSP locally reads only a documented fixed set of environment variables to
honor opt-outs. It never transmits their names or values. The analytics
event is built from typed fields and fixed enums, not a
map of arbitrary properties, and is sent as a plain HTTP request WSP
constructs itself — no vendor SDK runs inside WSP, so there is no
SDK-injected automatic context to disable.

### Local files

WSP stores only a small state file next to its other settings, at
`~/.local/share/wsp/analytics.yaml` on a typical Linux system (locations
differ on macOS and Windows) — the same directory as `config.yaml`, not a
dedicated subdirectory. It holds the current Install ID and whether the
first-run notice has been shown — nothing resembling a command history.

There is no local event queue: a foreground command never performs analytics
network I/O, and an event that cannot be sent within its short send window is
dropped rather than retried or persisted.

### Controlling and inspecting analytics

There is no dedicated `analytics` command. It's controlled the same way as
any other WSP setting:

```sh
wsp config set analytics.enabled false   # turn off; deletes the Install ID
wsp config set analytics.enabled true    # turn back on; issues a fresh Install ID
wsp config get analytics.enabled         # is it on right now
wsp doctor                                # includes an analytics line: on/off, install ID present
```

Turning analytics off deletes the local Install ID immediately. An HTTPS
request that already started cannot be recalled, and an already-sent event
cannot be selected for individual deletion after the fact from WSP's side —
the documentation says so plainly rather than implying otherwise.

For temporary or centrally managed opt-out, either variable disables new
collection and sending when set to a value other than `0`:

```sh
export WSP_NO_ANALYTICS=1
export DO_NOT_TRACK=1
```

Environment opt-out takes precedence over all configuration and never sends
an event while active.

### Provider: PostHog, via a public project key

[PostHog](https://posthog.com/) is the provider. WSP initially evaluated and
documented Statsig (following OpenAI Codex's precedent) and switched after a
hands-on comparison of what each vendor's *free tier* actually lets a
maintainer do with the data — see
[`features/usage-telemetry.md`](features/usage-telemetry.md) and
[`features/usage-telemetry-questions.md`](features/usage-telemetry-questions.md)
for the full reasoning. In short: PostHog's SQL-based exploration tool
(HogQL) is confirmed included free, with no small fixed limit on how many
fields a report can be broken down by; Statsig's free tier left real
ambiguity about whether that same depth of analysis was behind a paid
upgrade. PostHog also documents a concrete way to tell it not to derive
location data from an event's network address — see below — which Statsig's
design could only flag as an open question for its own provider, not solve.

**WSP does not embed PostHog's "Personal API Key"** (`phx_...`), which
PostHog's own documentation describes as granting full account access and
says must never be exposed. WSP embeds only the **Project API Key**
(`phc_...`), which PostHog documents as "intentionally public and safe to
include in frontend bundles... a write-only key that can only create new
events."

- WSP embeds a PostHog **Project API Key** directly in the binary. This is
  the key type PostHog documents as intended for public exposure; it is not
  a secret, and its exposure is not a security incident.
- WSP sends events with a plain, unauthenticated-by-WSP HTTP POST to
  PostHog's public capture endpoint — no SDK dependency, no ingestion relay.
- No credential in the binary can be used for anything beyond writing events
  into WSP's PostHog project. It cannot read data back or access any other
  PostHog project.
- **Every event sets `$geoip_disable: true`.** This is a PostHog-documented
  property that tells its ingestion pipeline not to derive or store location
  data from the request's network address. It doesn't change what PostHog's
  servers technically receive a connection from, but it does stop that from
  becoming stored, queryable location data tied to the install ID.

Because the key is public, any request "from WSP" is unauthenticated evidence,
not proof of anything: it can be replayed, forged, or sent with fabricated
field values by anyone who extracts it from the binary or observes the wire
format. WSP will:

- hard-disable paid overage / set a hard spend cap on the PostHog project —
  this matters more than it might otherwise, since PostHog's capture endpoint
  is documented to accept events and return success even when a project is
  over its billing quota, rather than rejecting them;
- treat all incoming data as advisory, never load-bearing for security or
  irreversible decisions;
- validate outgoing events against the fixed enums above before sending
  (this protects WSP's own dashboards from WSP's own bugs, not from a
  deliberate forger); and
- monitor for anomalous volume or field combinations.

**If abuse, spoofing, or free-tier limits become a real problem**, the
documented escalation path is a small relay WSP operates: a stateless proxy
validating request shape and rate before forwarding to PostHog. This is more
capable but reintroduces an operated service with its own availability,
logging, and security obligations — deliberately deferred until the simpler
project-key approach proves insufficient.

Before launch WSP must still publish a dated provider review confirming:

- exact plan limits, hard no-overage controls, and shutdown procedure;
- processing region, data-processing terms, subprocessors, and transfer basis;
- **confirmation that `$geoip_disable: true` actually works for events sent
  via raw HTTP capture**, not only through an official SDK — this is
  documented behavior, not yet independently verified by WSP;
- every other field stored or derived by PostHog on receipt;
- whether PostHog's data-deletion tools can remove all events tied to a given
  install ID on request;
- an enforceable raw-event retention limit (PostHog's free tier is documented
  at one year);
- aggregate retention and deletion procedures; and
- the credential model, export path, and response to abuse.

WSP will not use remote feature flags, targeted experiments, session replay,
automatic capture, user profiles, advertising, or error capture. It will not
sell analytics or join reports with GitHub, issue, support, or website
identities.

## Your OpenTelemetry metrics

Organizations may want to observe their own WSP performance and reliability.
WSP plans to support opt-in [OpenTelemetry](https://opentelemetry.io/) metrics.

The exporter will be disabled by default. WSP will make requests only to the
configured OTLP endpoint and will not intentionally copy that data to WSP or
its analytics provider. DNS, configured proxies, service meshes, and the
collector itself may process or forward requests; the operator controls
those systems and retention.

WSP will follow standard `OTEL_EXPORTER_OTLP_*` environment variables rather
than inventing credential interpolation in YAML. Header values will be redacted
from all output, never persisted after expansion, validated against header
injection, and excluded from the analytics sender's environment.

The initial metric catalog will be:

| Metric | Instrument and unit | Attributes |
|---|---|---|
| `wsp.command.invocations` | Monotonic counter, `{invocation}` | Command, outcome, interactive/JSON context |
| `wsp.command.duration` | Histogram, seconds (`s`) | Command, outcome, broad repository-count range |

`service.name=wsp`, `service.version`, and `deployment.environment.name` will be
OpenTelemetry resource attributes rather than repeated metric attributes.
Histogram boundaries, temporality, attribute enums, TLS trust, proxy behavior,
timeouts, redirects, batching, and shutdown flushing must be specified and
tested before the exporter is stable.

Unlike WSP analytics, your OTel destination may receive precise durations
because you control the endpoint and access. It still omits arguments, paths,
names, file contents, command output, and error text by default. Detailed logs,
traces, or errors require a separate design and explicit configuration.

## Changes and accountability

Adding a field requires a documented purpose, privacy review, serializer tests,
and a schema version. A new purpose, destination, identifier, finer time or size
granularity, or collection of logs/errors resets the first-run notice and
pauses sending until the notice has been shown again under the new schema —
the one case where "enabled by default" doesn't mean "never gated."

Before launch, WSP must add an effective date, project/controller contact,
applicable lawful basis, processing locations and recipients, rights and
complaint information, and a stable canonical URL. This technical proposal is
not yet a complete legal privacy notice.

The policy will be linked from the README, installation and configuration docs,
first-run notice, the `analytics.enabled` entry in the `config` help topic,
package metadata, and release notes.
Privacy concerns can be reported through the project's
[GitHub issue tracker](https://github.com/jganoff/wsp/issues).

The engineering threat model and rollout plan are in
[Privacy-Preserving Usage Telemetry](features/usage-telemetry.md).

The main precedent for the overall architecture is Codex's documented
separation of product metrics from independently configured OpenTelemetry
export, and its shipped source, which was read directly to verify the
safe-credential pattern (a public write-only key, never a server secret)
this design applies to PostHog:

- [Codex configuration reference](https://developers.openai.com/codex/config-reference/)
- [Codex configuration documentation](https://github.com/openai/codex/blob/main/docs/config.md#observability-and-telemetry)
- [Codex `codex-otel` crate source](https://github.com/openai/codex/tree/main/codex-rs/otel)
- [PostHog: Capture and batch API endpoints](https://posthog.com/docs/api/capture)
- [PostHog GeoIP plugin](https://github.com/PostHog/posthog-plugin-geoip) (`$geoip_disable`)

The full reasoning for choosing PostHog over Statsig, which was evaluated
first, is in
[`features/usage-telemetry.md`](features/usage-telemetry.md#provider-decision-posthog-project-api-key-direct-http)
and
[`features/usage-telemetry-questions.md`](features/usage-telemetry-questions.md).
