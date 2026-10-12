# Privacy-Preserving Usage Analytics and User Telemetry

Status: revised design proposal; not implemented
Last researched: 2026-08-23
Adversarial review: completed 2026-08-23 (see [adversarial review](usage-telemetry-adversarial-review.md))
Provider mechanism revised: 2026-09-19 — see [Revision: client-key, per-event ingestion](#revision-2026-09-19-client-key-per-event-ingestion)
Consent model and local state revised: 2026-09-23 — see [Revision: settings-based control, and local state shape](#revision-2026-09-23-settings-based-control-and-local-state-shape)
Requirements traceability: see [usage-telemetry-questions.md](usage-telemetry-questions.md) for which product question each field answers, and what provider capability answering it actually depends on.

## Decision

Follow the useful part of the Codex model: keep maintainer product metrics and
user-controlled OpenTelemetry completely independent.

1. **WSP analytics** is enabled by default (opt-out, matching Codex,
   Homebrew, and most peer CLI dev tools), pseudonymous product and health
   data intended for WSP maintainers, sent as one small event per eligible
   completed command. PostHog is the provider (switched from Statsig on
   2026-10-12; see "Provider Decision"), reached via a public Project API Key
   and a direct HTTP call — no vendor SDK, no server secret in the binary.
   Turned off with `wsp config set analytics.enabled false`, or
   `DO_NOT_TRACK`/`WSP_NO_ANALYTICS`.
2. **User telemetry** is an optional OpenTelemetry metrics exporter, disabled
   by default, whose only application destination is an OTLP endpoint chosen by
   the user. WSP never receives a copy.

Neither path belongs in `wsp-core`. Library consumers remain unaffected.
Neither path may change normal output, structured output, exit status, or core
command behavior. No foreground analytics network request is allowed.

The public contract is [`docs/analytics.md`](../analytics.md). It must be
published at a stable canonical URL before collection begins.

## Revision (2026-09-19): client-key, per-event ingestion

The original version of this document (below, largely preserved as designed)
assumed WSP needed to choose between direct ingestion with an unsafe embedded
secret, or operating a validating relay, and chose client-side daily
aggregation specifically to avoid ever having a stable per-install identifier.
Reading Codex's actual shipped source (`codex-rs/otel/src/config.rs`) changed
two of those conclusions:

1. **Codex does not embed a server secret.** It embeds a genuine Statsig
   **Client SDK Key** (`client-...`), which Statsig's own documentation
   describes as safe for public/client-side exposure. Its OTLP-to-Statsig
   traffic goes to a first-party OpenAI domain rather than Statsig's own
   documented OTLP endpoint (which itself requires a Server Secret Key per
   Statsig's docs) — strong evidence Codex runs a relay, not direct OTLP
   ingestion with a raw secret.
2. Statsig's plain event-logging HTTP endpoint (`/v1/log_event`) accepts a
   Client SDK Key directly, with no OTLP and no relay required. This gives WSP
   a safe direct-ingestion path Codex's own OTLP route does not use.

Given that, and given the explicit product goal of being able to tell **who**
(how many distinct reporting installations) is using WSP **for what**
(which commands/features), this revision:

- adopts a random, rotatable **Install ID**, generated when analytics is
  enabled, in place of the original design's day-scoped random report ID — a
  deliberate, disclosed trade of stricter anonymity for install-level
  visibility;
- sends **one event per eligible completed command** instead of client-side
  daily aggregation, removing the entire local queue/rotation/claim state
  machine and the detached background-uploader process design; and
- **defers the validating-relay option** (Section "Provider Decision," below)
  to a documented future escalation, since a public Client SDK Key is already
  the credential type Statsig intends to be exposed.

Sections below are updated in place to reflect this; where a subsection is
now obsolete (the local aggregate store, the detached uploader), it says so
rather than being silently deleted, since the design reasoning it worked
through (disable races, atomic writes) still informs the much smaller amount
of local state WSP keeps.

### Revision (2026-09-23): settings-based control, and local state shape

Two further changes, made together:

1. **No new `wsp analytics` command namespace.** Enable/disable is
   `wsp config set analytics.enabled <bool>` (default `true` — see "Enabled by
   default," below). `status` and `preview` are dropped as
   dedicated commands: `wsp config get analytics.enabled` answers on/off, and
   `wsp doctor` gains one informational check line for the richer state
   (enabled/disabled, Install ID present). `preview` is dropped entirely —
   plan A's per-event shape is fully fixed and enumerated already (unlike the
   old daily-aggregate design, which genuinely benefited from a live
   "show me" command), so a complete example in `docs/telemetry.md` is
   sufficient transparency. `purge` is dropped: `analytics.enabled=false`
   already deletes the Install ID, and re-enabling already issues a fresh one,
   so there is no leftover state a separate purge command would still need to
   clear.
2. **Enabled by default**, matching Codex, Homebrew, and most peer CLI dev
   tools, rather than the original opt-in design. `DO_NOT_TRACK` and
   `WSP_NO_ANALYTICS` remain the environment-variable escape hatches, and a
   first-run notice still applies (see "Consent and Control," below) — but the
   notice is disclosure, not a gate, since collection is already on.

This also prompted checking the local-state design against what the codebase
actually does elsewhere, rather than inventing a new pattern: see "Local
State," below, which replaces the original owner-only-permissions/queue design
with a flat file matching `approvals.yaml`'s existing precedent.

### Revision (2026-09-29): no CI distinction; `analytics.include-ci` dropped

The 2026-09-23 revision above still carried a CI-specific carve-out inherited
from the original opt-in design: a separate `analytics.include-ci` setting,
off by default even when analytics was on, plus a "published CI-marker
allowlist" to detect CI environments. Revisited because default-on plus a
separate CI opt-in is an inconsistency worth questioning directly, and because
"how would WSP even detect CI" turned out to have a better answer than adding
a detector.

The original reasoning had two real parts: (1) the first-run notice is
interactive-only, so a CI job would never see any disclosure a laptop user
gets, and (2) CI runners are typically ephemeral — no persisted
`analytics.yaml` between jobs — so treating CI identically mints a fresh
Install ID on every job, which would misrepresent "distinct reporting
installations" with one-shot noise. Both concerns are real, but neither
requires a *consent* carve-out or a *new detection mechanism* to address:

- **No separate gate.** `analytics.enabled` applies uniformly. CI is not
  treated differently, and there is no `analytics.include-ci` key.
- **No CI-marker allowlist.** A codebase search
  (`grep -rn "is_terminal" crates/`) shows `is_terminal()`-based interactive
  detection already used pervasively — setup-command approval
  (`setup_runner.rs`), destructive-op confirmation (`sync.rs`, `delete.rs`),
  color output (`log.rs`, `diff.rs`), and more. The event schema already had a
  separate `interactive` field for exactly this signal. A bespoke CI-specific
  detector would duplicate a mechanism the codebase already has, for a
  question `interactive` already answers well enough: whether a human was
  plausibly present to see the (interactive-only) notice, and — kept
  specifically for this — a way to separate likely-ephemeral/scripted traffic
  from human usage during later analysis, without gating on it.

This removes `analytics.include-ci` from every surface (config keys, help
topic, provider decision record) and removes the `ci` field/CI-marker
allowlist from the event schema and the OTel metric catalog. `interactive`
remains, unchanged in meaning, and now carries the full weight of that
distinction on its own.

### Revision (2026-09-29): five additional fields proposed

Added while account setup for the chosen provider was underway, to get more
signal on the product questions this design exists to answer. **This
section is history, not current truth** — three of the five fields below
were substantially revised again on 2026-10-10 (two rounds), after
independent review. For what each field actually is today, read the "Fixed
values" table and JSON example in "Analytics Data Model" — this section
exists only to record that `agent_context`, `shell_bucket`, `install_age_bucket`
(since dropped), new `feature` values, and `check_result_bucket` (since
renamed `failed_checks`) were introduced here, and why they seemed like the
right shape at the time. See "Revision (2026-10-10)," below, for the current
reasoning on all five.

### Revision (2026-10-10): adversarial review resolutions and main-branch refresh

Three independent fresh-eyes reviews (decision-usefulness, privacy/security,
implementation-coherence) and a refresh against `main` (which had drifted 20
commits, including a new workspace-local sandbox feature) produced the
following changes. Where a review's finding is specific to one field or
section, the fix is documented there; this section records the decisions and
the reasoning, in one place, dated.

**Dropped `install_age_bucket` entirely.** The security review's finding:
Statsig timestamps every event server-side on arrival regardless of what wsp
sends, and this bucket was present on *every* event. An observer of raw data
could watch a given Install ID's bucket transition (`<1d`→`1-7d`→`7-30d`→
`>=30d`) and read off the exact moment of each transition — pinning
`install_created_at` to within one send window, then using every other
event's own arrival timestamp for that Install ID to reconstruct a near-exact
invocation timeline. That is functionally the same thing "no per-invocation
timestamp" was designed to prevent. The fix is not a coarser bucket or a
lower send frequency — it's that **the field is redundant with data Statsig
already has for free**: grouping events by Install ID and looking at
first-seen/last-seen arrival timestamps gives the same retention signal,
unbucketed and more accurate, entirely in the aggregation/dashboard layer,
with no client-side field at all. `install_created_at` is removed from
`AnalyticsState` (see "Local State," below).

**`agent_context`: settled as a plain boolean, one signal, no allowlist.**
This field changed twice in one day; this is the final state, not an
intermediate one — read only this paragraph, not the ones above it, for what
it is. First pass: the decision-usefulness review wanted it promoted from a
bool to a named-harness enum; the security review had already flagged it as
a fingerprinting contributor, and naming specific harnesses moves in exactly
the wrong direction for that finding, so it became a three-value enum
(`known_agent`/`none`/`unknown`) fed by *either* an environment-variable
marker allowlist or detected workspace-local sandbox execution (see
`docs/reference/plans/2026-09-18-workspace-local-sandbox-operation.md` — a
mode built specifically so AI agents can run wsp against an isolated
workspace with no reachable host state). Second pass, from an independent
simplicity review the same day: the marker allowlist is strictly weaker than
the sandbox signal (a guess vs. a real, already-existing mechanism) *and* it
was the one adding fingerprinting risk the security review flagged — keeping
it bought nothing the sandbox signal didn't already cover, for a real cost.
**Final: `agent_context` is a plain boolean, true only when wsp detects it is
running in its own workspace-local/confined sandbox mode** (via
`GlobalCapabilities`, see "Local State," below) — no environment-variable
marker allowlist at all. This undercounts agent usage outside that sandbox
mode, which is an accepted, disclosed limitation, not an oversight: a weaker
guess was removed because it cost more (fingerprinting surface, a whole
allowlist to document and maintain) than it added.

**`failure_class` added — correction: it is not sourced from an existing
classifier, because none exists yet.** The decision-usefulness review:
`outcome` being only `success`/`observed_failure` means a maintainer can see
a failure rate rise for a command on one platform but never *why*. Added a
small fixed enum — `network`, `conflict`, `permission`, `config_invalid`,
`not_found`, `other` — present only when `outcome = observed_failure`. An
earlier version of this entry claimed it would be "sourced from wsp's
existing internal error classification." A PII audit checked that claim
directly and found no such general-purpose classifier in the codebase —
only narrow, unrelated ones (`classify_windows_write_error` for
`Availability`, `classify_show_ref_exit` for git exit codes). **This has to
be built from scratch, and how matters**: it must be a closed `match` over
typed error variants/categories that already exist in wsp's error handling,
never substring-matching on an error's `Display`/message text. Matching on
message text is the one implementation path that could let a fragment of a
real error string (which can contain paths, names, or other denylisted
content) leak through a field that looks like a safe fixed enum. This needs
a golden test asserting every `failure_class` call site takes a typed error
value, not a string, before this field ships.

**`install_channel` needs the same allowlist-with-fallback treatment `os`/
`arch` already get, which it was missing.** The PII audit found this field
specified only "compile-time packager marker only; otherwise `unknown`.
Never inferred from paths" — unlike `os`/`arch`'s explicit "small source
allowlist; uncommon values become `other`." "Never inferred from paths" only
promises wsp won't guess from argv0/CWD; it says nothing about validating
what a third-party packaging script injects at compile time, which could in
practice be a build-server hostname, an internal path, or a packager's
name/email used as a convention. Fix: `install_channel` gets the identical
closed-allowlist treatment as `os`/`arch` — a small fixed set of recognized
channels (`homebrew`, `cargo`, `github-release`, `nix`, ...), with anything
not on that list collapsing to `other` or `unknown`, validated before
transmission rather than passed through whatever a packager happened to set.

**`release_stage` simplified from four values to two, because `wsp_version`
already carries the rest.** A simplicity review: the field existed mostly to
exclude dogfood/dev noise from production metrics, but carried a `prerelease`
vs. `stable` distinction that duplicates information `wsp_version` already
has — a prerelease build's exact semver already contains its suffix (e.g.
`0.20.0-rc.1`), so a dashboard can already filter on that string without a
second field saying the same thing. The only information `release_stage`
adds that `wsp_version` doesn't is whether the binary was built from an
exact tag at all (a local/dev build has the *same* version number as its
last tag, dirty or ahead). Fix: `release_stage` becomes two values —
`tagged` / `untagged` — answering exactly that, and only that. This also
simplifies the still-needed `build.rs` change: one new boolean `rustc-env`
output (e.g. `WSP_BUILT_FROM_EXACT_TAG`) from the describe-vs-tag comparison
`build.rs` already computes, not a three-way classification.

**Raw-data minimum cell size, not just published aggregates.** Two
independent reviews (a simplicity review and a PII audit) converged on the
same gap: "suppress rare public breakdowns below a documented minimum cell
size" (stated earlier in this doc) only constrains what gets *published*. It
says nothing about `install_channel` + `release_stage` + `os` + `arch` +
`shell_bucket` + `agent_context` + `interactive` + `json` — eight low-
cardinality dimensions attached to every event under one persistent Install
ID — being join-unique for a specific install in *raw* storage, before any
dashboard exists, in a small population (e.g. one company's few wsp
installs). Publication-time suppression doesn't protect against whoever has
raw access. This is now an explicit requirement in "Mandatory provider
decision record," below: raw per-event access must be restricted the same
way the minimum-cell-size rule already restricts publication, not just the
dashboards built from it.

**`check_result_bucket` replaced with a check-ID list.** The
decision-usefulness review wanted to know *which* `doctor` invariant fired,
not just a count — otherwise `AGENTS.md`'s own requirement ("every Warn/Error
check must be auto-fixable or include actionable guidance") can't be
prioritized from the data at all. The refresh confirmed this is directly
implementable: `DoctorCheck.check: String` (`crates/wsp-core/src/output.rs`)
is already a stable per-check ID, unchanged in shape even though doctor's
implementation was heavily restructured in the same `main` refresh. New
field: `failed_checks` — the list of `check` IDs with `status != Ok` from
that run, bounded by doctor's own check count (currently well under the
~15-check ceiling `docs/features/doctor.md` already sets as a design limit),
only on the `doctor` command. Reported pre-`--fix` (as found), since the
point is measuring real-world drift regardless of whether this run then
fixed it.

