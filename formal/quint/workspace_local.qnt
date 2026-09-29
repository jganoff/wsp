// Bounded ownership model for workspace-local wsp operations.
// See docs/reference/plans/2026-09-18-workspace-local-quint-model-design.md.
module workspace_local {
  pure val REPOS = Set("alpha_api", "beta_api", "alpha_web")
  pure val API_REPOS = Set("alpha_api", "beta_api")
  pure val SLOTS = Set("api", "beta-api", "web")
  pure val PROCESSES = Set("p0", "p1")

  // `members` belongs to the mounted root. The sibling has the same workspace
  // name in the host index, so it is represented separately instead of being
  // selected by name.
  var members: Set[str]
  var siblingMembers: Set[str]
  var cloneState: str -> str
  var mapping: str -> str
  var registry: Set[str]
  var mirrorState: str -> str
  var guidance: str
  var lastAction: str
  var lastWasLocal: bool
  var globalWrite: bool
  // Each invocation captures its own root and store capabilities. These are
  // volatile observations, never authority for a later invocation.
  var processRoot: str -> str
  var registryAccess: str -> str
  var mirrorAccess: str -> str
  var selectedTransport: str -> str
  var transportFailed: str -> bool
  // Volatile ownership of the removal sequence prevents a preflight for one
  // member from authorizing deletion or accounting for another member.
  var removalTarget: str

  action init = all {
    members' = Set(), siblingMembers' = Set(),
    cloneState' = REPOS.mapBy(_ => "absent"),
    mapping' = REPOS.mapBy(_ => ""),
    registry' = Set(),
    mirrorState' = REPOS.mapBy(_ => "absent"),
    guidance' = "fresh",
    lastAction' = "init",
    lastWasLocal' = false,
    globalWrite' = false,
    processRoot' = PROCESSES.mapBy(_ => "none"),
    registryAccess' = PROCESSES.mapBy(_ => "denied"),
    mirrorAccess' = PROCESSES.mapBy(_ => "denied"),
    selectedTransport' = PROCESSES.mapBy(_ => "none"),
    transportFailed' = PROCESSES.mapBy(_ => false),
    removalTarget' = "",
  }

  // Begin records an invocation-local snapshot. An isolated process can only
  // operate on the mounted root and cannot use either global store; a host
  // process may use them. Both slots may be active at once.
  action beginIsolated(process) = all {
    processRoot' = processRoot.set(process, "mounted"),
    registryAccess' = registryAccess.set(process, "denied"),
    mirrorAccess' = mirrorAccess.set(process, "denied"),
    selectedTransport' = selectedTransport.set(process, "none"),
    transportFailed' = transportFailed.set(process, false),
    members' = members, siblingMembers' = siblingMembers,
    cloneState' = cloneState, mapping' = mapping, registry' = registry,
    mirrorState' = mirrorState, guidance' = guidance,
    lastAction' = "begin_isolated", lastWasLocal' = true, globalWrite' = false,
    removalTarget' = removalTarget,
  }

  action beginHost(process) = all {
    processRoot' = processRoot.set(process, "mounted"),
    registryAccess' = registryAccess.set(process, "writable"),
    mirrorAccess' = mirrorAccess.set(process, "writable"),
    selectedTransport' = selectedTransport.set(process, "none"),
    transportFailed' = transportFailed.set(process, false),
    members' = members, siblingMembers' = siblingMembers,
    cloneState' = cloneState, mapping' = mapping, registry' = registry,
    mirrorState' = mirrorState, guidance' = guidance,
    lastAction' = "begin_host", lastWasLocal' = false, globalWrite' = false,
    removalTarget' = removalTarget,
  }

  // A denied or absent mirror selects direct origin. A valid usable mirror is
  // selected once; failure is terminal for this invocation.
  action selectDirect(process) = all {
    processRoot.get(process) == "mounted",
    mirrorAccess.get(process) == "denied" or mirrorState.get("alpha_api") == "absent",
    selectedTransport' = selectedTransport.set(process, "direct"),
    transportFailed' = transportFailed.set(process, false),
    processRoot' = processRoot, registryAccess' = registryAccess,
    mirrorAccess' = mirrorAccess,
    members' = members, siblingMembers' = siblingMembers,
    cloneState' = cloneState, mapping' = mapping, registry' = registry,
    mirrorState' = mirrorState, guidance' = guidance,
    lastAction' = "select_direct", lastWasLocal' = mirrorAccess.get(process) == "denied",
    globalWrite' = false,
    removalTarget' = removalTarget,
  }

