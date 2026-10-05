#!/usr/bin/env python3
"""Zed installation artifacts; intentionally separate from historical native receipts."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile

PROJECT = 'zed'
REPO = 'https://github.com/cybito/zed.git'
PACKAGE = 'git.cybit.top/cybit/ias-zed'
TYPE = 'application/vnd.cybito.install-package.v1'
TAG = re.compile(r'v[0-9]+\.[0-9]+\.[0-9]+-custom\.[1-9][0-9]*\Z')
SHA = re.compile(r'[0-9a-f]{40}\Z')

def run(*args, cwd=None):
    result = subprocess.run(args, cwd=cwd, capture_output=True)
    if result.returncode:
        raise RuntimeError(result.stderr.decode(errors='replace'))
    return result.stdout

def digest(data):
    return hashlib.sha256(data).hexdigest()

def absolute(value):
    p = Path(value)
    if not p.is_absolute():
        raise argparse.ArgumentTypeError('absolute path required')
    return p

def identity(tag, commit, platform):
    if not TAG.fullmatch(tag) or not SHA.fullmatch(commit) or platform not in ('darwin','linux'):
        raise ValueError('invalid release identity')
    return f'{PACKAGE}:{tag}-{platform}-arm64'

def auth():
    # Anonymous operations still use an isolated, explicit registry configuration.
    return ['--registry-config', os.environ.get('ORAS_REGISTRY_CONFIG', str(Path(tempfile.gettempdir()) / 'zed-anonymous-registry.json'))]

def manifest(reference):
    data = run('oras', 'manifest', 'fetch', reference, *auth())
    if '@sha256:' in reference and digest(data) != reference.rsplit(':', 1)[1]:
        raise ValueError('manifest digest mismatch')
    value = json.loads(data)
    if value.get('artifactType') != TYPE:
        raise ValueError('wrong artifact type')
    layers = value.get('layers', [])
    names = [d.get('annotations', {}).get('org.opencontainers.image.title') for d in layers]
    if len(names) != len(set(names)) or not all(isinstance(n, str) and re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9_.-]*', n) for n in names):
        raise ValueError('unsafe or duplicate layer names')
    return data, layers

def verify(reference, directory):
    if not re.fullmatch(re.escape(PACKAGE) + r'@sha256:[0-9a-f]{64}', reference):
        raise ValueError('immutable project reference required')
    data, layers = manifest(reference)
    value = json.loads(data)
    if value.get('schemaVersion') != 2 or value.get('mediaType') != 'application/vnd.oci.image.manifest.v1+json':
        raise ValueError('invalid OCI manifest schema')
    config = value.get('config', {})
    if config.get('mediaType') != 'application/vnd.oci.empty.v1+json' or not re.fullmatch(r'sha256:[0-9a-f]{64}', config.get('digest', '')) or type(config.get('size')) is not int:
        raise ValueError('invalid OCI config descriptor')
    with tempfile.TemporaryDirectory() as temp:
        blob = Path(temp) / 'config'
        run('oras', 'blob', 'fetch', '--output', str(blob), PACKAGE + '@' + config['digest'], *auth())
        if blob.stat().st_size != config['size'] or 'sha256:' + digest(blob.read_bytes()) != config['digest']:
            raise ValueError('OCI config bytes mismatch')
    directory.mkdir(parents=True, exist_ok=True)
    if any(directory.iterdir()):
        raise ValueError('verification directory must be empty')
    run('oras', 'pull', reference, '--output', str(directory), *auth())
    for layer in layers:
        if not re.fullmatch(r'sha256:[0-9a-f]{64}', layer.get('digest', '')) or type(layer.get('size')) is not int:
            raise ValueError('invalid OCI layer descriptor')
        p = directory / layer['annotations']['org.opencontainers.image.title']
        raw = p.read_bytes()
        if layer['digest'] != 'sha256:' + digest(raw) or layer['size'] != len(raw):
            raise ValueError('layer bytes mismatch')
    receipt = json.loads((directory / 'release.json').read_text())
    if set(receipt) != {'schema','project','source_repo','source_commit','release_tag','platform','architecture','toolchains','files'}:
        raise ValueError('receipt fields mismatch')
    identity(receipt['release_tag'], receipt['source_commit'], receipt['platform'])
    if receipt['schema'] != 1 or receipt['project'] != PROJECT or receipt['source_repo'] != REPO or receipt['architecture'] != 'arm64' or receipt['platform'] not in ('darwin', 'linux'):
        raise ValueError('receipt identity mismatch')
    annotations = value.get('annotations', {})
    if annotations.get('org.opencontainers.image.source') != REPO or annotations.get('org.opencontainers.image.revision') != receipt['source_commit'] or annotations.get('org.opencontainers.image.version') != receipt['release_tag']:
        raise ValueError('OCI provenance annotations mismatch')
    platform, tag = receipt['platform'], receipt['release_tag']
    extension = 'dmg' if platform == 'darwin' else 'tar.gz'
    payload_names = {f'zed-{tag}-{platform}-arm64.{extension}', f'zed-remote-server-{tag}-{platform}-arm64.gz'}
    if not isinstance(receipt['files'], list) or len(receipt['files']) != 2 or {f.get('name') for f in receipt['files']} != payload_names:
        raise ValueError('missing or unexpected payload names')
    if not isinstance(receipt['toolchains'], dict) or not receipt['toolchains'] or not all(isinstance(v,str) and v for v in receipt['toolchains'].values()):
        raise ValueError('toolchain versions required')
    expected = {'release.json', 'SHA256SUMS'}
    for f in receipt['files']:
        if set(f) != {'name','sha256','size'} or Path(f['name']).name != f['name'] or f['name'] in expected:
            raise ValueError('invalid file record')
        expected.add(f['name'])
        raw = (directory / f['name']).read_bytes()
        if digest(raw) != f['sha256'] or len(raw) != f['size']:
            raise ValueError('payload mismatch')
    if expected != {d['annotations']['org.opencontainers.image.title'] for d in layers}:
        raise ValueError('unexpected layers')
    media = {'release.json': 'application/json', 'SHA256SUMS': 'text/plain'}
    for layer in layers:
        name = layer['annotations']['org.opencontainers.image.title']
        wanted = media.get(name, 'application/octet-stream' if name.endswith('.dmg') else 'application/gzip')
        if layer['mediaType'] != wanted:
            raise ValueError('layer media type mismatch')
    sums = ''.join(f'{digest((directory / n).read_bytes())}  {n}\n' for n in sorted(expected - {'SHA256SUMS'}))
    if (directory / 'SHA256SUMS').read_text() != sums:
        raise ValueError('SHA256SUMS mismatch')
    return receipt

def check(tag, commit, platform, directory):
    reference = identity(tag, commit, platform)
    p = subprocess.run(['oras','manifest','fetch','--descriptor',reference,*auth()], capture_output=True)
    if p.returncode:
        error = p.stderr.decode(errors='replace')
        if re.search(r'\b(MANIFEST_UNKNOWN|NAME_UNKNOWN|manifest unknown|name unknown)\b', error):
            return {'exists': False}
        raise RuntimeError(error)
    descriptor = json.loads(p.stdout)
    immutable = PACKAGE + '@' + descriptor['digest']
    receipt = verify(immutable, directory)
    if (receipt['release_tag'], receipt['source_commit'], receipt['platform']) != (tag, commit, platform):
        raise ValueError('published tag has a different identity')
    return {'exists': True, 'reference': immutable}

def pack(args):
    identity(args.tag, args.commit, args.platform)
    args.output_dir.mkdir(parents=True, exist_ok=True)
    if any(args.output_dir.iterdir()):
        raise ValueError('pack output must be empty')
    files = []
    for p in sorted(args.input_dir.iterdir()):
        if p.name.endswith(('.tar.gz','.dmg','.gz')):
            shutil.copy2(p, args.output_dir / p.name)
            raw = p.read_bytes()
            files.append({'name':p.name,'sha256':digest(raw),'size':len(raw)})
    if len(files) != 2 or not any('remote-server' in f['name'] for f in files):
        raise ValueError('desktop and remote server packages required')
    toolchains = json.loads((args.input_dir / 'toolchains.json').read_text())
    receipt = dict(schema=1,project=PROJECT,source_repo=REPO,source_commit=args.commit,release_tag=args.tag,platform=args.platform,architecture='arm64',toolchains=toolchains,files=files)
    (args.output_dir / 'release.json').write_text(json.dumps(receipt, sort_keys=True, indent=2) + '\n')
    names = sorted([f['name'] for f in files] + ['release.json'])
    (args.output_dir / 'SHA256SUMS').write_text(''.join(f'{digest((args.output_dir / n).read_bytes())}  {n}\n' for n in names))
    return {'directory': str(args.output_dir)}

def publish(args):
    os.environ['ORAS_REGISTRY_CONFIG'] = str(args.registry_config)
    receipt = json.loads((args.directory / 'release.json').read_text())
    with tempfile.TemporaryDirectory() as tmp:
        previous = check(receipt['release_tag'], receipt['source_commit'], receipt['platform'], Path(tmp) / 'existing')
        if previous['exists']:
            old = Path(tmp) / 'existing'
            if {p.name for p in args.directory.iterdir()} != {p.name for p in old.iterdir()} or any(p.read_bytes() != (old / p.name).read_bytes() for p in args.directory.iterdir()):
                raise ValueError('refusing replacement of published bytes')
            return {'reference':previous['reference'],'digest':previous['reference'].split('@')[1]}
        layout = Path(tmp) / 'layout'
        from datetime import datetime, timezone
        timestamp = int(run('git','show','-s','--format=%ct',receipt['source_commit']))
        created = datetime.fromtimestamp(timestamp, timezone.utc).strftime('%Y-%m-%dT%H:%M:%SZ')
        target = identity(receipt['release_tag'],receipt['source_commit'],receipt['platform'])
        layers = []
        for p in sorted(args.directory.iterdir()):
            media = 'application/json' if p.name == 'release.json' else 'text/plain' if p.name == 'SHA256SUMS' else 'application/octet-stream' if p.suffix == '.dmg' else 'application/gzip'
            layers.append(f'{p.name}:{media}')
        run('oras','push','--oci-layout',str(layout)+':release','--artifact-type',TYPE,'--annotation','org.opencontainers.image.created='+created,'--annotation','org.opencontainers.image.source='+REPO,'--annotation','org.opencontainers.image.revision='+receipt['source_commit'],'--annotation','org.opencontainers.image.version='+receipt['release_tag'],*layers,cwd=args.directory)
        descriptor = json.loads(run('oras','manifest','fetch','--oci-layout','--descriptor',str(layout)+':release'))
        sha = descriptor['digest']
        late = check(receipt['release_tag'], receipt['source_commit'], receipt['platform'], Path(tmp) / 'late')
        if late['exists']:
            old = Path(tmp) / 'late'
            if {p.name for p in args.directory.iterdir()} != {p.name for p in old.iterdir()} or any(p.read_bytes() != (old / p.name).read_bytes() for p in args.directory.iterdir()):
                raise ValueError('refusing replacement of bytes published during this build')
            return {'reference':late['reference'],'digest':late['reference'].split('@')[1]}
        run('oras','cp','--from-oci-layout',str(layout)+':release',target,'--to-registry-config',str(args.registry_config))
        immutable = PACKAGE + '@' + sha
        remote, _ = manifest(immutable)
        local = run('oras','manifest','fetch','--oci-layout',str(layout)+':release')
        if remote != local:
            raise ValueError('uploaded manifest differs')
        verify(immutable, Path(tmp) / 'verified')
        return {'reference':immutable,'digest':sha}

def main():
    parser = argparse.ArgumentParser()
    sub = parser.add_subparsers(dest='command', required=True)
    for cmd in ('check','pack'):
        p = sub.add_parser(cmd)
        p.add_argument('--tag',required=True)
        p.add_argument('--commit',required=True)
        p.add_argument('--platform',choices=('darwin','linux'),required=True)
        p.add_argument('--output-dir',type=absolute,required=True)
        if cmd == 'pack': p.add_argument('--input-dir',type=absolute,required=True)
    p = sub.add_parser('publish')
    p.add_argument('--directory',type=absolute,required=True)
    p.add_argument('--registry-config',type=absolute,required=True)
    p = sub.add_parser('verify')
    p.add_argument('--reference',required=True)
    p.add_argument('--output-dir',type=absolute,required=True)
    args = parser.parse_args()
    result = check(args.tag,args.commit,args.platform,args.output_dir) if args.command == 'check' else pack(args) if args.command == 'pack' else publish(args) if args.command == 'publish' else verify(args.reference,args.output_dir)
    print(json.dumps(result))

if __name__ == '__main__':
    try: main()
    except (RuntimeError, ValueError, OSError, KeyError) as e:
        print(str(e),file=sys.stderr)
        sys.exit(1)
