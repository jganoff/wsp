// Deliberately faulty: a stale metadata write loses a peer member.
module stale_metadata_snapshot {
  var members: Set[str]
  action init = members' = Set("alpha_api")
  action faultyCommit = members' = Set("beta_api")
  action step = any { faultyCommit }
  val peerMemberPreserved = members.contains("alpha_api")
}
