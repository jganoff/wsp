---
name: wsp-record-demo
description: Record a private terminal UX demo and render an inline GIF for a wsp pull request
user_invocable: true
---

# Record a Terminal Demo

Use this skill when a pull request needs inspectable terminal UX proof. Keep the recording and rendered image in the repository; do not upload captures to external services.

## Capture

1. Use a controlled, deterministic scenario. Do not record credentials, private repository names, local paths, or unrelated terminal history.
2. Record the actual command with [asciinema](https://docs.asciinema.org/):

   ```bash
   mkdir -p docs/demos
   asciinema rec docs/demos/<name>.cast
   ```

3. Run the scenario in the recording, including its initial status, meaningful intermediate state, and completion. Stop the recording promptly.

## Render

Render the cast locally with [agg](https://docs.asciinema.org/manual/agg/). Keep the source `.cast` next to the GIF:

```bash
agg docs/demos/<name>.cast docs/demos/<name>.gif \
  --theme github-dark --font-size 16 --rows <visible-rows> --cols <render-columns>
```

Choose the smallest `--cols` value that renders every animation frame without wrapping. This is especially important for progress output using Unicode block characters: renderer glyph-width rules can differ from the recording terminal. Re-render and inspect the animation until cursor updates replace the intended line instead of scrolling.

## Verify and attach

1. Inspect the GIF through a normal image viewer and confirm that every progress update redraws in place.
2. Check the working tree and review the final asset sizes. Keep demos short and compact.
3. Add the GIF as a normal Markdown image in the PR description, never inside a code block.
4. In the required `ux-proof` block, include an exact `Reproduce:` command and raw GitHub URLs for both the GIF and source `.cast`.

The image demonstrates the interaction; the `.cast` is the authoritative recording. Do not claim a handcrafted or static illustration is a recording.