**`wsp new`'s `feature` enum corrected to six real paths, not four.** The
implementation-coherence review, confirmed directly against `crates/wsp/src/cli/new.rs`
on current `main`: clap's `source` `ArgGroup` is
`["template", "from-workspace", "file", "empty"]` — bare positional `repos`
with none of those flags is a real, legal invocation in none of the four
buckets, and there's a separate, real implicit-derivation path ("when run
inside an existing workspace with no repos specified, automatically copies
the repo list," `new.rs` lines 230-231) that sets no flag at all. The
corrected enum: `template`, `from_workspace` (explicit `-w`), `file`,
`scratch_empty` (`--empty`), `scratch_repos` (bare positional repos, no
flags), `implicit_cwd_derive`.

**Cross-file delete ordering specified.** The implementation-coherence
review: "disable deletes the Install ID" had no specified order between the
`config.yaml` write and the `analytics.yaml` delete, and no specified failure
handling. Resolution: **delete/rotate `analytics.yaml`'s Install ID before
writing `config.yaml`'s `analytics.enabled=false`.** This makes the one
realistic failure mode benign and self-healing rather than silently
contradicting "disable deletes the Install ID immediately": if the process
dies between the two steps, the worst case is `config.yaml` still says
`enabled=true` with no Install ID present, which the existing rule ("enabling
generates an ID if one is not already present") already handles correctly on
the next eligible command — a fresh ID, not a stale one. Re-running the same
`wsp config set analytics.enabled false` is then a no-op on the already-gone
`analytics.yaml` and simply retries the `config.yaml` write, consistent with
wsp's existing "operations are resumable" tenet rather than needing new
cross-file transaction machinery.

**Corrected contradiction with wsp's own precedent.** The
implementation-coherence review, confirmed directly against
`crates/wsp-core/src/approvals.rs`: it does **not** store its path on `Paths`
— callers call `approvals_path(paths.data_dir())` directly. An earlier
version of this doc proposed adding `Paths.analytics_path` anyway, while
claiming to follow approvals.rs "exactly." Corrected: no new `Paths` field.
`analytics.rs` calls `analytics_path(paths.data_dir())` directly, the same
way `approvals.rs` is actually called from `setup_runner.rs` and `doctor.rs`
today — matching the real precedent, not an assumed one.

**`release_stage: development` needs a new `build.rs` output.** The
implementation-coherence review, confirmed against `crates/wsp/build.rs` on
current `main` (unchanged by the refresh in the relevant way): it only emits
one combined display string, `WSP_VERSION_STRING`
(`"{pkg}"` or `"{pkg} ({describe})"`), never a separate queryable signal for
"was this built from an exact tag." `release_stage`'s `development` case
needs `build.rs` to emit a second, dedicated `rustc-env` output (e.g.
`WSP_BUILT_FROM_EXACT_TAG=true/false`) reusing the describe-vs-tag comparison
it already computes — not string-parsing the display value, whose format is
a human-display convenience, not a stable contract.

**`analytics.yaml` availability wired into wsp's real mechanism, not a new
one.** The refresh found that `main` gained a `GlobalCapabilities`/
`Availability` mechanism (`crates/wsp-core/src/config.rs`), built for the
workspace-local sandbox feature, that answers exactly "can I write this
piece of global state" — `Absent`/`Unavailable`/`Malformed`/`ReadOnly`/
`Available`/`Unknown`, computed via a pure, side-effect-free OS access check
(`write_availability()`, no probe files, no locks). This replaces the
original design's speculative bespoke safety logic (symlink rejection,
read-only detection written from scratch) with: add `analytics: Availability`
to `GlobalCapabilities`, computed identically to `approvals`
(`write_availability(&paths.data_dir().join("analytics.yaml"))`). The
analytics sender's only gate is `capabilities.analytics == Available`; when
it isn't (including the sandbox case `GlobalCapabilities` exists to
describe), analytics is silently unavailable — no event, no Install ID
creation attempt — automatically, for free, instead of needing its own
detection code.

