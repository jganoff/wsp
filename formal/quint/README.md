# Workspace-local ownership model

This directory contains the executable bounded Quint model described in
[`docs/reference/plans/2026-09-18-workspace-local-quint-model-design.md`](../../docs/reference/plans/2026-09-18-workspace-local-quint-model-design.md).

The normative model in `workspace_local.qnt` covers membership, fixed
per-identity slot assignments, isolated publication, host registration, existing-member
retry, interrupted publication and adoption, guidance, and split removal.
Removal distinguishes safe preflight, a dirty-clone block, force deletion,
pre-delete failure, partial deletion, and forced confirmation of an owned
missing clone. A crash after a successful deletion preserves membership with
an absent clone; a fresh forced invocation can then confirm the missing owned
path and commit removal. A crash after removal metadata commits preserves that
removal and permits later guidance repair. It
also has two independently captured process slots: isolated invocation denies
registry and mirror access and selects direct origin, while host invocation may
select a matching mirror. A mirror failure stays selected and blocked; the
normative model has no direct-fallback transition. The mounted root's members
and same-name sibling members are separate persistent state, and every modeled
mounted operation preserves the sibling. Its finite identities include two
`api` repositories, so the named journey checks that two same-basename
identities retain their distinct, predetermined mappings. It is not an
allocator model: it does not enumerate candidate names, represent occupied
filesystem destinations, or explore competing allocation choices.

`cargo xtask quint`, reached through `just quint-check`, runs all of the
following with Quint pinned to `@informalsystems/quint@0.32.0`:

- typechecking the normative model and every standalone faulty variant;
- ten named deterministic journeys: isolated add with host retry and explicit
  registration, interrupted add with adoption, same-basename fixed mappings, removal
  that preserves registration, unforced dirty removal, forced removal,
  failed/partial deletion, crashed deletion with forced recovery, crash after
  removal commit with guidance repair, and
  simultaneous isolated/host transport selection after a mirror failure;
- 200 deterministic randomized 60-step executions checked against membership,
  fixed-mapping, local-confinement, sibling isolation, registry-domain, and
  no-transport-retry invariants;
- eleven expected counterexamples for small, standalone faulty variants:
  registration before existing-member inspection, unstable mapping, occupied
  publication, stale metadata, failed-deletion accounting, missing-clone
  clearing without force, force clearing foreign ownership, host-name routing,
  transport fallback, setup replay on adoption, and stale guidance. Each
  mutation command must select its named invariant and Quint must emit `error:
  Invariant violated`; an argument, typecheck, or runner error is rejected. A
  green mutation means the task itself fails. These variants verify that the
  Quint runner and the named invariant can expose the simplified fault they
  contain. They are not mutations of the normative state machine and therefore
  do not establish mutation sensitivity for its allocation, publication, or
  concurrency behavior.

```bash
just quint-check
just quint-traces
```

`quint-traces` writes one raw ITF trace from deterministic bounded simulation to
`target/quint-traces/`; these are generated artifacts and are not committed.
[`trace.schema.json`](trace.schema.json) defines the versioned normalized
product-trace envelope from the design. There is deliberately no adapter from
raw ITF to that envelope yet. The crash-barrier harness currently covers only
a subset of the modelled boundaries, so treating raw traces as passing
implementation replays would be misleading.

The model is a bounded safety oracle. It uses direct state transitions rather
than a complete per-process command/lock state machine. It does not yet model
per-store `ReadOnly`/`Unknown` capability variants, malformed global configuration,
process-owned staging, lock ownership, candidate allocation, occupied or
foreign filesystem destinations, partial batch removal, a foreign-path
replacement race, or a host-index transition that actually resolves the same
name to the sibling. Its crash transitions model completed filesystem writes
and process-state loss at publication, deletion, and removal-commit boundaries;
they do not replay the barrier protocol, model mid-Git children, or prove
power-loss durability. It does not prove filesystem
confinement, Git object/ref integrity, ACL behavior, or conformance of the Rust
binary. Those remain real-binary integration and platform-test obligations
described in the design document.
