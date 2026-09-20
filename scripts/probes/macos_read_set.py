#!/usr/bin/env python3
"""Re-derive, verify or trim porta's macOS strict-read set.

`sandbox-exec` can express a read allow-list, but a guessed one aborts every
process: the loader needs paths the list does not name, and the profile's own
`(trace ...)` facility is denied. The kernel reports the missing paths anyway,
in the unified log:

    kernel (Sandbox) Sandbox: echo(35707) deny(1) file-read-data /

This probe drives that loop. The set it checks is read out of
`native/sandbox_profile.rs`, not repeated here, so the probe cannot drift from
what porta actually applies.

    python3 scripts/probes/macos_read_set.py verify     # does the shipped set work?
    python3 scripts/probes/macos_read_set.py discover   # what else does a command want?
    python3 scripts/probes/macos_read_set.py ablate     # which entries are load-bearing?

A path this probe reports as unused was simply not reached by the commands
below; it is not thereby safe to remove. Widen COMMANDS before trimming.
"""

import argparse
import datetime
import pathlib
import re
import subprocess
import sys
import tempfile
import time

SOURCE = pathlib.Path(__file__).resolve().parents[2] / 'native' / 'sandbox_profile.rs'

# A sample wide enough that the platform paths a command needs to start are
# actually exercised: a shell, an interpreter, a TLS client, an archiver.
COMMANDS = [
    ['/bin/sh', '-c', 'cat {mount}/probe.txt'],
    ['/bin/echo', 'hi'],
    ['/bin/cat', '/etc/hosts'],
    ['/bin/ls', '{mount}'],
    ['/bin/date'],
    ['/usr/bin/env'],
    ['/usr/bin/grep', '-c', '.', '/etc/hosts'],
    ['/usr/bin/awk', 'BEGIN{{print 1}}'],
    ['/usr/bin/sed', '-n', '1p', '/etc/hosts'],
    ['/usr/bin/curl', '--version'],
    ['/usr/bin/openssl', 'version'],
    ['/usr/bin/tar', '--version'],
    ['/usr/bin/perl', '-e', 'print 1'],
    ['/usr/bin/sqlite3', '--version'],
    ['/usr/bin/find', '{mount}', '-type', 'f'],
    ['/usr/bin/plutil', '-help'],
    ['/usr/bin/sw_vers'],
    ['/usr/bin/unzip', '-v'],
]


def shipped_rules():
    """The read rules porta applies, read from the Rust source."""
    source = SOURCE.read_text()

    def array(name):
        match = re.search(rf'{name}: \[&str; \d+\] =\s*(\[[^\]]*\])', source)
        if not match:
            sys.exit(f'{name} not found in {SOURCE}; the probe needs updating')
        return re.findall(r'"([^"]+)"', match.group(1))

    subpaths = array('PROFILE_READABLE') + array('PROFILE_WRITABLE')
    return ([f'(subpath "{path}")' for path in subpaths]
            + [f'(literal "{path}")' for path in array('PROFILE_READABLE_LITERALS')])


def profile_text(rules, mount):
    allow = '\n'.join(f'(allow file-read* {rule})' for rule in rules)
    return (f'(version 1)\n(allow default)\n(deny file-read*)\n'
            f'(allow file-read* (subpath "{mount}"))\n{allow}\n')


def run_under(rules, command, mount):
    """Run one command under these rules; return (exit code, first stderr line)."""
    with tempfile.NamedTemporaryFile('w', suffix='.sb', delete=False) as handle:
        handle.write(profile_text(rules, mount))
        path = handle.name
    try:
        filled = [part.format(mount=mount) for part in command]
        result = subprocess.run(['sandbox-exec', '-f', path] + filled,
                                capture_output=True, text=True, cwd='/')
        stderr = (result.stderr.strip().splitlines() or [''])[0]
        return result.returncode, stderr
    finally:
        pathlib.Path(path).unlink(missing_ok=True)


def denials_since(started):
    """Paths the kernel refused to let a sandboxed process read, in order."""
    shown = subprocess.run(
        ['/usr/bin/log', 'show', '--style', 'compact', '--start', started,
         '--predicate', 'senderImagePath CONTAINS "Sandbox"'],
        capture_output=True, text=True).stdout
    paths = []
    for line in shown.splitlines():
        found = re.search(r'deny\(1\) file-read-\w+ (/\S*)', line)
        if found and found.group(1) not in paths:
            paths.append(found.group(1))
    return paths


def confined_mount():
    """A directory standing in for a granted mount, holding one readable file."""
    mount = pathlib.Path(tempfile.mkdtemp(prefix='porta-read-set-')).resolve()
    (mount / 'probe.txt').write_text('mounted data\n')
    return str(mount)


def failures(rules, mount):
    """Commands that fail or emit a denial under these rules."""
    bad = []
    for command in COMMANDS:
        code, stderr = run_under(rules, command, mount)
        if code != 0 or 'not permitted' in stderr:
            bad.append((command[0], code, stderr[:60]))
    return bad


def verify(rules, mount):
    bad = failures(rules, mount)
    for name, code, stderr in bad:
        print(f'  FAIL {name} exit={code} {stderr}')
    print(f'{len(COMMANDS)} commands, {len(bad)} failing')
    secret = pathlib.Path.home() / '.ssh'
    code, _ = run_under(rules, ['/bin/ls', str(secret)], mount)
    print(f'  reading {secret}: {"denied" if code != 0 else "ALLOWED — the set is too wide"}')
    return 1 if bad else 0


def discover(rules, mount):
    """Add what the kernel says is missing until every command runs."""
    rules = list(rules)
    for _ in range(20):
        bad = failures(rules, mount)
        if not bad:
            print('every command runs; nothing to add')
            return 0
        started = datetime.datetime.now().strftime('%Y-%m-%d %H:%M:%S')
        for command in COMMANDS:
            run_under(rules, command, mount)
        time.sleep(3)
        missing = [path for path in denials_since(started)
                   if f'(literal "{path}")' not in rules and f'(subpath "{path}")' not in rules]
        if not missing:
            print(f'{len(bad)} commands still fail but the kernel named nothing new:')
            for name, code, stderr in bad:
                print(f'  {name} exit={code} {stderr}')
            return 1
        print(f'adding: {", ".join(missing[:8])}')
        rules += [f'(literal "{path}")' for path in missing]
    print('did not converge in 20 rounds')
    return 1


def ablate(rules, mount):
    """Drop each rule in turn and report which ones anything actually needs."""
    print('full set failures:', failures(rules, mount) or 'none')
    for rule in rules:
        without = [other for other in rules if other != rule]
        bad = sorted({name for name, _, _ in failures(without, mount)})
        verdict = f'REQUIRED  {bad}' if bad else 'not reached by this sample'
        print(f'  without {rule:<34} {verdict}')
    return 0


def main():
    if sys.platform != 'darwin':
        sys.exit('this probe reads the macOS unified log; it only runs on macOS')
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('mode', choices=['verify', 'discover', 'ablate'])
    mode = parser.parse_args().mode
    rules = shipped_rules()
    print(f'{len(rules)} read rules from {SOURCE.name}')
    sys.exit({'verify': verify, 'discover': discover, 'ablate': ablate}[mode](rules, confined_mount()))


if __name__ == '__main__':
    main()
