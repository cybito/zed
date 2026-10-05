#!/usr/bin/env python3
"""Build, publish and verify Zed installation packages as GitHub Release assets."""
import argparse
import filecmp
import hashlib
import json
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile

PROJECT = 'zed'
REPO = 'https://github.com/cybito/zed.git'
TAG = re.compile(r'v[0-9]+\.[0-9]+\.[0-9]+-custom\.[1-9][0-9]*\Z')
SHA = re.compile(r'[0-9a-f]{40}\Z')
MAX_FILE_SIZE = 2 * 1024**3
MAX_ASSETS = 1000


def run(*args, cwd=None):
    result = subprocess.run(args, cwd=cwd, capture_output=True)
    if result.returncode:
        raise RuntimeError(result.stderr.decode(errors='replace'))
    return result.stdout


def digest(data):
    return hashlib.sha256(data).hexdigest()

def digest_file(path):
    value = hashlib.sha256()
    with path.open('rb') as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b''):
            value.update(chunk)
    return value.hexdigest()


def absolute(value):
    path = Path(value)
    if not path.is_absolute():
        raise argparse.ArgumentTypeError('absolute path required')
    return path


def identity(tag, commit, platform):
    if not TAG.fullmatch(tag) or not SHA.fullmatch(commit) or platform not in ('darwin', 'linux'):
        raise ValueError('invalid release identity')


def prefix(tag, platform):
    identity(tag, '0' * 40, platform)
    return f'{tag}-{platform}-'


def release_assets(tag):
    value = json.loads(run('gh', 'release', 'view', tag, '--repo', 'cybito/zed', '--json', 'assets'))
    return value['assets']


def asset_name(tag, platform, name):
    return prefix(tag, platform) + name


def validate_package(directory, tag, commit, platform):
    receipt = json.loads((directory / 'release.json').read_text())
    if set(receipt) != {'schema','project','source_repo','source_commit','release_tag','platform','architecture','toolchains','files'}:
        raise ValueError('receipt fields mismatch')
    identity(tag, commit, platform)
    if (receipt['schema'], receipt['project'], receipt['source_repo'], receipt['source_commit'], receipt['release_tag'], receipt['platform'], receipt['architecture']) != (1, PROJECT, REPO, commit, tag, platform, 'arm64'):
        raise ValueError('receipt identity mismatch')
    extension = 'dmg' if platform == 'darwin' else 'tar.gz'
    payload_names = {f'zed-{tag}-{platform}-arm64.{extension}', f'zed-remote-server-{tag}-{platform}-arm64.gz'}
    if not isinstance(receipt['files'], list) or len(receipt['files']) != 2 or {f.get('name') for f in receipt['files']} != payload_names:
        raise ValueError('missing or unexpected payload names')
    if not isinstance(receipt['toolchains'], dict) or not receipt['toolchains'] or not all(isinstance(v, str) and v for v in receipt['toolchains'].values()):
        raise ValueError('toolchain versions required')
    expected = {'release.json', 'SHA256SUMS'}
    for item in receipt['files']:
        if set(item) != {'name','sha256','size'} or Path(item['name']).name != item['name'] or item['name'] in expected:
            raise ValueError('invalid file record')
        expected.add(item['name'])
        path = directory / item['name']
        if digest_file(path) != item['sha256'] or path.stat().st_size != item['size']:
            raise ValueError('payload mismatch')
    if {p.name for p in directory.iterdir()} != expected:
        raise ValueError('package files mismatch')
    sums = ''.join(f'{digest_file(directory / name)}  {name}\n' for name in sorted(expected - {'SHA256SUMS'}))
    if (directory / 'SHA256SUMS').read_text() != sums:
        raise ValueError('SHA256SUMS mismatch')
    return receipt


def remote_package(tag, commit, platform, assets, directory):
    start = prefix(tag, platform)
    selected = {}
    for asset in assets:
        full_name = asset['name']
        if not full_name.startswith(start):
            continue
        name = full_name[len(start):]
        if not re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9_.-]*', name) or name in selected:
            raise ValueError('unsafe or duplicate release asset name')
        selected[name] = asset
    if not selected:
        return None
    directory.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory() as temp:
        names = [a['name'] for a in assets if a['name'].startswith(start)]
        run('gh', 'release', 'download', tag, '--repo', 'cybito/zed', '--dir', temp, *sum((['--pattern', n] for n in names), []))
        downloaded = Path(temp)
        for name, asset in selected.items():
            path = downloaded / asset['name']
            if not path.is_file() or path.stat().st_size != asset['size'] or asset['size'] >= MAX_FILE_SIZE:
                raise ValueError('downloaded asset size mismatch or GitHub file limit exceeded')
            shutil.copyfile(path, directory / name)
    extension = 'dmg' if platform == 'darwin' else 'tar.gz'
    expected = {'release.json', 'SHA256SUMS', f'zed-{tag}-{platform}-arm64.{extension}', f'zed-remote-server-{tag}-{platform}-arm64.gz'}
    if not set(selected).issubset(expected):
        raise ValueError('unexpected release asset for platform package')
    receipt = None
    if set(selected) == expected:
        receipt = validate_package(directory, tag, commit, platform)
    return {'files': selected, 'receipt': receipt}