  action selectMirror(process) = all {
    processRoot.get(process) == "mounted",
    mirrorAccess.get(process) == "writable",
    mirrorState.get("alpha_api") == "matching",
    selectedTransport' = selectedTransport.set(process, "mirror"),
    transportFailed' = transportFailed.set(process, false),
    processRoot' = processRoot, registryAccess' = registryAccess,
    mirrorAccess' = mirrorAccess,
    members' = members, siblingMembers' = siblingMembers,
    cloneState' = cloneState, mapping' = mapping, registry' = registry,
    mirrorState' = mirrorState, guidance' = guidance,
    lastAction' = "select_mirror", lastWasLocal' = false, globalWrite' = false,
    removalTarget' = removalTarget,
  }

  action mirrorFails(process) = all {
    selectedTransport.get(process) == "mirror",
    transportFailed' = transportFailed.set(process, true),
    processRoot' = processRoot, registryAccess' = registryAccess,
    mirrorAccess' = mirrorAccess, selectedTransport' = selectedTransport,
    members' = members, siblingMembers' = siblingMembers,
    cloneState' = cloneState, mapping' = mapping, registry' = registry,
    mirrorState' = mirrorState, guidance' = guidance,
    lastAction' = "mirror_failed", lastWasLocal' = false, globalWrite' = false,
    removalTarget' = removalTarget,
  }

  action finishBlocked(process) = all {
    selectedTransport.get(process) == "mirror", transportFailed.get(process),
    processRoot' = processRoot, registryAccess' = registryAccess,
    mirrorAccess' = mirrorAccess, selectedTransport' = selectedTransport,
    transportFailed' = transportFailed,
    members' = members, siblingMembers' = siblingMembers,
    cloneState' = cloneState, mapping' = mapping, registry' = registry,
    mirrorState' = mirrorState, guidance' = guidance,
    lastAction' = "transport_blocked", lastWasLocal' = false, globalWrite' = false,
    removalTarget' = removalTarget,
  }

  pure def slotFor(repo) =
    if (repo == "alpha_api") "api"
    else if (repo == "beta_api") "beta-api"
    else "web"

  pure def occupiedSlots() = members.map(repo => mapping.get(repo))

  // Isolated add has no global effect. Publication and membership are distinct
  // durable boundaries so a crash can leave a compatible orphan clone.
  action localPublish(repo) = all {
    not(members.contains(repo)),
    cloneState.get(repo) == "absent",
    cloneState' = cloneState.set(repo, "published"),
    members' = members, siblingMembers' = siblingMembers, mapping' = mapping, registry' = registry,
    mirrorState' = mirrorState, guidance' = guidance,
    lastAction' = "local_publish",
    lastWasLocal' = true,
    globalWrite' = false,
    processRoot' = processRoot, registryAccess' = registryAccess, mirrorAccess' = mirrorAccess,
    selectedTransport' = selectedTransport, transportFailed' = transportFailed,
    removalTarget' = removalTarget,
  }

  action localCommit(repo) = all {
    not(members.contains(repo)),
    cloneState.get(repo) == "published",
    not(occupiedSlots.contains(slotFor(repo))),
    members' = members.union(Set(repo)), siblingMembers' = siblingMembers,
    mapping' = mapping.set(repo, slotFor(repo)),
    guidance' = "stale",
    cloneState' = cloneState, registry' = registry, mirrorState' = mirrorState,
    lastAction' = "local_commit",
    lastWasLocal' = true,
    globalWrite' = false,
    processRoot' = processRoot, registryAccess' = registryAccess, mirrorAccess' = mirrorAccess,
    selectedTransport' = selectedTransport, transportFailed' = transportFailed,
    removalTarget' = removalTarget,
  }

