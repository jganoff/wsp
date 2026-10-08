#!/usr/bin/env python3
"""Controlled local Git fixture: actual wsp fetch with scripted enumeration."""
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import time

binary = Path(os.environ.get('WSP_DEMO_BINARY', 'target/release/wsp')).resolve()
git = shutil.which('git')
with tempfile.TemporaryDirectory(prefix='wsp-demo-') as tmp:
    root = Path(tmp)
    env = dict(os.environ, HOME=str(root), XDG_DATA_HOME=str(root / 'data'),
               GIT_CONFIG_NOSYSTEM='1', GIT_CONFIG_GLOBAL=str(root / 'gitconfig'))
    source = root / 'remotes' / 'widgets'
    source.mkdir(parents=True)
    def quiet(args, cwd=root):
        subprocess.run(args, cwd=cwd, env=env, stdout=subprocess.DEVNULL,
                       stderr=subprocess.PIPE, check=True)
    quiet([git, 'init', '-q', '-b', 'main'], source)
    quiet([git, '-c', 'user.name=Demo', '-c', 'user.email=demo@example.test',
           '-c', 'commit.gpgsign=false', 'commit', '--allow-empty', '-qm', 'initial'], source)
    (root / 'gitconfig').write_text('[url "file://' + str(root / 'remotes') + '/"]\n'
                                   '    insteadOf = https://github.com/acme/\n')
    quiet([str(binary), 'config', 'set', 'workspaces-dir', str(root / 'workspaces'), '--global'])
    quiet([str(binary), 'config', 'set', 'hints', 'false', '--global'])
    quiet([str(binary), 'new', 'demo', '--empty'])
    workspace = root / 'workspaces' / 'demo'
    quiet([str(binary), 'repo', 'add', 'https://github.com/acme/widgets'], workspace)
    quiet([git, '-c', 'user.name=Demo', '-c', 'user.email=demo@example.test',
           '-c', 'commit.gpgsign=false', 'commit', '--allow-empty', '-qm', 'next'], source)
    bindir = root / 'bin'; bindir.mkdir()
    wrapper = bindir / 'git'
    wrapper.write_text('#!/bin/sh\nif [ "$1" = fetch ]; then\n'
                       '  printf "remote: Enumerating objects: 2, done.\\n" >&2\n'
                       '  sleep 3\nfi\nexec "$WSP_DEMO_REAL_GIT" "$@"\n')
    wrapper.chmod(0o755)
    env.update(PATH=str(bindir) + os.pathsep + env['PATH'], WSP_DEMO_REAL_GIT=git)
    print('Local fixture: Git emits enumeration, then waits before transfer.', flush=True)
    print('$ wsp repo fetch', flush=True)
    time.sleep(0.4)
    subprocess.run([str(binary), 'repo', 'fetch'], cwd=workspace, env=env, check=True)
    print('\nFetch completed; transient feedback stopped.', flush=True)
    time.sleep(0.8)
