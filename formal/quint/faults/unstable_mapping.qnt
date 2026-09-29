// Deliberately faulty: a retry recomputes an existing member's directory.
module unstable_mapping {
  var members: Set[str]
  var mapping: str -> str
  var previousMapping: str

  action init = all {
    members' = Set("alpha_api"),
    mapping' = Set("alpha_api").mapBy(_ => "api"),
    previousMapping' = "api",
  }

  action faultyRetry = all {
    members' = members,
    mapping' = mapping.set("alpha_api", "api-2"),
    previousMapping' = mapping.get("alpha_api"),
  }

  action step = any { faultyRetry }

  val stableMapping = mapping.get("alpha_api") == previousMapping
}
