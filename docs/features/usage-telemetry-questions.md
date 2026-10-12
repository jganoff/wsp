# Requirements → Telemetry Traceability

Status: proposal, companion to [`usage-telemetry.md`](usage-telemetry.md)

This page exists so "what question are we trying to answer" and "what field
answers it" never drift apart. Every field in the schema should trace back to
a real question below; every question below should name the exact field(s)
that answer it and what the analytics platform actually has to do to make
that answer self-serve. If a field can't be traced to a question, or a
question has no field, that's a bug in one document or the other.

## A capability check that shapes this whole page

**Revised 2026-10-12 — provider switched from Statsig to PostHog**, after
this page's original version documented real ambiguity in Statsig's free
tier (an unverifiable 4-dimension cap on persistent "Custom Metrics," and an
undocumented line between free "analytics" and a paid "advanced analytics"
tier that our actual needs might have fallen on the wrong side of). See
`usage-telemetry.md`'s "Provider Decision" section for the full comparison.
Every "Statsig mechanism" column in this page's previous version is replaced
below.

From PostHog's own documentation (summarized here — **direct verification is
still needed**; every attempt to fetch `posthog.com/docs` pages directly was
blocked by their bot protection, same as it was for Statsig, so this is from
consistent secondary sources, not a page we read ourselves):

- **Insights** (the standard breakdown/trends UI) lets you group by any event
  property, with its own minor display limit (around 10 distinct values
  shown per breakdown) — a truncation, not a dimension-count cap.
- **HogQL** — real SQL running directly over PostHog's hosted event store
  (ClickHouse), confirmed included in the free tier, not gated — lets you
  combine as many properties as a query needs (e.g. `concat()` for a
  multi-property breakdown), with no small fixed dimension limit analogous
  to Statsig's. It also has documented functions for array-valued properties
  (`JSONExtractArrayRaw`, `arrayJoin`), which is the direct answer to this
  page's one previously-unresolved question (#10, below).
- Event payload: PostHog's capture endpoint (`/i/v0/e/`) takes `api_key`,
  `event`, `distinct_id`, and a `properties` object — no documented
  per-request size limit as small as Statsig's reported 64-character string
  fields was; still worth a real test rather than an assumption, same as any
  other claim on this page.

**Practical effect on this design:** both ad hoc one-off questions and
persistent dashboards should be answerable with full flexibility via HogQL,
removing the "pick your top 3" compromise the Statsig version of this page
described as an accepted limitation. The remaining risk is the same shape as
before: secondhand documentation, not a page we read ourselves.

**Recommended next action, now that a real PostHog project exists:** send a
small batch of synthetic test events covering a representative spread of
field values (via the project API key, using `/i/v0/e/` directly — no code
needed, just `curl`, including `$geoip_disable: true` on every event), then
confirm directly in the PostHog console: does a HogQL query group cleanly by
every field below, does `arrayJoin`/`JSONExtractArrayRaw` actually handle
`failed_checks`'s list value the way documentation suggests, and does
`$geoip_disable` visibly suppress location data on the received event. This
replaces every "per secondary sources" caveat on this page with a verified
fact, cheaply, before any Rust code gets written around assumptions that
might be wrong.

## Traceability table

