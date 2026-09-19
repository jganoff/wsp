// Deliberately faulty: clone publication replaces a foreign destination.
module overwrite_occupied_publication {
  var slotOwner: str
  action init = slotOwner' = "foreign"
  action faultyPublish = slotOwner' = "alpha_api"
  action step = any { faultyPublish }
  val foreignDestinationPreserved = slotOwner == "foreign"
}
