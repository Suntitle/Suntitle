"""Verify deployment packaging, rendered provisioning, and retained PKI identities."""

import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile
import sys
import unittest
from unittest.mock import patch
from urllib.error import URLError


ROOT = Path(__file__).resolve().parent.parent
SPEC = importlib.util.spec_from_file_location('deployment', ROOT / 'infrastructure/deployment.py')
DEPLOYMENT = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(DEPLOYMENT)


class InfrastructureTest(unittest.TestCase):
    """Exercise deployable artifacts without contacting AWS or starting Docker."""

    def test_buildx_checksum_manifest_formats(self):
        """Accept SHA-256 binary/text markers and reject missing or invalid entries."""
        bootstrap = (ROOT / 'infrastructure/bootstrap.sh').read_text()
        start = bootstrap.index('buildx_sum=')
        end = bootstrap.index("printf '%s  %s", start)
        parser = bootstrap[start:end].replace('/tmp/buildx.sha256', '"$1"')
        script = 'set -euo pipefail\nBUILDX_VERSION=v0.38.2\n' + parser + 'printf "%s" "$buildx_sum"'
        digest = '7992ec09413b80ff57ba4e081f56fc833d7bac1d5e71143c6c809e7e6065f6d6'
        with tempfile.TemporaryDirectory() as temporary:
            manifest = Path(temporary) / 'checksums.txt'
            for marker in ['*', '']:
                with self.subTest(marker=marker):
                    manifest.write_text(f'{digest} {marker}buildx-v0.38.2.linux-amd64\n')
                    result = subprocess.run(['bash', '-c', script, 'checksum-test', str(manifest)],
                                            check=True, capture_output=True, text=True)
                    self.assertEqual(result.stdout, digest)
            for entry in ['other-artifact', 'bad-hash *buildx-v0.38.2.linux-amd64']:
                with self.subTest(entry=entry):
                    manifest.write_text(entry + '\n')
                    result = subprocess.run(['bash', '-c', script, 'checksum-test', str(manifest)],
                                            capture_output=True, text=True)
                    self.assertNotEqual(result.returncode, 0)
                    self.assertIn('Missing or invalid checksum', result.stderr)

    def test_archive_excludes_credentials_and_build_outputs(self):
        """Only eligible source files enter the reproducible deployment archive."""
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            paths = [
                'compose.yaml', 'compose.release.yaml', 'ui/src/main.tsx', 'ui/.env.local',
                'docker/ws-proxy/Dockerfile.release', 'docker/ws-proxy/Dockerfile',
                'infrastructure/install-release.sh',
                'ui/node_modules/secret', 'ui/dist/index.html',
                'ui/cypress/videos/large.mp4', 'pki/admin.key', '.env.local',
                'Cargo.toml', 'Cargo.lock', 'ws-proxy/Cargo.toml', 'ws-proxy/src/main.rs',
                'control-api/Cargo.toml', 'control-api/src/main.rs',
                'suntitle-core/Cargo.toml', 'suntitle-core/src/lib.rs',
                'diamant/Cargo.toml', 'diamant/src/main.rs',
                'diamant-ui/Cargo.toml', 'diamant-ui/src/lib.rs',
                'target/debug/ws-proxy', 'diamant/target/debug/diamant',
            ]
            for name in paths:
                path = root / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text(name)
            (root / 'ui/leaked-key').symlink_to(root / 'pki/admin.key')
            first = root / 'first.tar.gz'
            second = root / 'second.tar.gz'
            digest = DEPLOYMENT.build_source_archive(root, first)
            self.assertEqual(digest, DEPLOYMENT.build_source_archive(root, second))
            with tarfile.open(first) as archive:
                self.assertEqual(set(archive.getnames()), {
                    'compose.yaml', 'compose.release.yaml', 'ui/src/main.tsx',
                    'docker/ws-proxy/Dockerfile.release', 'infrastructure/install-release.sh',
                })
            (root / 'ui/src/main.tsx').write_text('updated')
            self.assertNotEqual(digest, DEPLOYMENT.build_source_archive(root, second))

    def test_latest_release_discovery_and_explicit_override(self):
        """Resolve fresh tags, bypass discovery for pins, and fail on unusable releases."""
        assets = [{'name': f'{component}.x86_64-unknown-linux-musl.tar.xz{suffix}'}
                  for component in ['ws-proxy', 'control-api'] for suffix in ['', '.sha256']]
        for requested, tag in [(None, 'v1.2.3'), ('latest', 'v1.2.4')]:
            with self.subTest(requested=requested), patch.object(DEPLOYMENT, 'urlopen') as lookup:
                lookup.return_value.__enter__.return_value = io.StringIO(
                    json.dumps({'tag_name': tag, 'assets': assets}))
                self.assertEqual(DEPLOYMENT.resolve_release(requested, 'owner/repo'), tag)
                request = lookup.call_args.args[0]
                self.assertEqual(request.full_url, 'https://api.github.com/repos/owner/repo/releases/latest')
        with patch.object(DEPLOYMENT, 'urlopen') as lookup:
            self.assertEqual(DEPLOYMENT.resolve_release('v1.0.0', 'owner/repo'), 'v1.0.0')
            lookup.assert_not_called()
        for payload in [{'tag_name': 'v1.2.3', 'assets': []}, {}, {'tag_name': 'main'}]:
            with self.subTest(payload=payload), patch.object(DEPLOYMENT, 'urlopen') as lookup:
                lookup.return_value.__enter__.return_value = io.StringIO(json.dumps(payload))
                with self.assertRaises(ValueError):
                    DEPLOYMENT.resolve_release(None, 'owner/repo')
        with patch.object(DEPLOYMENT, 'urlopen', side_effect=URLError('unavailable')):
            with self.assertRaisesRegex(ValueError, 'Cannot resolve the latest release'):
                DEPLOYMENT.resolve_release(None, 'owner/repo')

    def test_release_configuration_and_rendered_bootstrap(self):
        """Require pinned releases and render a valid script selecting the release overlay."""
        arguments = dict(bucket='source-bucket', key='digest.tar.gz', region='eu-north-1',
                         volume_id='vol-123', registry_host='registry.example.com',
                         compose_version='v5.6.0', buildx_version='v0.38.2',
                         release_version='v1.2.3-rc.1', release_repository='owner/Suntitle')
        script = DEPLOYMENT.render_user_data(**arguments)
        subprocess.run(['bash', '-n'], input=script, text=True, check=True)
        self.assertIn('export SUNTITLE_RELEASE_VERSION=v1.2.3-rc.1', script)
        self.assertIn('export SUNTITLE_RELEASE_REPOSITORY=owner/Suntitle', script)
        self.assertIn('COMPOSE_FILE=compose.yaml:compose.release.yaml', script)
        for version in ['', 'latest', 'main', 'v1/other', 'v1\nmalformed']:
            with self.subTest(version=version), self.assertRaises(ValueError):
                DEPLOYMENT.render_user_data(**(arguments | {'release_version': version}))
        for repository in ['https://github.com/owner/repo', 'owner/repo/extra', 'owner/repo?query']:
            with self.subTest(repository=repository), self.assertRaises(ValueError):
                DEPLOYMENT.render_user_data(**(arguments | {'release_repository': repository}))

    @unittest.skipUnless(shutil.which('docker'), 'Docker Compose is required')
    def test_compose_release_overlay_uses_runtime_image(self):
        """Resolve deployment Compose without inheriting the Rust source Dockerfile."""
        environment = {key: value for key, value in os.environ.items()
                       if key not in {'COMPOSE_FILE', 'SUNTITLE_RELEASE_VERSION',
                                      'SUNTITLE_RELEASE_REPOSITORY'}}
        with tempfile.TemporaryDirectory() as temporary:
            dotenv = Path(temporary) / '.env.local'
            dotenv.write_text(f'COMPOSE_FILE={ROOT}/compose.yaml:{ROOT}/compose.release.yaml\n'
                              'SUNTITLE_RELEASE_VERSION=v1.2.3\n'
                              'SUNTITLE_RELEASE_REPOSITORY=owner/repo\n')
            output = subprocess.check_output([
                'docker', 'compose', '--env-file', str(dotenv), 'config', '--format', 'json',
            ], cwd=ROOT, env=environment, text=True)
        services = json.loads(output)['services']
        build = services['kubernetes-bootstrap']['build']
        self.assertEqual(build['dockerfile'], 'docker/ws-proxy/Dockerfile.release')
        self.assertEqual(build['args']['SUNTITLE_RELEASE_VERSION'], 'v1.2.3')
        self.assertEqual(build['args']['SUNTITLE_RELEASE_REPOSITORY'], 'owner/repo')
        for service in ['kubernetes-bootstrap', 'api', 'ws-proxy']:
            self.assertEqual(services[service]['image'], 'suntitle-control:v1.2.3')
        self.assertEqual(services['ui']['build']['context'], str(ROOT / 'ui'))

    @unittest.skipUnless(shutil.which('sha256sum'), 'sha256sum is required')
    def test_release_installer_verifies_both_archives_before_installing(self):
        """Install mock release executables and reject corrupt, absent, or mismatched assets."""
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            fixtures = root / 'fixtures'
            fixtures.mkdir()
            tools = root / 'tools'
            tools.mkdir()
            curl = tools / 'curl'
            curl.write_text(f'#!{sys.executable}\n' +
                            'import os, pathlib, shutil, sys\n'
                            'args = sys.argv[1:]\n'
                            'url = args[args.index("-o") - 1]\n'
                            'assert url.startswith("https://github.com/owner/repo/releases/download/v1.2.3/")\n'
                            'shutil.copyfile(pathlib.Path(os.environ["RELEASE_FIXTURES"]) / url.rsplit("/", 1)[1], args[args.index("-o") + 1])\n')
            curl.chmod(0o755)
            for component, binary in [('ws-proxy', 'ws-proxy'), ('control-api', 'control_api')]:
                archive = fixtures / f'{component}.x86_64-unknown-linux-musl.tar.xz'
                payload = b'#!/bin/sh\nprintf "release executable\\n"\n'
                with tarfile.open(archive, 'w:xz') as output:
                    info = tarfile.TarInfo(binary)
                    info.size = len(payload)
                    info.mode = 0o755
                    output.addfile(info, io.BytesIO(payload))
                digest = hashlib.sha256(archive.read_bytes()).hexdigest()
                archive.with_suffix('.xz.sha256').write_text(f'{digest}  {archive.name}\n')
            environment = os.environ | {'PATH': str(tools) + os.pathsep + os.environ['PATH'],
                                        'RELEASE_FIXTURES': str(fixtures)}
            destination = root / 'installed'
            command = ['bash', str(ROOT / 'infrastructure/install-release.sh'),
                       'v1.2.3', 'owner/repo', str(destination)]
            subprocess.run(command, env=environment, check=True, capture_output=True)
            for binary in ['ws-proxy', 'control_api']:
                output = subprocess.check_output([str(destination / binary)], text=True)
                self.assertEqual(output, 'release executable\n')
            shutil.rmtree(destination)
            archive = fixtures / 'control-api.x86_64-unknown-linux-musl.tar.xz'
            original = archive.read_bytes()
            archive.write_bytes(original + b'corrupt')
            result = subprocess.run(command, env=environment, capture_output=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertFalse(destination.exists())
            archive.write_bytes(original)
            checksum = archive.with_suffix('.xz.sha256')
            checksum.write_text('0' * 64 + '  wrong-archive.tar.xz\n')
            result = subprocess.run(command, env=environment, capture_output=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertFalse(destination.exists())
            checksum.unlink()
            result = subprocess.run(command, env=environment, capture_output=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertFalse(destination.exists())

    def test_static_binary_checks_reject_dependencies_and_loaders(self):
        """Reject dynamic ELF requirements and inspection or startup failures."""
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            inspector = root / 'readelf'
            inspector.write_text(
                '#!/bin/sh\n'
                'case "$ELF_CASE:$1" in\n'
                '  dependency:-d) echo "(NEEDED) Shared library: [libc.so.6]" ;;\n'
                '  loader:-l) echo "INTERP /lib/ld-musl-x86_64.so.1" ;;\n'
                '  error:*) exit 1 ;;\n'
                'esac\n')
            inspector.chmod(0o755)
            binary = root / 'binary'
            binary.write_text('#!/bin/sh\n[ "$ELF_CASE" != startup-error ]\n')
            binary.chmod(0o755)
            for case in ['static', 'dependency', 'loader', 'error', 'startup-error']:
                with self.subTest(case=case):
                    environment = os.environ | {'PATH': str(root) + os.pathsep + os.environ['PATH'],
                                                'ELF_CASE': case}
                    result = subprocess.run([
                        'sh', str(ROOT / 'docker/ws-proxy/check-static.sh'), str(binary),
                    ], env=environment, capture_output=True)
                    self.assertEqual(result.returncode == 0, case == 'static')

    @unittest.skipUnless(shutil.which('openssl'), 'OpenSSL is required')
    def test_pki_verifies_and_retains_identity(self):
        """Bootstrap certificates validate, and reruns preserve keys and identities."""
        with tempfile.TemporaryDirectory() as temporary:
            command = ['bash', str(ROOT / 'infrastructure/generate-pki.sh')]
            subprocess.run(command, cwd=temporary, check=True, capture_output=True)
            pki = Path(temporary) / 'pki'
            for identity in ['admin', 'suntitle-api', 'suntitle-proxy']:
                result = subprocess.run([
                    'openssl', 'verify', '-purpose', 'sslclient', '-CAfile',
                    str(pki / 'ca.crt'), str(pki / f'{identity}.crt'),
                ], check=True, capture_output=True, text=True)
                self.assertIn('OK', result.stdout)
                config = (pki / f'{identity}.compose.kubeconfig').read_text()
                self.assertIn('server: https://kube-apiserver:6443', config)
                self.assertIn(f'client-key: /etc/kubernetes/pki/{identity}.key', config)
            subprocess.run([
                'openssl', 'verify', '-purpose', 'sslserver', '-CAfile', str(pki / 'ca.crt'),
                str(pki / 'apiserver.crt'),
            ], check=True, capture_output=True)
            certificate = subprocess.run([
                'openssl', 'x509', '-in', str(pki / 'apiserver.crt'), '-text', '-noout',
            ], check=True, capture_output=True, text=True).stdout
            self.assertIn('DNS:kube-apiserver', certificate)
            self.assertIn('IP Address:127.0.0.1', certificate)
            originals = {path.name: path.read_bytes() for path in pki.glob('*.key')}
            ca_key = Path(temporary) / 'private-ca/ca.key'
            original_ca_key = ca_key.read_bytes()
            self.assertFalse((pki / 'ca.key').exists())
            subprocess.run(command, cwd=temporary, check=True, capture_output=True)
            self.assertEqual(originals, {path.name: path.read_bytes() for path in pki.glob('*.key')})
            self.assertEqual(original_ca_key, ca_key.read_bytes())


if __name__ == '__main__':
    unittest.main()
