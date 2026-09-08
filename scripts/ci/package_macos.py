"""Retain the smoke-tested stable executable; never execute or install it."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import stat
import subprocess
import tarfile


def read(*command):
    return subprocess.check_output(command, text=True, timeout=30).strip()


def sha256(path):
    with path.open('rb') as source:
        return stream_sha256(source)


def stream_sha256(source):
    digest = hashlib.sha256()
    for chunk in iter(lambda: source.read(1024 * 1024), b''):
        digest.update(chunk)
    return digest.hexdigest()


def checkout():
    # On pull_request this is normally GitHub's synthetic merge commit, not PR head.
    return {'commit': read('git', 'rev-parse', 'HEAD'),
            'tree': read('git', 'rev-parse', 'HEAD^{tree}')}


def provenance(binary):
    rust = read('rustc', '-vV')
    hosts = [line.removeprefix('host: ') for line in rust.splitlines() if line.startswith('host: ')]
    arches = read('/usr/bin/lipo', '-archs', str(binary)).split()
    if platform.system() != 'Darwin' or hosts != ['aarch64-apple-darwin'] or arches != ['arm64']:
        raise RuntimeError('artifact requires an arm64 Mach-O on the aarch64 macOS Rust host')
    required = ('GITHUB_REPOSITORY', 'GITHUB_RUN_ID', 'GITHUB_RUN_ATTEMPT',
                'GITHUB_EVENT_NAME', 'GITHUB_REF', 'GITHUB_SHA', 'GITHUB_SERVER_URL', 'GITHUB_WORKFLOW')
    context = {key: os.environ[key] for key in required}
    if context['GITHUB_EVENT_NAME'] not in ('push', 'pull_request'):
        raise RuntimeError('artifacts are only produced for main/PR CI')
    return {'schema_version': 1, 'checkout': checkout(), 'channel': 'stable',
            'rust_host': hosts[0], 'rustc_verbose_version': rust, 'binary_architecture': arches[0],
            'build_command': 'cargo build --locked --release -p locald-cli --bin locald --features channel-stable',
            'github': context,
            'run_url': f"{context['GITHUB_SERVER_URL']}/{context['GITHUB_REPOSITORY']}/actions/runs/{context['GITHUB_RUN_ID']}"}


def stage_binary(binary, stage):
    binary = binary.resolve(strict=True)
    mode = binary.stat().st_mode
    if not stat.S_ISREG(mode) or not mode & stat.S_IXUSR:
        raise RuntimeError('smoke-tested binary must be a regular owner-executable file')
    manifest = provenance(binary)
    manifest['binary'] = {'name': 'locald', 'sha256': sha256(binary), 'mode': stat.S_IMODE(mode)}
    stage.mkdir(parents=True, exist_ok=False)
    shutil.copy2(binary, stage / 'locald')
    if sha256(stage / 'locald') != manifest['binary']['sha256']:
        raise RuntimeError('binary changed while preserving smoke-tested bytes')
    (stage / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
    # Internal-only path: not shipped, used to detect later release-binary replacement.
    (stage / 'source-path.txt').write_text(str(binary))


def package_binary(stage, output):
    manifest = json.loads((stage / 'manifest.json').read_text())
    binary = stage / 'locald'
    original = Path((stage / 'source-path.txt').read_text())
    expected = manifest['binary']
    # Unit suites may compile other channels. Neither the retained copy nor the
    # release path may drift from the bytes that passed the stable smoke checks.
    if sha256(binary) != expected['sha256'] or sha256(original) != expected['sha256']:
        raise RuntimeError('stable binary changed after smoke tests')
    if stat.S_IMODE(binary.stat().st_mode) != expected['mode'] or not expected['mode'] & stat.S_IXUSR:
        raise RuntimeError('stable binary executable permissions changed')
    if checkout() != manifest['checkout']:
        raise RuntimeError('checkout changed after smoke tests')
    output.mkdir(parents=True, exist_ok=False)
    archive = output / 'locald-macos-aarch64.tar.gz'
    with tarfile.open(archive, 'w:gz') as bundle:
        bundle.add(binary, arcname='locald', recursive=False)
        bundle.add(stage / 'manifest.json', arcname='manifest.json', recursive=False)
    # Verify what will be uploaded, including tar's executable-bit preservation.
    with tarfile.open(archive, 'r:gz') as bundle:
        member = bundle.getmember('locald')
        with bundle.extractfile(member) as payload:
            if stream_sha256(payload) != expected['sha256'] or member.mode != expected['mode']:
                raise RuntimeError('archive differs from the smoke-tested binary')
    artifact = {**manifest, 'archive': {'name': archive.name, 'sha256': sha256(archive)}}
    receipt = output / 'artifact.json'
    receipt.write_text(json.dumps(artifact, indent=2) + '\n')
    (output / 'SHA256SUMS').write_text(
        f'{sha256(archive)}  {archive.name}\n{sha256(receipt)}  {receipt.name}\n')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    modes = parser.add_subparsers(dest='mode', required=True)
    stage = modes.add_parser('stage')
    stage.add_argument('--binary', required=True, type=Path)
    stage.add_argument('--stage', required=True, type=Path)
    package = modes.add_parser('package')
    package.add_argument('--stage', required=True, type=Path)
    package.add_argument('--output', required=True, type=Path)
    args = parser.parse_args()
    if args.mode == 'stage':
        stage_binary(args.binary, args.stage)
    else:
        package_binary(args.stage, args.output)


if __name__ == '__main__':
    main()
