// Deliberately faulty: failed deletion still removes membership.
module clear_after_failed_delete {
  var members: Set[str]
  var deleted: bool
  action init = all { members' = Set("alpha_api"), deleted' = false }
  action faultyRemove = all { members' = Set(), deleted' = false }
  action step = any { faultyRemove }
  val failedDeleteKeepsMembership = deleted or members.contains("alpha_api")
}
