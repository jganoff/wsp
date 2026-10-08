---
name: wsp-record-demo
description: Record a private terminal UX demo and render an inline GIF for a wsp pull request
user_invocable: true
---

# Record a Terminal Demo

Use this skill when a pull request needs inspectable terminal UX proof. Keep reproducible scripts and small text recordings in the repository. Render generated images and videos in a temporary directory and upload them as GitHub PR attachments; never commit them.

## Capture

1. Use a controlled, deterministic scenario. Do not record credentials, private repository names, local paths, or unrelated terminal history.
2. Record the actual command with [asciinema](https://docs.asciinema.org/):

   ```bash
   mkdir -p docs/demos
   asciinema rec docs/demos/<name>.cast
   ```

3. Run the scenario in the recording, including its initial status, meaningful intermediate state, and completion. Stop the recording promptly.

## Render

Render the cast locally with [agg](https://docs.asciinema.org/manual/agg/):

```bash
agg docs/demos/<name>.cast /tmp/<name>.gif \
  --theme github-dark --font-size 16 --rows <visible-rows> --cols <render-columns>
```

Choose the smallest `--cols` value that renders every animation frame without wrapping. This is especially important for progress output using Unicode block characters: renderer glyph-width rules can differ from the recording terminal. Re-render and inspect the animation until cursor updates replace the intended line instead of scrolling.

## Verify and attach

1. Inspect the GIF through a normal image viewer and confirm that every progress update redraws in place.
2. Review the asset size and confirm generated media is absent from the Git diff. Keep demos short and compact.
3. Attach the GIF with `gh pr edit <number> --attach /tmp/<name>.gif`. To embed it in an existing description, use `--body-file` with a local Markdown image reference; GitHub CLI replaces that reference with the uploaded URL. Read the resulting PR body to verify the attachment before deleting local media.
4. In the required `ux-proof` block, include an exact `Reproduce:` command, the GitHub attachment URL, and a raw GitHub URL for the source `.cast`. The inline GIF belongs outside the code block. Upload failure is not a reason to commit generated media.

The image demonstrates the interaction; the `.cast` is the authoritative recording. Do not claim a handcrafted or static illustration is a recording.