  // A host add may create shared infrastructure only for a genuinely new
  // member. Existing membership is checked before this action is enabled.
  action hostPrepare(repo) = all {
    not(members.contains(repo)),
    not(registry.contains(repo)),
    registry' = registry.union(Set(repo)),
    mirrorState' = mirrorState.set(repo, "matching"),
    members' = members, siblingMembers' = siblingMembers, cloneState' = cloneState, mapping' = mapping,
    guidance' = guidance,
    lastAction' = "host_prepare",
    lastWasLocal' = false,
    globalWrite' = true,
    processRoot' = processRoot, registryAccess' = registryAccess, mirrorAccess' = mirrorAccess,
    selectedTransport' = selectedTransport, transportFailed' = transportFailed,
    removalTarget' = removalTarget,
  }

  action hostPublish(repo) = all {
    not(members.contains(repo)),
    cloneState.get(repo) == "absent",
    cloneState' = cloneState.set(repo, "published"),
    members' = members, siblingMembers' = siblingMembers, mapping' = mapping, registry' = registry,
    mirrorState' = mirrorState, guidance' = guidance,
    lastAction' = "host_publish",
    lastWasLocal' = false,
    globalWrite' = false,
    processRoot' = processRoot, registryAccess' = registryAccess, mirrorAccess' = mirrorAccess,
    selectedTransport' = selectedTransport, transportFailed' = transportFailed,
    removalTarget' = removalTarget,
  }

  action hostCommit(repo) = all {
    not(members.contains(repo)),
    cloneState.get(repo) == "published",
    not(occupiedSlots.contains(slotFor(repo))),
    members' = members.union(Set(repo)), siblingMembers' = siblingMembers,
    mapping' = mapping.set(repo, slotFor(repo)),
    guidance' = "stale",
    cloneState' = cloneState, registry' = registry, mirrorState' = mirrorState,
    lastAction' = "host_commit",
    lastWasLocal' = false,
    globalWrite' = false,
    processRoot' = processRoot, registryAccess' = registryAccess, mirrorAccess' = mirrorAccess,
    selectedTransport' = selectedTransport, transportFailed' = transportFailed,
    removalTarget' = removalTarget,
  }

  // Existing-member retry and explicit registration preserve workspace state.
  action retryExisting(repo) = all {
    members.contains(repo),
    members' = members, siblingMembers' = siblingMembers, cloneState' = cloneState, mapping' = mapping,
    registry' = registry, mirrorState' = mirrorState, guidance' = guidance,
    lastAction' = "retry_existing",
    lastWasLocal' = false,
    globalWrite' = false,
    processRoot' = processRoot, registryAccess' = registryAccess, mirrorAccess' = mirrorAccess,
    selectedTransport' = selectedTransport, transportFailed' = transportFailed,
    removalTarget' = removalTarget,
  }

  action register(repo) = all {
    members.contains(repo),
    not(registry.contains(repo)),
    registry' = registry.union(Set(repo)),
    mirrorState' = mirrorState.set(repo, "matching"),
    members' = members, siblingMembers' = siblingMembers, cloneState' = cloneState, mapping' = mapping,
    guidance' = guidance,
    lastAction' = "register",
    lastWasLocal' = false,
    globalWrite' = true,
    processRoot' = processRoot, registryAccess' = registryAccess, mirrorAccess' = mirrorAccess,
    selectedTransport' = selectedTransport, transportFailed' = transportFailed,
    removalTarget' = removalTarget,
  }

  action generateGuidance = all {
    guidance' = "fresh",
    members' = members, siblingMembers' = siblingMembers, cloneState' = cloneState, mapping' = mapping,
    registry' = registry, mirrorState' = mirrorState,
    lastAction' = "guidance",
    lastWasLocal' = false,
    globalWrite' = false,
    processRoot' = processRoot, registryAccess' = registryAccess, mirrorAccess' = mirrorAccess,
    selectedTransport' = selectedTransport, transportFailed' = transportFailed,
    removalTarget' = removalTarget,
  }