## What We Adopt From Codex

Codex documents two independent systems:

- periodic product and health metrics sent to OpenAI, with an analytics
  disable setting; and
- separately configured, opt-in OTel logs and metrics sent to a collector the
  operator controls.

WSP adopts the separation, typed low-cardinality event fields, and an explicit
OTel destination for the user-owned path. It deliberately differs in several
ways appropriate for a small open-source project:

- WSP does not collect raw prompts, logs, errors, output, or traces;
- the Install ID is disclosed and deleted the moment analytics is turned off,
  and never a hardware/advertising identifier — unlike a persistent device
  ID, it is scoped to the current analytics-enabled period; and
- WSP treats CI the same as any other invocation rather than gating it
  separately (see "Revision (2026-09-29)," above) — the one place this design
  is deliberately less CI-cautious than a typical enterprise rollout might be,
  traded for not inventing a second consent mechanism.

Using the same provider as Codex is precedent, not proof that WSP receives the
same commercial terms, privacy configuration, or ingestion architecture —
those are verified independently in "Provider Decision," below.

## Product Questions and Limits

Analytics may answer:

1. Which command and feature counters are represented among reporting
   installations, and by how many distinct installations?
2. Which versions, OS families, architectures, and package-supplied install
   channels are represented?
3. Among commands that reach the recording point, what fraction report success
   versus an ordinary observed failure, and (when failing) in which broad
   failure category?
4. What duration-bucket distribution is observed for completed commands?
5. Which coarse repository-count buckets accompany relevant commands?
6. Roughly how long reporting installations have had analytics on — computed
   in the aggregation/dashboard layer from PostHog's own first-seen/last-seen
   arrival timestamps per `distinct_id`, never a client-transmitted field (see
   the 2026-10-10 revision above for why).

It cannot validly answer:

- who a person, organization, repository, or project is;
- exact active users, or a complete cross-day adoption funnel — only what
  reporting installations with a live Install ID show;
- behavior of users who did not opt in or whose uploads fail;
- crash, `SIGKILL`, power-loss, or complete interruption rates;
- exact end-to-end process latency unless a separately defined timer covers it;
- security, abuse, or individual developer performance; or
- whether low usage makes a feature safe to remove.

