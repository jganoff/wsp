# Reviewing pull requests

Run `just ci` before opening a pull request. It runs formatting and linting,
cross-target checks, the dependency audit, the test suite, and `just validate`.
Validation builds the release binary, runs the offline smoke test against it,
and checks that generated CLI documentation is current. Run `just smoke` as
well when a change affects its network or release-only coverage.

## UX proof

Every pull request description has a `ux-proof` block. Use `NONE` only when
the change has no human-visible behavior. Otherwise, include an exact
reproduction command and a URL to evidence a reviewer can inspect.

For terminal interactions, prefer an [asciinema](https://asciinema.org/)
recording. It preserves timing and cursor updates, which screenshots lose. A
screenshot is appropriate when a single stable frame is the behavior under
review. Keep recordings short, deterministic, and free of credentials and
private repository names.

This does not replace tests. Tests prove the CLI contract, including `--json`;
the recording shows the interactive experience that a test failure alone does
not explain.

For example, a progress-related pull request should show one complete
operation, including its initial status line, progress updates, and completion:

```ux-proof
Reproduce: XDG_DATA_HOME="$tmp" wsp fetch --all
https://asciinema.org/a/example
```

Upload the recording before requesting review, then replace the example URL.