  // Removal is deliberately split at every persistent boundary.  `dirty`
  // represents a complete but unsafe clone; `damaged` represents a failed or
  // partial deletion; and `absent` with a member represents a crash after a
  // successful deletion before the manifest replacement.  None of those
  // states grants a non-forced deletion.
  action makeDirty(repo) = all {
    members.contains(repo), cloneState.get(repo) == "published",
    cloneState' = cloneState.set(repo, "dirty"),
    members' = members, siblingMembers' = siblingMembers, mapping' = mapping,
    registry' = registry, mirrorState' = mirrorState, guidance' = guidance,
    lastAction' = "external_dirty", lastWasLocal' = false, globalWrite' = false,
    processRoot' = processRoot, registryAccess' = registryAccess, mirrorAccess' = mirrorAccess,
    selectedTransport' = selectedTransport, transportFailed' = transportFailed,
    removalTarget' = removalTarget,
  }

  action preflightSafeRemove(repo) = all {
    members.contains(repo), cloneState.get(repo) == "published",
    members' = members, siblingMembers' = siblingMembers, cloneState' = cloneState, mapping' = mapping,
    registry' = registry, mirrorState' = mirrorState, guidance' = guidance,
    lastAction' = "remove_preflight_safe", lastWasLocal' = false, globalWrite' = false,
    processRoot' = processRoot, registryAccess' = registryAccess, mirrorAccess' = mirrorAccess,
    selectedTransport' = selectedTransport, transportFailed' = transportFailed,
    removalTarget' = repo,
  }

  action preflightForcedRemove(repo) = all {
    members.contains(repo),
    cloneState.get(repo) == "published" or cloneState.get(repo) == "dirty" or cloneState.get(repo) == "absent",
    members' = members, siblingMembers' = siblingMembers, cloneState' = cloneState, mapping' = mapping,
    registry' = registry, mirrorState' = mirrorState, guidance' = guidance,
    lastAction' = "remove_preflight_forced", lastWasLocal' = false, globalWrite' = false,
    processRoot' = processRoot, registryAccess' = registryAccess, mirrorAccess' = mirrorAccess,
    selectedTransport' = selectedTransport, transportFailed' = transportFailed,
    removalTarget' = repo,
  }

  action removeBlocked(repo) = all {
    members.contains(repo), cloneState.get(repo) == "dirty",
    members' = members, siblingMembers' = siblingMembers, cloneState' = cloneState, mapping' = mapping,
    registry' = registry, mirrorState' = mirrorState, guidance' = guidance,
    lastAction' = "remove_blocked", lastWasLocal' = false, globalWrite' = false,
    processRoot' = processRoot, registryAccess' = registryAccess, mirrorAccess' = mirrorAccess,
    selectedTransport' = selectedTransport, transportFailed' = transportFailed,
    removalTarget' = removalTarget,
  }

  action deleteClone(repo) = all {
    members.contains(repo), removalTarget == repo,
    lastAction == "remove_preflight_safe" or lastAction == "remove_preflight_forced",
    cloneState.get(repo) == "published" or cloneState.get(repo) == "dirty",
    cloneState' = cloneState.set(repo, "absent"),
    members' = members, siblingMembers' = siblingMembers, mapping' = mapping,
    registry' = registry, mirrorState' = mirrorState, guidance' = guidance,
    lastAction' = "clone_deleted", lastWasLocal' = false, globalWrite' = false,
    processRoot' = processRoot, registryAccess' = registryAccess, mirrorAccess' = mirrorAccess,
    selectedTransport' = selectedTransport, transportFailed' = transportFailed,
    removalTarget' = removalTarget,
  }

  action deleteFails(repo) = all {
    members.contains(repo), removalTarget == repo,
    lastAction == "remove_preflight_safe" or lastAction == "remove_preflight_forced",
    cloneState.get(repo) == "published" or cloneState.get(repo) == "dirty",
    members' = members, siblingMembers' = siblingMembers, cloneState' = cloneState, mapping' = mapping,
    registry' = registry, mirrorState' = mirrorState, guidance' = guidance,
    lastAction' = "delete_failed", lastWasLocal' = false, globalWrite' = false,
    processRoot' = processRoot, registryAccess' = registryAccess, mirrorAccess' = mirrorAccess,
    selectedTransport' = selectedTransport, transportFailed' = transportFailed,
    removalTarget' = removalTarget,
  }