Dashboards must say "reporting installations" and disclose exclusions,
delivery failure, schema/version coverage, and spoofing risk. Rare breakdowns
must be suppressed below a documented minimum cell size. **Any published
"distinct installations" figure must be reported split by `interactive`,
never as one blended number** — otherwise a single organization's ephemeral
CI fleet (each job mints a fresh Install ID; see the no-CI-distinction
revision above) can dominate and misrepresent it uncontested.

## Consent and Control

### State: two independent booleans, not a four-state machine

Analytics is enabled by default, so the original four-state
(`unnotified`/`notice_shown`/`enabled`/`disabled`) model collapses to two
independent, much simpler pieces of state:

| State | Where it lives | Behavior |
|---|---|---|
| `analytics.enabled` | `config.yaml` (user-authored, default `true`) | Whether eligible commands send events at all. |
| notice shown | `analytics.yaml` (generated, one-time) | Whether the one-time first-run disclosure has already been printed. Independent of `enabled` — the notice is disclosure, not a gate, since collection already defaults on. |

Setting `analytics.enabled` to `true` (explicitly, or by never having touched
it) generates a fresh Install ID if one is not already present. Setting it to
`false` deletes the Install ID and the `analytics.yaml` state file
immediately. Re-enabling after a `false` period issues a new Install ID — the
old one is never reused. A materially new schema or purpose resets the
notice-shown flag and blocks sending until the notice has been shown again,
regardless of the `enabled` setting.

Suggested notice:

> wsp sends small, anonymous usage reports by default. Run
> `wsp config set analytics.enabled false` to turn it off. Details:
> https://github.com/jganoff/wsp/blob/main/docs/telemetry.md

The notice is plain text on stderr, contains no color-only meaning, honors
`NO_COLOR`, and is shown once, after the first eligible completed interactive
invocation regardless of success. It never appears in JSON, completion, CI,
noninteractive use, or the analytics sender itself.

### Effective precedence

Highest priority wins:

1. a build without the `analytics` Cargo feature is `unavailable`;
2. `DO_NOT_TRACK` other than `0` pauses sending;
3. `WSP_NO_ANALYTICS` other than `0` pauses sending;
4. `analytics.enabled=false` in `config.yaml` pauses sending;
5. otherwise sending proceeds (the default).

Environment opt-out never sends anything while active, and there is no local
backlog to flush once it is removed — with no event queue, "removing the
opt-out" simply means the next eligible command can send its own event.
Setting `analytics.enabled=false` acquires the `analytics.yaml` lock and
deletes the Install ID immediately.

There is no CI-specific exclusion (see "Revision (2026-09-29)," above):
`analytics.enabled` is the only gate, evaluated the same way regardless of
`interactive`. Packager builds may compile analytics out, removing the
sender, endpoint/key strings, and analytics-only dependencies.

### User surface

No new command namespace, and no CI-specific setting. Analytics rides
entirely on two existing surfaces:

- `wsp config set analytics.enabled <true|false>`: the one enable/disable
  control, evaluated identically whether interactive, JSON, or CI. Setting it
  to `true` generates a fresh Install ID if one is not already present.
  Setting it to `false` deletes the Install ID and the local state file
  immediately — this is an explicit, user-run mutating command, so deleting
  state as its consequence matches the "explicit side effects" tenet rather
  than violating it.
