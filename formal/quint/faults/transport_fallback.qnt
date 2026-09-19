// Deliberately faulty: a failed mirror refresh falls back to direct origin.
module transport_fallback {
  var selected: str
  var failedSelected: bool
  var usedDirectAfterMirrorFailure: bool

  action init = all {
    selected' = "none", failedSelected' = false,
    usedDirectAfterMirrorFailure' = false,
  }

  action selectMirror = all {
    selected' = "mirror", failedSelected' = false,
    usedDirectAfterMirrorFailure' = false,
  }

  action mirrorFails = all {
    selected == "mirror",
    selected' = selected, failedSelected' = true,
    usedDirectAfterMirrorFailure' = false,
  }

  action faultyDirectFallback = all {
    selected == "mirror",
    failedSelected,
    selected' = "direct", failedSelected' = failedSelected,
    usedDirectAfterMirrorFailure' = true,
  }

  action step = any { selectMirror, mirrorFails, faultyDirectFallback }

  val noTransportRetry = not(usedDirectAfterMirrorFailure)
}
