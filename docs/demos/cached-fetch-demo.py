#!/usr/bin/env python3
"""Local fixture for cached fetches and the slow-fetch progress lifecycle."""
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import time

binary = Path(os.environ.get("WSP_DEMO_BINARY", "target/release/wsp")).resolve()
git = shutil.which("git")
with tempfile.TemporaryDirectory(prefix="wsp-demo-") as tmp:
    root = Path(tmp)
    env = dict(os.environ, HOME=str(root), XDG_DATA_HOME=str(root / "data"),
               GIT_CONFIG_NOSYSTEM="1", GIT_CONFIG_GLOBAL=str(root / "gitconfig"))
    source = root / "upstream"
    source.mkdir()

    def quiet(args, cwd=root):
        subprocess.run(args, cwd=cwd, env=env, stdout=subprocess.DEVNULL,
                       stderr=subprocess.PIPE, check=True)

    quiet([git, "init", "-q", "-b", "main"], source)
    quiet([git, "config", "user.name", "Demo"], source)
    quiet([git, "config", "user.email", "demo@example.test"], source)
    quiet([git, "config", "commit.gpgsign", "false"], source)
    (source / "cached.txt").write_text("Cached history\n" * 1000)
    quiet([git, "add", "."], source)
    quiet([git, "commit", "-qm", "cached history"], source)
    (root / "gitconfig").write_text(
        '[url "' + source.as_uri() + '"]\n'
        '    insteadOf = https://github.com/acme/widgets\n')
    quiet([str(binary), "config", "set", "workspaces-dir", str(root / "workspaces"), "--global"])
    quiet([str(binary), "config", "set", "hints", "false", "--global"])
    quiet([str(binary), "registry", "add", "https://github.com/acme/widgets"])
    (source / "new.txt").write_text("An incremental update\n")
    quiet([git, "add", "."], source)
    quiet([git, "commit", "-qm", "incremental update"], source)
    bindir = root / "bin"
    bindir.mkdir()
    wrapper = bindir / "git"
    wrapper.write_text(
        '#!/bin/sh\n'
        'if [ "$WSP_DEMO_SLOW" = 1 ]; then\n'
        '  case "$PWD" in\n'
        '    */.wsp-fetch-*)\n'
        '      if [ "$1" = fetch ]; then\n'
        '        sleep 1.5\n'
        '        printf "Receiving objects: 25%% (1/4), 1.00 KiB | 2.00 KiB/s\\r" >&2\n'
        '        sleep 1.2\n'
        '        printf "Receiving objects: 75%% (3/4), 3.00 KiB | 2.00 KiB/s\\r" >&2\n'
        '        sleep 1.2\n'
        '      elif [ "$1" = repack ]; then\n'
        '        sleep 1.5\n'
        '      fi;;\n'
        '  esac\n'
        'fi\n'
        'exec "$WSP_DEMO_REAL_GIT" "$@"\n')
    wrapper.chmod(0o755)
    env.update(PATH=str(bindir) + os.pathsep + env["PATH"], WSP_DEMO_REAL_GIT=git)
    print("Local fixture: scripted transfer counts and brief transport/import delays.", flush=True)
    print("$ wsp new incremental widgets", flush=True)
    env["WSP_DEMO_SLOW"] = "1"
    subprocess.run([str(binary), "new", "incremental", "widgets"], cwd=root, env=env, check=True)
    time.sleep(0.7)
    print("\nUnchanged upstream: reuse the cache and skip repacking.", flush=True)
    print("$ wsp new cached widgets", flush=True)
    env["WSP_DEMO_SLOW"] = "0"
    subprocess.run([str(binary), "new", "cached", "widgets"], cwd=root, env=env, check=True)
    time.sleep(0.8)
