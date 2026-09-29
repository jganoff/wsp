// Deliberately faulty: adopting a clone replays host setup.
module replay_setup_on_adoption {
  var adopted: bool
  var setupRan: bool
  action init = all { adopted' = false, setupRan' = false }
  action faultyAdopt = all { adopted' = true, setupRan' = true }
  action step = any { faultyAdopt }
  val adoptionDoesNotReplaySetup = not(adopted and setupRan)
}