def check(tag, commit, platform, directory):
    identity(tag, commit, platform)
    directory.mkdir(parents=True, exist_ok=True)
    if any(directory.iterdir()):
        raise ValueError('check output must be empty')
    assets = release_assets(tag)
    result = remote_package(tag, commit, platform, assets, directory)
    if result is None:
        return {'exists': False}
    if result['receipt'] is None:
        return {'exists': False, 'partial': sorted(a['name'] for a in result['files'].values())}
    return {'exists': True, 'assets': sorted(a['name'] for a in result['files'].values())}


def pack(args):
    identity(args.tag, args.commit, args.platform)
    args.output_dir.mkdir(parents=True, exist_ok=True)
    if any(args.output_dir.iterdir()):
        raise ValueError('pack output must be empty')
    files = []
    for path in sorted(args.input_dir.iterdir()):
        if path.name.endswith(('.tar.gz', '.dmg', '.gz')):
            shutil.copy2(path, args.output_dir / path.name)
            files.append({'name': path.name, 'sha256': digest_file(path), 'size': path.stat().st_size})
    if len(files) != 2 or not any('remote-server' in f['name'] for f in files):
        raise ValueError('desktop and remote server packages required')
    toolchains = json.loads((args.input_dir / 'toolchains.json').read_text())
    receipt = dict(schema=1, project=PROJECT, source_repo=REPO, source_commit=args.commit, release_tag=args.tag, platform=args.platform, architecture='arm64', toolchains=toolchains, files=files)
    (args.output_dir / 'release.json').write_text(json.dumps(receipt, sort_keys=True, indent=2) + '\n')
    names = sorted([f['name'] for f in files] + ['release.json'])
    (args.output_dir / 'SHA256SUMS').write_text(''.join(f'{digest_file(args.output_dir / name)}  {name}\n' for name in names))
    validate_package(args.output_dir, args.tag, args.commit, args.platform)
    return {'directory': str(args.output_dir)}


def require_matching_bytes(local, remote):
    if not filecmp.cmp(local, remote, shallow=False):
        raise ValueError('refusing replacement of existing release asset bytes')


def publish(args):
    validate_package(args.directory, args.tag, args.commit, args.platform)
    args.output_dir.mkdir(parents=True, exist_ok=True)
    assets = release_assets(args.tag)
    existing = remote_package(args.tag, args.commit, args.platform, assets, args.output_dir)
    expected = {p.name: p for p in args.directory.iterdir()}
    present = existing['files'] if existing else {}
    if len(assets) > MAX_ASSETS or len(assets) + len(expected) - len(present) > MAX_ASSETS:
        raise ValueError('GitHub release asset limit exceeded')
    for name, path in expected.items():
        if path.stat().st_size >= MAX_FILE_SIZE:
            raise ValueError('GitHub per-file size limit exceeded')
        if name in present:
            require_matching_bytes(path, args.output_dir / name)
    missing = [path for name, path in expected.items() if name not in present]
    if missing:
        with tempfile.TemporaryDirectory() as stage:
            staged = []
            for path in missing:
                target = Path(stage) / asset_name(args.tag, args.platform, path.name)
                shutil.copyfile(path, target)
                staged.append(str(target))
            run('gh', 'release', 'upload', args.tag, *staged, '--repo', 'cybito/zed')
    with tempfile.TemporaryDirectory() as temp:
        fetched = Path(temp)
        names = [asset_name(args.tag, args.platform, name) for name in expected]
        run('gh', 'release', 'download', args.tag, '--repo', 'cybito/zed', '--dir', temp, *sum((['--pattern', n] for n in names), []))
        for name, path in expected.items():
            downloaded = fetched / asset_name(args.tag, args.platform, name)
            if not filecmp.cmp(downloaded, path, shallow=False):
                raise ValueError('release asset readback bytes mismatch')
            shutil.copyfile(downloaded, args.output_dir / name)
        validate_package(args.output_dir, args.tag, args.commit, args.platform)
    return {'tag': args.tag, 'platform': args.platform, 'assets': sorted(asset_name(args.tag, args.platform, n) for n in expected)}


def main():
    parser = argparse.ArgumentParser()
    sub = parser.add_subparsers(dest='command', required=True)
    for command in ('check', 'pack'):
        p = sub.add_parser(command)
        p.add_argument('--tag', required=True)
        p.add_argument('--commit', required=True)
        p.add_argument('--platform', choices=('darwin', 'linux'), required=True)
        p.add_argument('--output-dir', type=absolute, required=True)
        if command == 'pack': p.add_argument('--input-dir', type=absolute, required=True)
    p = sub.add_parser('publish')
    p.add_argument('--directory', type=absolute, required=True)
    p.add_argument('--output-dir', type=absolute, required=True)
    p.add_argument('--tag', required=True)
    p.add_argument('--commit', required=True)
    p.add_argument('--platform', choices=('darwin', 'linux'), required=True)
    args = parser.parse_args()
    result = check(args.tag, args.commit, args.platform, args.output_dir) if args.command == 'check' else pack(args) if args.command == 'pack' else publish(args)
    print(json.dumps(result))


if __name__ == '__main__':
    try:
        main()
    except (RuntimeError, ValueError, OSError, KeyError, json.JSONDecodeError) as error:
        print(str(error), file=sys.stderr)
        sys.exit(1)
