// Deliberately faulty: same-name routing targets the host-indexed sibling.
module route_by_host_name {
  var mountedMembers: Set[str]
  var siblingMembers: Set[str]
  action init = all { mountedMembers' = Set(), siblingMembers' = Set() }
  action faultyMountedAdd = all {
    mountedMembers' = mountedMembers, siblingMembers' = Set("alpha_api")
  }
  action step = any { faultyMountedAdd }
  val mountedInvocationLeavesSiblingUntouched = siblingMembers == Set()
}
