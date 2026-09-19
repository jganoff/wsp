// Deliberately faulty: guidance is written from a stale manifest snapshot.
module stale_guidance_snapshot {
  var members: Set[str]
  var guidanceMembers: Set[str]
  action init = all { members' = Set("alpha_api"), guidanceMembers' = Set("alpha_api") }
  action faultyCommitAndGuidance = all {
    members' = Set("alpha_api", "beta_api"), guidanceMembers' = guidanceMembers
  }
  action step = any { faultyCommitAndGuidance }
  val guidanceMatchesLatestMetadata = guidanceMembers == members
}
