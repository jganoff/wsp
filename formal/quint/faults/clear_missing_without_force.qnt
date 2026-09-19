// Deliberately faulty: an interrupted removal clears metadata without the
// explicit forced confirmation required for an owned missing clone.
module clear_missing_without_force {
  var members: Set[str]
  var clone: str
  var forcedConfirmation: bool

  action init = all {
    members' = Set("alpha_api"), clone' = "absent", forcedConfirmation' = false
  }

  action faultyCommitRemoval = all {
    members' = Set(), clone' = clone, forcedConfirmation' = forcedConfirmation
  }

  action step = any { faultyCommitRemoval }

  val missingCloneNeedsForcedConfirmation =
    forcedConfirmation or members.contains("alpha_api")
}