- `wsp config get analytics.enabled` / `wsp config ls`: on/off status.
- `wsp doctor`: one informational check line — enabled/disabled and whether
  an Install ID is present. Never prints last-send timing (see "Local State"
  for why that isn't tracked) and never prints anything OTel-related, since
  that is a fully separate, user-owned path.

There is no `preview` and no `purge`. Plan A's event shape is fixed and fully
enumerated (see "Analytics Data Model"), so `docs/telemetry.md` carries a
complete real example instead of a live preview command, and `disable`
already clears all local state a `purge` would otherwise need to.

## Analytics Data Model

### One event per eligible completed command

WSP builds one typed event immediately after a command reaches its recording
point and sends it as a PostHog capture payload:

```jsonc
{
  "api_key": "<Project API Key, phc_...>",
  "event": "wsp_command",
  "distinct_id": "<Install ID>",
  "properties": {
    "$geoip_disable": true,             // required on every event — see "Provider Decision"
    "schema_version": "1",
    "command": "st",
    "outcome": "success",
    "duration_bucket": "100-249ms",
    "wsp_version": "0.19.0",
    "release_stage": "tagged",
    "os": "macos",
    "arch": "arm64",
    "install_channel": "homebrew",
    "interactive": "true",
    "json": "false",
    "agent_context": "false",
    "shell_bucket": "zsh",
    "repo_count_bucket": "3-5",
    "feature": null
    // failure_class: only present when outcome="observed_failure"
    // failed_checks: only present when command="doctor"
  }
}
```

There is no per-invocation timestamp field WSP sends — PostHog's ingestion
records arrival time itself, and WSP does not persist a local copy of sent
events. `distinct_id` is the only linkage between two events; it is the
Install ID, not a person. `$geoip_disable: true` is not optional decoration:
it is the mechanism (see "Provider Decision," above) that keeps PostHog's own
pipeline from deriving and storing location data from the request's source
IP — every event construction path must set it, enforced the same way the
rest of this payload is typed and fixed rather than freeform.

### Fixed values

| Field | Rule |
|---|---|
| `schema_version` | Positive source-controlled integer, sent as a string. |
| `command` | Static canonical clap leaf; aliases normalized; no unknown string. |
| `outcome` | `success` or `observed_failure`. No error, exit code, or crash claim. |
| `failure_class` | `network`, `conflict`, `permission`, `config_invalid`, `not_found`, or `other`. Only present when `outcome = observed_failure`. Must be built as a closed `match` over typed error variants wsp's error handling already distinguishes — never substring-matching on an error's display/message text, which could leak a denylisted fragment through what looks like a safe enum. No classifier for this exists yet; it has to be built, with a golden test proving every call site takes a typed error, not a string. |
| `duration_bucket` | `<10ms`, `10-49ms`, `50-99ms`, `100-249ms`, `250-499ms`, `500-999ms`, `1-3s`, `3-10s`, `10-30s`, `30-120s`, `>=120s`. |
| `wsp_version` | Semver embedded in binary. |
| `release_stage` | `tagged` or `untagged`. Simplified from an earlier four-value `stable`/`prerelease`/`development`/`unknown` design once it was clear `wsp_version` already carries the stable-vs-prerelease distinction in its own semver suffix (e.g. `0.20.0-rc.1`) — a second field repeating that would be redundant. The only new information this field adds is whether `build.rs`'s existing `git describe --tags --dirty --always` comparison shows the build came from an exact tag (`tagged`) or not (`untagged`: a dirty tree, or commits past the last tag — a local/dev build, which has the *same* version number as its last tag). Requires a new dedicated `build.rs` boolean output; see "Implementation Map." |
| `os` | Small source allowlist; uncommon values become `other`. No patch version. |
| `arch` | Small source allowlist; uncommon values become `other`. |
| `install_channel` | Small fixed source allowlist (`homebrew`, `cargo`, `github-release`, `nix`, ...), the same closed-set-with-fallback treatment `os`/`arch` get; anything not on the allowlist becomes `other` or `unknown`, validated before transmission. Never inferred from paths, and never a raw compile-time marker passed through unchecked — a third-party packaging script could otherwise inject an arbitrary string (a hostname, an internal path, a packager's name/email). |
| `interactive` | Fixed boolean derived from terminal state (`is_terminal()`, the same signal used elsewhere in the codebase). The one signal that distinguishes likely-human, likely-scripted/CI, and JSON traffic — no separate CI field or detector. |
| `json` | Fixed boolean. |
| `agent_context` | Fixed boolean. True only when wsp detects it is running in its own workspace-local/confined sandbox mode (via `GlobalCapabilities`; see "Local State") — a mode built specifically so AI agents can run wsp against an isolated workspace. No environment-variable marker allowlist (dropped: it added fingerprinting surface for a weaker signal than the sandbox detection already gives). Necessarily undercounts agent usage outside that sandbox mode; never claimed as complete agent-traffic accounting. |
| `shell_bucket` | `zsh`, `bash`, `fish`, `powershell`, `other`, `unknown`. Derived locally from shell detection; the raw shell path/name is never transmitted — see "Hard denylist," below, for why this is a carve-out and not an exception to it. |
| `repo_count_bucket` | `0`, `1`, `2`, `3-5`, `6-10`, `11-25`, `26-50`, `>50`. |
| `feature` | Source-defined reviewed enum, never user input. For `wsp new`: `template` (`-t`), `from_workspace` (explicit `-w`), `file` (`-f`), `scratch_empty` (`--empty`), `scratch_repos` (bare positional repos, no flags), or `implicit_cwd_derive` (no repos given, inside a workspace, repos copied automatically) — six real paths through `new.rs`'s `source` `ArgGroup` plus the two it doesn't cover. |
| `failed_checks` | A list of `doctor`'s own stable per-check IDs (`DoctorCheck.check: String` only — never `DoctorCheck.message` or `DoctorCheck.details`, which can contain real clone/registered URLs per `docs/features/doctor.md`'s own example) with `status != Ok` from that run — e.g. `["stale-mirror"]`. Only present on the `doctor` command, reported pre-`--fix`. Bounded by doctor's own check count (a fixed, reviewed, small set, currently well under the ~15-check design ceiling in `docs/features/doctor.md`), not an open list. |
| `Install ID` | Random UUID generated on `enable`; rotated on every fresh `enable` after a `disable`; never derived from hardware or account identity. Sent as PostHog's top-level `distinct_id`, not inside `properties`. |
| `$geoip_disable` | Always `true`. Not a measurement field — the mechanism that tells PostHog's ingestion not to derive/store location from the request's source IP. Required on every event, not optional. |

The initial timer is named `dispatch_to_render_duration`; it starts immediately
before path/config resolution and dispatch and ends after rendering. Argument
parsing, process startup, sender spawn, and teardown are excluded. If a true
process-start measurement is later added, it is a separate metric and schema
field. User-facing claims must match the actual interval.

### Hard denylist

Typed constructors and golden tests prove that events cannot contain:

- arguments, flags with values, arbitrary clap strings, or exit codes;
- workspace, template, registry, repo, remote, host, organization, branch, or
  user names;
- paths, URLs, environment names/values, the raw shell name/path (as opposed
  to `shell_bucket`, above), terminal, locale, timezone, hostname, username,
  email, or IP fields;
- any compile-time packager-supplied string beyond what matches
  `install_channel`'s fixed allowlist;
- stdout, stderr, Git output, file content, setup commands, descriptions,
  AGENTS.md, prompts, generated content, errors, panics, backtraces, or signals;
- `doctor`'s own `message` or `details` fields (only its fixed `check` ID may
  appear in `failed_checks`; `details` can contain real clone/registered
  URLs per `docs/features/doctor.md`'s own example);
- hardware, MAC, or advertising identifiers; or
- arbitrary provider properties beyond the fixed `metadata` keys above.

Because WSP sends a hand-built JSON request rather than running a vendor SDK,
there is no SDK-injected automatic context (device info, session replay
hooks, autocapture) to separately disable — it was never added.
IP-derived geolocation is the one enrichment a provider's ingestion pipeline
could otherwise perform entirely server-side, outside WSP's control; PostHog
documents a client-settable `$geoip_disable: true` property for exactly this
(see "Provider Decision" and the `Install ID`/`$geoip_disable` rows above),
which is why it's sent on every event rather than left as an unresolved
provider-review item the way it was under the original Statsig design.
Confirming that flag actually works via raw HTTP capture (not only through an
official SDK) is still a pre-launch verification item, not an assumption.

## Local State

**Not a subdirectory.** The original design's `analytics/` directory
(intended to hold a rotating queue) is dropped along with the queue itself.
What's left to store — an Install ID and a notice-shown flag, nothing else —
doesn't fit "collection of many keyed items" the way `mirrors/`, `gc/`, and
`templates/` do; it fits `config.yaml` and `approvals.yaml`'s shape instead:
a single small record. It lives at `<data_dir>/analytics.yaml`
(`~/.local/share/wsp/analytics.yaml` on a typical Linux system), next to
`config.yaml` and `approvals.yaml`, following `approvals.rs`'s existing
precedent exactly:

```rust
pub struct AnalyticsState {
    pub install_id: Option<String>,
    pub notice_shown: bool,
}

pub fn analytics_path(data_dir: &Path) -> PathBuf {
    data_dir.join("analytics.yaml")
}
```

No `install_created_at` or any other timestamp — removed in the 2026-10-10
revision (see above) once it was shown to reopen the "no per-invocation
timestamp" property via bucket-transition timing. Retention analysis, if
wanted, is a dashboard/aggregation-layer computation over the provider's own
arrival timestamps grouped by Install ID (`distinct_id` on PostHog) — not a
client-side field.

- **No `Paths.analytics_path` field.** Corrected in the 2026-10-10 revision:
  `approvals.rs` — the precedent this is supposed to follow — does not store
  its path on `Paths` either; callers call `approvals_path(paths.data_dir())`
  directly (confirmed against `setup_runner.rs` and `doctor.rs`). `analytics.rs`
  does the same: `analytics_path(paths.data_dir())` at each call site, no new
  `Paths` field, no edit to `Paths::from_dirs`.
- Reads are lock-free (load + check), same as `approvals::load`. Writes
  acquire `FileLock::acquire(&analytics_path, Duration::from_secs(30))`
  directly — this is a fourth kind of store beyond config/metadata/template,
  same as `approvals.rs` already is, so it does not need a new named
  `filelock::with_*` helper, just the same manual pattern approvals.rs uses.
- Serialized with `serde_yaml_ng`, atomic `NamedTempFile::new_in` + `persist`,
  matching `approvals::save` line for line — no reason for analytics to be
  the one JSON file, or the one hand-rolled write path, in a codebase that
  already has this exact pattern.
- **Availability is `GlobalCapabilities`'s job, not bespoke logic.** `main`
  gained, for the workspace-local sandbox feature, a `GlobalCapabilities`
  struct (`crates/wsp-core/src/config.rs`) with one `Availability` field per
  global store (`registry`, `mirrors`, `templates`, `approvals`), each
  computed by `write_availability()` — a pure, side-effect-free OS access
  check ("ask the operating system... without probes or lock files"), never a
  probe file. This is exactly the question the original design's symlink-
  rejection/read-only-detection section was trying to answer from scratch.
  Corrected: `GlobalCapabilities` gains a fifth field, `analytics:
  Availability`, computed identically —
  `write_availability(&paths.data_dir().join("analytics.yaml"))`. The
  analytics sender's only gate is `capabilities.analytics ==
  Availability::Available`; every other state (`Absent`, `Unavailable`,
  `Malformed`, `ReadOnly`, `Unknown`) means silently do nothing — no event, no
  Install ID creation attempt. This is also what makes analytics behave
  correctly, for free, inside a workspace-local sandbox with no reachable
  host state, which is exactly the scenario `GlobalCapabilities` exists to
  describe.
- **No last-send outcome.** An earlier draft of this section considered
  recording each send's outcome for `doctor` to report. Dropped: unlike
  `config.yaml`/`approvals.yaml`, which only change on a user-driven mutation,
  a last-send field would mean acquiring the lock and rewriting this file on
  every single eligible command — exactly the per-invocation disk write
  "reads stay local" and "fast is invisible" warn against. `analytics.yaml`
  now only changes when `analytics.enabled` toggles, matching how rarely
  `approvals.yaml` changes. `wsp doctor` reports only enabled/disabled and
  whether an Install ID is present, both free to read.
- **Delete-then-write ordering.** Per the 2026-10-10 revision above: setting
  `analytics.enabled=false` deletes/rotates `analytics.yaml`'s Install ID
  *before* `config.yaml`'s `analytics.enabled` field is written, making the
  one realistic failure mode (process dies between the two writes) self-heal
  on the next eligible command rather than leaving a stale Install ID behind.

**Obsolete from the original design:** the daily-aggregate rotation
(ready → claimed → sent), the 256 KiB queue cap, the seven-day expiry, and all
crash/orphaned-claim recovery. There is no queue to rotate or recover, because
there is nothing to hold once an event is either sent within its send window
or dropped.

## Sending an Event

After a command reaches its recording point, WSP builds the event, checks
effective consent (enabled, not environment-opted-out — the same check
regardless of `interactive`), and if eligible:

1. spawns sending the HTTP request on a background thread/task, not on the
   command's own return path;
2. gives it a short, fixed grace window (on the order of 100–300ms, to be
   set by benchmark) before the process is allowed to exit;
3. if the request does not complete within the window, the event is simply
   dropped — no retry, no local persistence, no backlog, and (per "Local
   State," above) no write recording that it happened either way.

This intentionally accepts some best-effort delivery loss (a command that
exits abruptly, or a slow network, can drop an event) in exchange for
eliminating the detached background-uploader process, its
platform-specific detachment code, and its crash/claim-recovery logic
entirely. No foreground command performs analytics network I/O; the grace
window is bounded and benchmarked so it cannot become user-visible latency.

**Obsolete from the original design:** the detached child process, its
platform-specific session/process-group/Known-Folder handling, spawn/queue
benchmarks across read-only filesystems and antivirus-slow process creation,
and the binary-replacement-during-upgrade claim-recovery logic. All of that
existed to serve a durable local queue that per-event sending no longer needs.

HTTP behavior still needs a real answer, at a much smaller scope than before:
native trust store, a short connect/total timeout, no automatic retry, and a
capped response read (the response body is discarded either way). Redirects
to a different origin are rejected. Credentials and OTLP environment
variables are excluded from the analytics sender's environment; there is now
only one credential (the public Client SDK Key), and it is not a secret.

## Provider Decision: PostHog, Project API Key, Direct HTTP

**Revised 2026-10-12 — switched from Statsig to PostHog.** The original
provider choice (Statsig, documented below the line at the end of this
section for the record) followed Codex's precedent and was safe — the
client-key/direct-HTTP architecture was verified sound. It was replaced after
hands-on comparison of what each vendor's *free tier* actually lets a
maintainer do with the data, which is a different question from whether
ingestion is safe:

- **PostHog's ad hoc SQL layer (HogQL) is confirmed free-tier, with no
  per-metric dimension cap.** Statsig's free Developer tier draws an explicit
  line between "analytics" and a paid-only "advanced analytics" tier, and
  which side of that line our actual breakdown needs would fall on could not
  be confirmed from any available source. Statsig's "Custom Metrics" widget
  also has a documented hard cap of **4 dimensions** (1 via `value`, 3 via
  `metadata`) — survivable, but an unforced constraint PostHog's SQL access
  doesn't have, and array-valued fields like `failed_checks` have a
  documented PostHog path (`JSONExtractArrayRaw`/`arrayJoin` in HogQL) that
  Statsig had no confirmed equivalent for.
- **PostHog has a concrete fix for this design's single biggest unresolved
  item.** The Statsig version of this document flagged, as its most
  important open risk, that the Install ID reaches raw ingestion with no
  relay, so IP-based geolocation could attach to it indefinitely, outside
  wsp's control. PostHog documents a per-event property for exactly this:
  **`$geoip_disable: true`** skips IP-based enrichment for that event,
  set directly by the sender — not a hoped-for provider setting to be
  confirmed later, a flag wsp controls on every request it sends. This
  doesn't eliminate PostHog's own access to the source IP of the HTTPS
  request itself (nothing client-side can prevent a server from seeing the
  IP a request came from), but it stops PostHog's own pipeline from deriving
  and *storing* location data from it. This is now a **required** field, not
  an optional hardening step — see "Hard denylist," below.

See [`usage-telemetry-questions.md`](usage-telemetry-questions.md) for the
full capability comparison (ease of use, free-tier limits, UI maturity) that
drove this decision.

**Chosen:** a PostHog **Project API Key** (`phc_...`), sent as the `api_key`
field in the JSON body of a plain HTTPS POST to
`https://us.i.posthog.com/i/v0/e/` (region-specific — `eu.i.posthog.com` for
EU Cloud; confirm the actual project's region during implementation). No
vendor SDK dependency, no relay.

### Why not an SDK

PostHog's official SDKs exist for many languages but not, as far as this
design could confirm, officially for Rust. More importantly, the same
reasoning that ruled out Statsig's server SDK applies in spirit here: a
hand-built JSON request means no SDK-injected automatic context (device
info, session replay hooks, autocapture) to separately disable — it was
never added. Raw HTTP to the capture endpoint is also simply what the
PostHog capture API is designed for; unlike Statsig, there was never a
secret-vs-public-key SDK distinction to reason through here, because
PostHog's capture API has always been the public-token-direct-HTTP path.

### Credential safety

PostHog's own documentation is explicit about the split, the same shape as
Statsig's client-key/server-secret split: the **Project API Key** (`phc_...`)
"is intentionally public and safe to include in frontend bundles... a
write-only key that can only create new events and can't read events or any
of your other data." The **Personal API Key** (`phx_...`) "grants admin
access and must never be exposed" — this is PostHog's equivalent of a server
secret, and wsp never uses it. Only `phc_...` ships in the binary.

### Untrusted ingestion threat

No open-source client can keep a credential secret, even a "public" one, from
being extracted and reused outside WSP. A Project API Key is safe to expose
in the sense that PostHog designed it to be exposed and scoped its
capabilities accordingly (write events to one project; no read access; no
cross-project reach) — it is not safe in the sense of being unforgeable.
Anyone holding the key can send arbitrary events shaped however they like
into WSP's PostHog project. PostHog's own capture endpoint documentation
confirms this isn't hypothetical: the public capture endpoint has no
request-level rate limit and **returns 200 even when the project is over its
billing quota** — ingestion doesn't self-protect against volume at all.

Mitigations, unchanged in spirit from the Statsig version of this design:

- hard-disable paid overage on the PostHog project (a spend cap, not a
  request-count throttle WSP controls) — load-bearing given the point above;
- treat all received data as unauthenticated advisory evidence — never use it
  for security or irreversible decisions;
- validate WSP's own outgoing events against the fixed enums above (protects
  WSP's dashboards from WSP's own bugs, not from a deliberate forger);