| # | Question | Why it matters | Field(s) | PostHog mechanism | Confidence |
|---|---|---|---|---|---|
| 1 | Which commands/features are used, and by how many distinct installations? | The core "who's using this for what" the whole project started from | `command`, `feature`, Install ID (`distinct_id`) | Insights or HogQL, grouped by `command`/`feature`, counted by distinct `distinct_id` | High — basic grouping/distinct-count is core to any event platform |
| 2 | Which versions, OS/arch, and install channels are represented? | Prioritize packaging/testing investment | `wsp_version`, `os`, `arch`, `install_channel` | Insights or HogQL — four properties, trivial either way | High |
| 3 | What fraction of commands fail, and in which broad category? | Find real friction without reading error text | `outcome`, `failure_class` | HogQL, filtered on `outcome=observed_failure`, grouped by `failure_class` | Medium — `failure_class` has no classifier built yet (see `usage-telemetry.md`); the field's existence doesn't guarantee good category coverage until that's implemented and reviewed |
| 4 | What's the duration distribution for completed commands? | Find real slowness, not anecdotes | `duration_bucket` | Insights or HogQL, grouped by `duration_bucket` | High |
| 5 | Which repository-count buckets accompany relevant commands? | Does wsp work well at the workspace sizes people actually have? | `repo_count_bucket` | Insights or HogQL | High |
| 6 | Roughly how long have reporting installations had analytics on? | A retention signal, without a client-transmitted timestamp | *(none — deliberately not a field)* | HogQL, computing min/max arrival timestamp grouped by `distinct_id` — a native SQL `GROUP BY` + `MIN()`/`MAX()`, not a special feature to locate | High — this is now a basic SQL query rather than an assumption about console capability, which is a meaningfully better position than the Statsig version of this row was in |
| 7 | How much usage is agent-driven? | wsp's own stated mission treats agent use as first-class, not an edge case | `agent_context` | Insights or HogQL, grouped by `agent_context` | High for the field; **known limited scope** — only detects wsp's own agent-sandbox mode, undercounts agents running with full host access (documented, accepted trade-off, not a gap in this table) |
| 8 | Which shells should completion-script investment prioritize? | `wsp completion` only supports zsh/bash/fish today | `shell_bucket` | Insights or HogQL, grouped by `shell_bucket` | High |
| 9 | Which `wsp new` creation path do people actually use? | Template vs. file vs. scratch vs. implicit-derive adoption | `feature` (six values, `new.rs`-specific) | Insights or HogQL, filtered to `command=new`, grouped by `feature` | High |
| 10 | Which `doctor` checks fire most in real installs? | Prioritize which invariant to harden/auto-fix first, per `AGENTS.md`'s own requirement | `failed_checks` | HogQL with `arrayJoin(properties.failed_checks)` to expand the list into one row per check before grouping — PostHog documents this exact function for array-valued properties | Medium — the field itself is confirmed implementable (see `usage-telemetry.md`); the specific `arrayJoin` approach is documented but not yet run against a real `failed_checks` payload |
| 11 | Is "distinct installations using command X" real developer usage, or dominated by one org's ephemeral CI fleet? | Required segmentation per `usage-telemetry.md`'s governance rule | `interactive`, `json` | Insights or HogQL, always split by `interactive` before reading a "distinct installations" number | High for the field; **process risk, not a data risk** — this only works if whoever reads the dashboard actually applies the split every time, which is a discipline requirement on the governance side, not something the schema can enforce |
| 12 | Is prerelease (`-rc.N`) traffic showing up during dogfood before a stable release? | Confirms dogfood is actually exercising the build, before defaulting a release to everyone | `wsp_version` (its own suffix), `release_stage` (`tagged`/`untagged`) | HogQL, filtered on `wsp_version` containing `-rc.` | High |

## Orphan check (fields with no question above)

Every field in `usage-telemetry.md`'s "Fixed values" table maps to one of the
twelve questions above, except `schema_version`, which isn't a signal at all
— it's a compatibility mechanism (lets the schema change without silently
mixing incompatible data), not something anyone queries by. No field exists
without a reason; no question above lacks a field. If a future field doesn't
fit a row in this table, that's a reason to ask why it's being added before
it ships, not after.

## What this still can't answer

Unchanged from `usage-telemetry.md`'s "Product Questions and Limits" — who a
person/org/repo is, exact active users or a full adoption funnel, behavior of
opted-out users, crash/`SIGKILL` rates, exact end-to-end latency, security or
individual-developer performance, or whether low usage makes a feature safe
to remove. Restated here only so this traceability page doesn't imply
completeness it doesn't have.