  action deletePartially(repo) = all {
    members.contains(repo), removalTarget == repo,
    lastAction == "remove_preflight_safe" or lastAction == "remove_preflight_forced",
    cloneState.get(repo) == "published" or cloneState.get(repo) == "dirty",
    cloneState' = cloneState.set(repo, "damaged"),
    members' = members, siblingMembers' = siblingMembers, mapping' = mapping,
    registry' = registry, mirrorState' = mirrorState, guidance' = guidance,
    lastAction' = "delete_partial", lastWasLocal' = false, globalWrite' = false,
    processRoot' = processRoot, registryAccess' = registryAccess, mirrorAccess' = mirrorAccess,
    selectedTransport' = selectedTransport, transportFailed' = transportFailed,
    removalTarget' = removalTarget,
  }

  // An owned missing directory can be cleared only after an explicit forced
  // recheck. It is a successful accounting path without a deletion receipt.
  action forceConfirmMissing(repo) = all {
    members.contains(repo), removalTarget == repo, lastAction == "remove_preflight_forced",
    cloneState.get(repo) == "absent",
    members' = members, siblingMembers' = siblingMembers, cloneState' = cloneState, mapping' = mapping,
    registry' = registry, mirrorState' = mirrorState, guidance' = guidance,
    lastAction' = "missing_clone_confirmed", lastWasLocal' = false, globalWrite' = false,
    processRoot' = processRoot, registryAccess' = registryAccess, mirrorAccess' = mirrorAccess,
    selectedTransport' = selectedTransport, transportFailed' = transportFailed,
    removalTarget' = removalTarget,
  }

  action commitRemoval(repo) = all {
    members.contains(repo), removalTarget == repo,
    lastAction == "clone_deleted" or lastAction == "missing_clone_confirmed",
    cloneState.get(repo) == "absent",
    members' = members.exclude(Set(repo)), siblingMembers' = siblingMembers,
    cloneState' = cloneState, mapping' = mapping.set(repo, ""),
    guidance' = "stale", registry' = registry, mirrorState' = mirrorState,
    lastAction' = "removal_committed", lastWasLocal' = false, globalWrite' = false,
    processRoot' = processRoot, registryAccess' = registryAccess, mirrorAccess' = mirrorAccess,
    selectedTransport' = selectedTransport, transportFailed' = transportFailed,
    removalTarget' = removalTarget,
  }

  // Model an interruption after clone publication. No dead-process provenance
  // is retained; a later invocation may adopt the compatible clone.
  action crashAfterPublish(repo) = all {
    cloneState.get(repo) == "published",
    not(members.contains(repo)),
    members' = members, siblingMembers' = siblingMembers, cloneState' = cloneState, mapping' = mapping,
    registry' = registry, mirrorState' = mirrorState, guidance' = guidance,
    lastAction' = "crash_after_publish",
    lastWasLocal' = false,
    globalWrite' = false,
    processRoot' = processRoot, registryAccess' = registryAccess, mirrorAccess' = mirrorAccess,
    selectedTransport' = selectedTransport, transportFailed' = transportFailed,
    removalTarget' = removalTarget,
  }

  // Process death releases volatile state but does not repair a successfully
  // deleted directory or rewrite metadata. A fresh forced invocation is the
  // only modeled recovery for this state.
  action crashAfterDelete(repo) = all {
    members.contains(repo), removalTarget == repo, cloneState.get(repo) == "absent", lastAction == "clone_deleted",
    members' = members, siblingMembers' = siblingMembers, cloneState' = cloneState, mapping' = mapping,
    registry' = registry, mirrorState' = mirrorState, guidance' = guidance,
    lastAction' = "crash_after_delete", lastWasLocal' = false, globalWrite' = false,
    processRoot' = PROCESSES.mapBy(_ => "none"),
    registryAccess' = PROCESSES.mapBy(_ => "denied"), mirrorAccess' = PROCESSES.mapBy(_ => "denied"),
    selectedTransport' = PROCESSES.mapBy(_ => "none"), transportFailed' = PROCESSES.mapBy(_ => false),
    removalTarget' = "",
  }

