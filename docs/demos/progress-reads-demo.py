#!/usr/bin/env python3
"""Record real wsp status output with an isolated, controlled gh fixture.

Reproduce from the repository root:
  WSP_DEMO_BINARY="$PWD/target/release/wsp" asciinema rec \
    docs/demos/progress-reads.cast --overwrite --return --window-size 88x18 \
    --output-format asciicast-v2 --command \
    "python3 docs/demos/progress-reads-demo.py"
  agg docs/demos/progress-reads.cast /tmp/progress-reads.gif \
    --theme github-dark --font-size 16 --rows 18 --cols 88 --fps-cap 12

The GitHub fixture waits for the CLI's visible progress before being released.
Recording pauses are presentation pacing, not concurrent timing assertions.
"""

import argparse
import errno
import fcntl
import os
from pathlib import Path
import pty
import select
import struct
import subprocess
import sys
import tempfile
import termios
import time


def run_quiet(binary, environment, directory, *arguments):
    result = subprocess.run(
        [str(binary), *arguments], cwd=directory, env=environment,
        capture_output=True, check=False,
    )
    if result.returncode:
        raise RuntimeError("controlled demo fixture setup failed")


def show_status(binary, environment, workspace, release=None):
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 18, 88, 0, 0))
    child = subprocess.Popen(
        [str(binary), "st"], cwd=workspace, env=environment,
        stdin=slave, stdout=slave, stderr=slave,
    )
    os.close(slave)
    transcript = bytearray()
    release_at = None
    watchdog = time.monotonic() + 40
    try:
        while True:
            now = time.monotonic()
            if release_at is not None and now >= release_at:
                release.touch()
                release_at = None
            if now >= watchdog:
                raise RuntimeError("controlled demo did not reach its visible PR lookup")
            readable, _, _ = select.select([master], [], [], 0.05)
            if not readable:
                continue
            try:
                chunk = os.read(master, 65536)
            except OSError as error:
                if error.errno == errno.EIO:
                    break
                raise
            if not chunk:
                break
            transcript.extend(chunk)
            sys.stdout.buffer.write(chunk)
            sys.stdout.buffer.flush()
            if release is not None and not release.exists() and release_at is None:
                if b"Fetching pull request for acme/api" in transcript:
                    release_at = time.monotonic() + 1.4
        if child.wait() != 0:
            raise RuntimeError("recorded status command failed")
        if release is not None and not release.exists():
            raise RuntimeError("recording did not show the named PR lookup")
        if b"REPOSITORY" not in transcript or b"api" not in transcript:
            raise RuntimeError("recording is missing the completed status table")
    finally:
        if child.poll() is None:
            child.kill()
            child.wait()
        os.close(master)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", default=os.environ.get("WSP_DEMO_BINARY", "target/release/wsp"))
    binary = Path(parser.parse_args().binary).resolve()
    with tempfile.TemporaryDirectory(prefix="wsp-reads-demo-") as directory:
        root = Path(directory)
        environment = os.environ.copy()
        environment.update(HOME=str(root), XDG_DATA_HOME=str(root / "data"),
                           USERPROFILE=str(root), TERM="xterm-256color",
                           WSP_SHELL="", WSP_PWD="", WSP_CD_FILE="")
        for name in ["WSP_SHELL", "WSP_PWD", "WSP_CD_FILE", "ASCIINEMA_SESSION"]:
            environment.pop(name, None)
        workspaces = root / "workspaces"
        run_quiet(binary, environment, root, "config", "set", "workspaces-dir", str(workspaces), "--global")
        run_quiet(binary, environment, root, "config", "set", "hints", "false", "--global")
        run_quiet(binary, environment, root, "new", "review", "--empty")
        workspace = workspaces / "review"
        metadata = workspace / ".wsp.yaml"
        source = metadata.read_text()
        if "repos: {}" not in source:
            raise RuntimeError("controlled demo metadata is not empty")
        metadata.write_text(source.replace("repos: {}", "repos:\n  github.com/acme/api: null"))
        repo = workspace / "api"
        repo.mkdir()
        for arguments in [
            ["init", "--quiet", "-b", "demo/review"],
            ["config", "user.name", "Demo"], ["config", "user.email", "demo@example.com"],
            ["config", "commit.gpgsign", "false"], ["commit", "--quiet", "--allow-empty", "-m", "fixture"],
        ]:
            subprocess.run(["git", *arguments], cwd=repo, env=environment,
                           stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, check=True)
        # Match the workspace branch without recording any machine-specific configuration.
        branch = next(line.split(": ", 1)[1] for line in metadata.read_text().splitlines() if line.startswith("branch: "))
        subprocess.run(["git", "branch", "-m", branch], cwd=repo, env=environment, check=True)
        print("\033[2J\033[H$ wsp st  # fast local read", flush=True)
        show_status(binary, environment, workspace)
        time.sleep(1.5)
        run_quiet(binary, environment, root, "config", "set", "pr.source", "github", "--global")
        fixtures = root / "bin"
        fixtures.mkdir()
        gh = fixtures / "gh"
        gh.write_text(
            '#!/bin/sh\nwhile [ ! -f "$WSP_DEMO_RELEASE" ]; do sleep 0.02; done\n'
            'printf \'[{"number":42,"url":"https://github.com/acme/api/pull/42",'
            '"state":"OPEN","title":"Add API docs","isDraft":false}]\\n\'\n'
        )
        gh.chmod(0o755)
        release = root / "release"
        environment.update(PATH=str(fixtures) + os.pathsep + environment["PATH"],
                           WSP_DEMO_RELEASE=str(release))
        print("\033[2J\033[H$ wsp st  # PR lookup enabled", flush=True)
        print("Controlled fixture: gh waits until progress is visible.\n", flush=True)
        show_status(binary, environment, workspace, release)
        time.sleep(1.5)


if __name__ == "__main__":
    main()