- monitor for anomalous volume or field combinations; and
- never claim PostHog enforces WSP's schema on write — it does not.

### Deferred: a validating relay

If free-tier limits, spend, or forged-event volume become a real problem, the
escalation path is a small WSP-operated relay: a stateless endpoint holding
the real PostHog Personal API Key (if one were ever needed for a relay's own
administration — ordinary event forwarding doesn't require it), enforcing
JSON schema, enum and timestamp bounds, size, and rate limits before
forwarding to PostHog. It is deliberately not built today: it adds its own
availability, logging, retention, and security obligations — a permanent
service — which is exactly the operational cost a project-key approach
avoids while it remains sufficient.

### Mandatory provider decision record

Record dated evidence for:

- exact free quota (1M events/mo per available sources), definition of a
  billable event, and confirmation (from PostHog's terms/console, not a
  third-party summary) of what actually happens past the free tier;
- ingestion endpoint for the project's actual region, maximum request/event
  size, and any documented rate limit specific to the Project API Key on
  `/i/v0/e/`;
- complete wire capture and provider-stored field/profile snapshot, including
  **confirmation that `$geoip_disable: true` actually suppresses stored
  location data** for events sent via raw HTTP capture (not only through an
  official SDK) — this needs the same empirical spike-test treatment as the
  rest of this design, not an assumption from documentation;
- processing region, DPA, subprocessors, international transfers, breach
  route, controller/contact, and applicable lawful basis;
- enforceable raw retention (1 year on the free tier per available sources),
  aggregate retention, deletion, and export;
- whether PostHog's deletion tooling can remove all events tied to a given
  Install ID (`distinct_id`) on request;
- **raw per-event access restricted by the same minimum-cell-size discipline
  the "Product Questions and Limits" section already requires for published
  aggregates** — the joint tuple of `install_channel`/`release_stage`/`os`/
  `arch`/`shell_bucket`/`agent_context`/`interactive`/`json` under one
  persistent Install ID can be unique to a single install in a small
  population, which publication-time suppression alone does nothing to
  protect against; and
- account/key shutdown and data deletion procedure.

Failure of any privacy, retention, or hard-cost gate means choosing a
different provider or building the relay described above.

### Alternatives

| Option | Strength | Why not first |
|---|---|---|
| Statsig | Higher free-tier event quota (2M/mo vs. PostHog's 1M/mo); direct Codex precedent; safe client-key architecture verified sound | Chosen first, then replaced: free-tier analytics depth was ambiguous (an undocumented "advanced analytics" paywall line, an unverifiable 4-dimension Custom Metric cap, no confirmed geolocation opt-out), none of which PostHog leaves ambiguous |
| Aptabase | Purpose-built privacy-first application analytics, open-source, official Rust SDK | Free tier (20K events/mo) is small relative to expected per-command event volume |
| TelemetryDeck | Privacy-conscious app event service | Smaller ecosystem and no official Rust SDK found |
| Umami/Matomo/Plausible | Established privacy-first web analytics | Web/page semantics fit a CLI poorly |
| Sentry | Excellent errors and Rust integration | Error content and stack context have a different, higher-risk purpose; requires a separate explicit design |
| OTel + Grafana Cloud/Honeycomb/etc. | Vendor-neutral, reuses the already-planned OTel instrument catalog | Maintainer-analytics-via-operator-observability is not how comparable AI coding tools report product usage; would still require the maintainer to own dashboards and cardinality |
| Custom database/Cloudflare relay | Maximum schema control | This is the deferred relay above — a permanent service, privacy surface, dashboards, abuse controls, backups, and operational work |
| Downloads/Homebrew/Scarf | No runtime telemetry, useful distribution signal | Cannot measure active commands, outcomes, or duration; use alongside runtime reports |

## User-Owned OpenTelemetry

Unchanged from the original design. OTel configuration is global/user-level
and disabled by default. Project files cannot silently enable an exporter or
select a destination. Follow standard `OTEL_EXPORTER_OTLP_*` environment
variables where possible; do not invent `${TOKEN}` expansion in YAML.

Initial catalog:

| Metric | Instrument | Unit | Attributes |
|---|---|---|---|
| `wsp.command.invocations` | Monotonic counter | `{invocation}` | `wsp.command`, `wsp.outcome`, `wsp.interactive`, `wsp.json` |
| `wsp.command.duration` | Histogram | `s` | `wsp.command`, `wsp.outcome`, `wsp.repo_count_bucket` |

Resource attributes include `service.name=wsp`, `service.version`,
`telemetry.sdk.*`, and optionally `deployment.environment.name`. Define explicit
histogram boundaries, cumulative/delta temporality support, stable enum values,
and a schema URL/version.

Short-lived process semantics are a design gate. Choose one after measurement:

- a bounded shutdown flush to a local/nearby collector with a strict opt-in
  latency budget; or
- a separate durable OTel queue/uploader.

Never silently lose every metric, block indefinitely, or reuse the analytics
sender's destination. Specify connect/total/shutdown timeouts, retry,
redirect, proxy, TLS/mTLS/custom CA, gzip, endpoint path, header validation,
standard env precedence, and failure visibility. Header values are redacted in
all status/debug/error output, never persisted expanded, and never inherited by
the analytics sender. Test unreachable DNS, TCP blackhole, TLS stall, 429,
collector forwarding, abrupt exit, and exporter self-instrumentation recursion.

Logs, traces, precise command arguments, output, paths, names, and error text
are outside the initial OTel design. Adding them requires an explicit separate
privacy/security proposal even though the user owns the endpoint.

## Documentation and Governance

Before launch:

- convert `docs/analytics.md` from proposal into a complete technical and legal
  notice with effective date, controller/contact, lawful basis, recipients,
  regions/transfers, rights, complaint route, retention, and limitations;
- link it from README, installation/config docs, first-run notice,
  the `analytics.enabled` entry in the `config` help topic, package metadata,
  release notes, and website;
- publish exact schema, wire example, stored provider snapshot, provider
  decision record, and minimum dashboard cell size;
- publish an aggregate dashboard with sampling/delivery/spoofing caveats;
- restrict raw access and prohibit joins with GitHub/support/web identities;
- rerun provider and privacy review annually and on material changes.

Material changes — new purpose, destination, identifier, finer time/size
granularity, logs/errors, or provider enrichment — bump the schema, reset the
notice-shown flag, and pause sending until the notice has been shown again
under the new schema. This is the one case where default-on doesn't mean
"never gated": a materially new schema gets its own fresh disclosure before
it starts flowing, even though ordinary first use does not wait on one.

## Verification

Unit/property tests cover typed serialization, every clap leaf, buckets,
consent precedence, schema-bump notice reset, and inability for denylisted
random strings to enter events.

Integration/security tests cover:

- zero DNS/socket attempts while `analytics.enabled=false`, DNT, or
  environment-opted-out;
- notice/JSON/completion/help/analytics-sender isolation;
- toggling `analytics.enabled=false` racing an in-flight send;
- corrupt or unreadable `analytics.yaml` falls back to a fresh state without
  crashing, and concurrent-process access to it behaves like `approvals.yaml`'s
  existing tests — no bespoke symlink/reparse defenses, since none exist
  elsewhere in the codebase for this class of file;
- proxy/custom CA/redirect/DNS/TLS/IPv6 and response-size behavior for the
  outgoing request;
- exact PostHog golden wire payload for `/i/v0/e/`, including `$geoip_disable`;
- exact OTLP metrics/resource attributes and redacted secrets for the
  user-owned path;
- analytics-disabled builds contain no sender code, endpoint, or key.

Release gate: packet capture must assert the destination, TLS SNI, request
bytes, and absence of forbidden data. An independent maintainer signs the
checklist. Prerelease dogfooding with analytics forced on is required for at
least two weeks before defaulting it on for everyone.

## Rollout

1. **Policy and due diligence:** publish proposal; complete the PostHog
   provider decision record, legal/privacy notice, and public dashboard
   design.
2. **Local-only experiment:** typed event construction, Install ID lifecycle,
   `analytics.yaml` load/save, `config`/`doctor` wiring, send-window
   benchmarks, and no real network endpoint.
3. **Prerelease analytics, forced on for dogfooding:** wire the real
   `/i/v0/e/` call, hard cost cap on the PostHog project, full
   packet/stored-field audit (including confirming `$geoip_disable` actually
   suppresses stored location data), and maintainer dogfood — ahead of, not
   instead of, the eventual default.
4. **Stable, enabled by default:** ship the notice and the `true` default;
   audit stored fields and public aggregates after one week, 30 days, and 90
   days; remove unused fields.
5. **User OTel:** independently implement and stabilize only after lifecycle,
   TLS/proxy, credential, and semantic-convention gates pass.

## Implementation Map

- `crates/wsp/src/main.rs`: measurement boundaries, canonical command/outcome,
  notice eligibility, post-render event construction and send trigger.
- `crates/wsp-core/src/analytics.rs`: typed event builder, Install ID
  lifecycle, `analytics.yaml` load/save (mirrors `approvals.rs`'s shape),
  HTTP sender. No dedicated CLI module — there are no analytics subcommands.
- `crates/wsp/src/cli/doctor.rs`: the one informational analytics check line;
  `DoctorOutput.checks`/`summary` (`crates/wsp-core/src/output.rs`) already
  exposes the per-check `check: String` IDs the `failed_checks` event field
  reads, unchanged by this design.
- `crates/wsp/src/otel.rs`: independent OTel instruments/export lifecycle.
- `crates/wsp-core/src/config.rs`: durable user-authored `analytics.enabled`
  (default `true`) and `otel.*` configuration only if needed by the binary;
  no collection or transport code. Also where `GlobalCapabilities` gains its
  fifth field, `analytics: Availability`, alongside `registry`/`mirrors`/
  `templates`/`approvals`. `wsp config set` already provides read-modify-write
  through existing config locking, so toggling analytics needs no new command
  surface — only a hook so that writing `analytics.enabled=false` first
  deletes the `analytics.yaml` Install ID, then writes `config.yaml` (see
  "Local State" for why that order, not the reverse).
- `crates/wsp/src/cli/cfg.rs` and help: documented settings/completer for
  `analytics.enabled`.
- `crates/wsp/build.rs`: needs a new dedicated `rustc-env` output (e.g.
  `WSP_BUILT_FROM_EXACT_TAG`) reusing its existing describe-vs-tag
  comparison, so `release_stage: development` is derivable — `WSP_VERSION_STRING`
  alone is a display string, not a stable contract to parse.

Install ID and the notice-shown flag belong in `analytics.yaml`; the user's
enabled/disabled choice remains in `config.yaml`. All read-modify-write uses
WSP's file-lock patterns; no lock is held over network I/O.

## Research Sources

- [Codex configuration reference](https://developers.openai.com/codex/config-reference/)
- [Codex observability and telemetry configuration](https://github.com/openai/codex/blob/main/docs/config.md#observability-and-telemetry)
- [Codex `codex-otel` crate](https://github.com/openai/codex/tree/main/codex-rs/otel) — `README.md`, `src/config.rs`, `src/metrics/config.rs`, `src/metrics/client.rs`, and `crates/wsp/src/core/src/otel_init.rs`-equivalent (`codex-rs/core/src/otel_init.rs`, `codex-rs/core/src/config/otel.rs`) read directly to verify the client-key/first-party-domain pattern and the `analytics_enabled` opt-out gate
- **Evaluated and not chosen:** [Statsig Rust server SDK](https://docs.statsig.com/server-core/rust-core) (server secret, not used), [Statsig: Client vs Server SDKs](https://docs.statsig.com/sdks/client-vs-server), [Statsig: HTTP API overview](https://docs.statsig.com/http-api/overview), [Statsig: Log Custom Events](https://docs.statsig.com/api-reference/events/log-custom-events), [Statsig: Ingesting OpenTelemetry Data](https://docs.statsig.com/server/concepts/open_telemetry), [Statsig pricing](https://www.statsig.com/pricing) — kept for the historical record in this document's earlier revisions, not current guidance
- [PostHog](https://github.com/PostHog/posthog) — the chosen provider
- [PostHog: Capture and batch API endpoints](https://posthog.com/docs/api/capture) — `/i/v0/e/` endpoint, request shape, no-rate-limit/over-quota-200 behavior (direct fetch blocked by bot protection on every attempt; sourced from search-engine summaries of this page, flagged for primary-source re-verification)
- [PostHog: project vs personal API keys](https://posthog.com/docs/api) — `phc_...` public/write-only vs `phx_...` secret/admin, same confidence caveat as above
- [PostHog GeoIP plugin](https://github.com/PostHog/posthog-plugin-geoip) and related PRs (`posthog-ios#872`, `posthog#112342`, `posthog#113972`) — `$geoip_disable` mechanism, read directly from GitHub (not blocked)
- [Homebrew Anonymous Analytics](https://docs.brew.sh/Analytics)
- [OpenTelemetry Protocol](https://opentelemetry.io/docs/specs/otlp/)
- [OpenTelemetry semantic conventions](https://opentelemetry.io/docs/specs/semconv/)
- [Console Do Not Track](https://consoledonottrack.com/)
- [Go Telemetry](https://telemetry.go.dev/)
- [Aptabase](https://github.com/aptabase/aptabase)
- [Umami](https://github.com/umami-software/umami)
- [Sentry for Rust](https://docs.sentry.io/platforms/rust/)

Provider pricing, terms, retention, and API behavior change. The dated
provider decision record is a blocking engineering artifact, not procurement
cleanup. The Codex source excerpts above were read on 2026-09-19 and should be
re-checked for drift before implementation begins.