  // Removal metadata is already committed at this boundary. A later crash may
  // leave derived guidance stale, but must never resurrect the member.
  action crashAfterRemovalCommit(repo) = all {
    not(members.contains(repo)), cloneState.get(repo) == "absent", lastAction == "removal_committed",
    members' = members, siblingMembers' = siblingMembers, cloneState' = cloneState, mapping' = mapping,
    registry' = registry, mirrorState' = mirrorState, guidance' = guidance,
    lastAction' = "crash_after_removal_commit", lastWasLocal' = false, globalWrite' = false,
    processRoot' = PROCESSES.mapBy(_ => "none"),
    registryAccess' = PROCESSES.mapBy(_ => "denied"), mirrorAccess' = PROCESSES.mapBy(_ => "denied"),
    selectedTransport' = PROCESSES.mapBy(_ => "none"), transportFailed' = PROCESSES.mapBy(_ => false),
    removalTarget' = "",
  }

  action adopt(repo) = all {
    not(members.contains(repo)),
    cloneState.get(repo) == "published",
    not(occupiedSlots.contains(slotFor(repo))),
    members' = members.union(Set(repo)), siblingMembers' = siblingMembers,
    mapping' = mapping.set(repo, slotFor(repo)),
    guidance' = "stale",
    cloneState' = cloneState, registry' = registry, mirrorState' = mirrorState,
    lastAction' = "adopt",
    lastWasLocal' = false,
    globalWrite' = false,
    processRoot' = processRoot, registryAccess' = registryAccess, mirrorAccess' = mirrorAccess,
    selectedTransport' = selectedTransport, transportFailed' = transportFailed,
    removalTarget' = removalTarget,
  }

  action step = {
    nondet repo = REPOS.oneOf()
    any {
      localPublish(repo), localCommit(repo), hostPrepare(repo), hostPublish(repo),
      hostCommit(repo), retryExisting(repo), register(repo), generateGuidance,
      makeDirty(repo), preflightSafeRemove(repo), preflightForcedRemove(repo), removeBlocked(repo),
      deleteClone(repo), deleteFails(repo), deletePartially(repo), forceConfirmMissing(repo),
      commitRemoval(repo), crashAfterPublish(repo), crashAfterDelete(repo), crashAfterRemovalCommit(repo), adopt(repo),
      beginIsolated("p0"), beginIsolated("p1"), beginHost("p0"), beginHost("p1"),
      selectDirect("p0"), selectDirect("p1"), selectMirror("p0"), selectMirror("p1"),
      mirrorFails("p0"), mirrorFails("p1"), finishBlocked("p0"), finishBlocked("p1"),
    }
  }

  // Membership intentionally outlives a clone after an interrupted removal.
  // The invariant is that valid manifest entries retain an allocated slot;
  // clone validation is an operation precondition, not a universal fact.
  val validMembers = members.forall(repo => mapping.get(repo) != "")
  val uniqueMappings = members.forall(repo => members.forall(other => repo == other or mapping.get(repo) != mapping.get(other)))
  val localConfinement = not(lastWasLocal) or not(globalWrite)
  val mappingIsAllocated = members.forall(repo => mapping.get(repo) == slotFor(repo))
  val registryDoesNotOwnMembership = registry.subseteq(REPOS)
  val mountedInvocationLeavesSiblingUntouched = siblingMembers == Set()
  val noTransportRetry = PROCESSES.forall(process =>
    not(transportFailed.get(process) and selectedTransport.get(process) == "direct"))
  // `commitRemoval` is the sole membership-clearing action and its guard
  // requires either `clone_deleted` or `missing_clone_confirmed`. The active
  // target makes that accounting assertion specific even with two repos.
  val removalAccounting = lastAction != "removal_committed" or all {
    removalTarget != "",
    not(members.contains(removalTarget)),
    mapping.get(removalTarget) == "",
    cloneState.get(removalTarget) == "absent",
  }

  // Named journeys are intentionally deterministic. They complement randomized
  // simulation by making the contract at each persistence boundary reviewable.
  run isolated_add_host_retry_and_registration = init
    .then(localPublish("alpha_api"))
    .then(localCommit("alpha_api"))
    .expect(members == Set("alpha_api"))
    .expect(registry == Set())
    .then(retryExisting("alpha_api"))
    .expect(registry == Set())
    .then(register("alpha_api"))
    .expect(registry == Set("alpha_api"))
    .then(retryExisting("alpha_api"))
    .expect(mapping.get("alpha_api") == "api")

  run published_clone_is_adopted_without_registration = init
    .then(localPublish("beta_api"))
    .then(crashAfterPublish("beta_api"))
    .then(adopt("beta_api"))
    .expect(members == Set("beta_api"))
    .expect(registry == Set())
    .expect(mapping.get("beta_api") == "beta-api")

