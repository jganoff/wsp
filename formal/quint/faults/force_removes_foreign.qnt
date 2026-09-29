// Deliberately faulty: --force treats a foreign path as an owned missing clone.
module force_removes_foreign {
  var members: Set[str]
  var directory: str
  var forced: bool

  action init = all {
    members' = Set("alpha_api"), directory' = "foreign", forced' = true
  }

  action faultyForceCommit = all {
    members' = Set(), directory' = directory, forced' = forced
  }

  action step = any { faultyForceCommit }

  val forceNeverClearsForeignOwnership =
    directory != "foreign" or members.contains("alpha_api")
}
