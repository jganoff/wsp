// Deliberately faulty: an existing member is registered during a retry.
module register_before_member_check {
  var members: Set[str]
  var registry: Set[str]
  action init = all { members' = Set("alpha_api"), registry' = Set() }
  action faultyRetry = all { members' = members, registry' = Set("alpha_api") }
  action step = any { faultyRetry }
  val existingRetryHasNoGlobalEffect = registry == Set()
}