  run colliding_basenames_keep_distinct_stable_slots = init
    .then(localPublish("alpha_api"))
    .then(localCommit("alpha_api"))
    .then(localPublish("beta_api"))
    .then(localCommit("beta_api"))
    .expect(members == API_REPOS)
    .expect(mapping.get("alpha_api") == "api")
    .expect(mapping.get("beta_api") == "beta-api")
    .then(generateGuidance)
    .expect(guidance == "fresh")

  run removal_does_not_unregister_a_member = init
    .then(hostPrepare("alpha_web"))
    .then(hostPublish("alpha_web"))
    .then(hostCommit("alpha_web"))
    .then(preflightSafeRemove("alpha_web"))
    .then(deleteClone("alpha_web"))
    .then(commitRemoval("alpha_web"))
    .expect(not(members.contains("alpha_web")))
    .expect(registry.contains("alpha_web"))

  run unforced_dirty_removal_is_blocked_and_preserves_membership = init
    .then(localPublish("alpha_api"))
    .then(localCommit("alpha_api"))
    .then(makeDirty("alpha_api"))
    .then(removeBlocked("alpha_api"))
    .expect(members == Set("alpha_api"))
    .expect(cloneState.get("alpha_api") == "dirty")

  run force_removal_deletes_an_unsafe_owned_clone = init
    .then(localPublish("alpha_api"))
    .then(localCommit("alpha_api"))
    .then(makeDirty("alpha_api"))
    .then(preflightForcedRemove("alpha_api"))
    .then(deleteClone("alpha_api"))
    .then(commitRemoval("alpha_api"))
    .expect(not(members.contains("alpha_api")))
    .expect(cloneState.get("alpha_api") == "absent")

  run failed_or_partial_deletion_keeps_membership = init
    .then(localPublish("alpha_api"))
    .then(localCommit("alpha_api"))
    .then(preflightSafeRemove("alpha_api"))
    .then(deleteFails("alpha_api"))
    .expect(members == Set("alpha_api"))
    .then(preflightForcedRemove("alpha_api"))
    .then(deletePartially("alpha_api"))
    .expect(members == Set("alpha_api"))
    .expect(cloneState.get("alpha_api") == "damaged")

  run crashed_removal_requires_fresh_forced_missing_confirmation = init
    .then(localPublish("alpha_api"))
    .then(localCommit("alpha_api"))
    .then(preflightSafeRemove("alpha_api"))
    .then(deleteClone("alpha_api"))
    .then(crashAfterDelete("alpha_api"))
    .expect(members == Set("alpha_api"))
    .expect(cloneState.get("alpha_api") == "absent")
    .then(beginHost("p0"))
    .then(preflightForcedRemove("alpha_api"))
    .then(forceConfirmMissing("alpha_api"))
    .then(commitRemoval("alpha_api"))
    .expect(not(members.contains("alpha_api")))

  run crash_after_removal_commit_preserves_removal_and_allows_guidance_repair = init
    .then(localPublish("alpha_api"))
    .then(localCommit("alpha_api"))
    .then(preflightSafeRemove("alpha_api"))
    .then(deleteClone("alpha_api"))
    .then(commitRemoval("alpha_api"))
    .then(crashAfterRemovalCommit("alpha_api"))
    .expect(not(members.contains("alpha_api")))
    .expect(guidance == "stale")
    .then(beginHost("p0"))
    .then(generateGuidance)
    .expect(guidance == "fresh")

  run two_contexts_keep_the_mounted_root_and_transport_policy = init
    .then(beginIsolated("p0"))
    .then(selectDirect("p0"))
    .expect(processRoot.get("p0") == "mounted")
    .expect(selectedTransport.get("p0") == "direct")
    .expect(siblingMembers == Set())
    .then(hostPrepare("alpha_api"))
    .then(beginHost("p1"))
    .then(selectMirror("p1"))
    .then(mirrorFails("p1"))
    .then(finishBlocked("p1"))
    .expect(selectedTransport.get("p1") == "mirror")
    .expect(transportFailed.get("p1"))
    .expect(noTransportRetry)
}
