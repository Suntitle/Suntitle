"""Verify deployment packaging, rendered provisioning, and retained PKI identities."""

import importlib.util
import io
import json
import os
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile
import unittest
from unittest.mock import patch
from urllib.error import URLError


ROOT = Path(__file__).resolve().parent.parent
SPEC = importlib.util.spec_from_file_location('deployment', ROOT / 'infrastructure/deployment.py')
DEPLOYMENT = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(DEPLOYMENT)


class InfrastructureTest(unittest.TestCase):
    """Exercise deployable artifacts without contacting AWS or starting Docker."""

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
                    'compose.yaml', 'compose.release.yaml',
                })
            (root / 'ui/src/main.tsx').write_text('updated')
            self.assertEqual(digest, DEPLOYMENT.build_source_archive(root, second))
            (root / 'compose.release.yaml').write_text('updated')
            self.assertNotEqual(digest, DEPLOYMENT.build_source_archive(root, second))

    def test_latest_release_discovery_and_explicit_override(self):
        """Resolve fresh tags, bypass discovery for pins, and fail on unusable releases."""
        for requested, tag in [(None, 'v1.2.3'), ('latest', 'v1.2.4')]:
            with self.subTest(requested=requested), patch.object(DEPLOYMENT, 'urlopen') as lookup:
                lookup.return_value.__enter__.return_value = io.StringIO(
                    json.dumps({'tag_name': tag}))
                self.assertEqual(DEPLOYMENT.resolve_release(requested, 'owner/repo'), tag)
                request = lookup.call_args.args[0]
                self.assertEqual(request.full_url, 'https://api.github.com/repos/owner/repo/releases/latest')
        with patch.object(DEPLOYMENT, 'urlopen') as lookup:
            self.assertEqual(DEPLOYMENT.resolve_release('v1.0.0', 'owner/repo'), 'v1.0.0')
            lookup.assert_not_called()
        for payload in [{}, {'tag_name': 'main'}]:
            with self.subTest(payload=payload), patch.object(DEPLOYMENT, 'urlopen') as lookup:
                lookup.return_value.__enter__.return_value = io.StringIO(json.dumps(payload))
                with self.assertRaises(ValueError):
                    DEPLOYMENT.resolve_release(None, 'owner/repo')
        with patch.object(DEPLOYMENT, 'urlopen', side_effect=URLError('unavailable')):
            with self.assertRaisesRegex(ValueError, 'Cannot resolve the latest release'):
                DEPLOYMENT.resolve_release(None, 'owner/repo')

    def test_release_configuration_and_rendered_bootstrap(self):
        """Render selected images safely and require pulls without instance-side builds."""
        arguments = dict(bucket='source-bucket', key='digest.tar.gz', region='eu-north-1',
                         volume_id='vol-123', registry_host='registry.example.com',
                         compose_version='v5.6.0', images={'api': 'example/api:v2',
                         'proxy': 'example/proxy:v2', 'ui': 'example/ui:v2'})
        script = DEPLOYMENT.render_user_data(**arguments)
        subprocess.run(['bash', '-n'], input=script, text=True, check=True)
        self.assertIn('export SUNTITLE_API_IMAGE=example/api:v2', script)
        self.assertIn('docker compose --env-file .env.local pull', script)
        self.assertIn('up --no-build', script)
        self.assertNotIn('docker compose --env-file .env.local build', script)
        self.assertIn('COMPOSE_FILE=compose.yaml:compose.release.yaml', script)
        for image in ['', 'https://registry/image', 'image\nINJECTED=x', 'image${VALUE}', 'image name', 'image#comment']:
            with self.subTest(image=image), self.assertRaises(ValueError):
                DEPLOYMENT.render_user_data(**(arguments | {'images': arguments['images'] | {'api': image}}))

    def test_pull_failure_stops_stack_start(self):
        """Pull failure MUST prevent startup; successful pulls MUST use --no-build."""
        bootstrap = (ROOT / 'infrastructure/bootstrap.sh').read_text()
        script = 'set -e\n' + bootstrap[bootstrap.index('docker compose --env-file .env.local pull'):]
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            docker = root / 'docker'
            docker.write_text('#!/bin/sh\nprintf "%s\\n" "$*" >> "$DOCKER_CALLS"\n'
                              'case "$*" in *" pull") exit "$PULL_EXIT" ;; esac\n')
            docker.chmod(0o755)
            calls = root / 'calls'
            for exit_code in [0, 1]:
                with self.subTest(exit_code=exit_code):
                    calls.write_text('')
                    environment = os.environ | {'PATH': str(root) + os.pathsep + os.environ['PATH'],
                                                'DOCKER_CALLS': str(calls), 'PULL_EXIT': str(exit_code)}
                    result = subprocess.run(['bash', '-c', script], env=environment, capture_output=True)
                    self.assertEqual(result.returncode, exit_code)
                    commands = calls.read_text().splitlines()
                    self.assertEqual(len(commands), 3 if exit_code == 0 else 1)
                    if exit_code == 0:
                        self.assertIn('up --no-build', commands[1])

    def test_image_overrides_and_release_defaults(self):
        """Resolve missing images once; full overrides MUST work without GitHub."""
        digest = 'registry.example/api@sha256:' + 'a' * 64
        with patch.object(DEPLOYMENT, 'resolve_release', return_value='v1.2.3') as lookup:
            version, images = DEPLOYMENT.resolve_images(None, 'Suntitle/Suntitle')
            self.assertEqual(version, 'v1.2.3')
            self.assertEqual(images, {'api': 'ghcr.io/suntitle/suntitle/control-api:v1.2.3',
                                     'proxy': 'ghcr.io/suntitle/suntitle/ws-proxy:v1.2.3',
                                     'ui': 'ghcr.io/suntitle/suntitle/ui:v1.2.3'})
            lookup.assert_called_once_with(None, 'Suntitle/Suntitle')
        with patch.object(DEPLOYMENT, 'resolve_release', return_value='v2') as lookup:
            _, images = DEPLOYMENT.resolve_images('v2', 'owner/fork', api_image=digest)
            self.assertEqual(images['api'], digest)
            self.assertEqual(images['proxy'], 'ghcr.io/owner/fork/ws-proxy:v2')
            lookup.assert_called_once()
        with patch.object(DEPLOYMENT, 'urlopen') as lookup:
            version, images = DEPLOYMENT.resolve_images(None, 'Suntitle/Suntitle',
                api_image=digest, proxy_image='custom/proxy:latest', ui_image='custom/ui:v3')
            self.assertIsNone(version)
            self.assertEqual(images, {'api': digest, 'proxy': 'custom/proxy:latest', 'ui': 'custom/ui:v3'})
            lookup.assert_not_called()
        with patch.object(DEPLOYMENT, 'urlopen') as lookup:
            with self.assertRaises(ValueError):
                DEPLOYMENT.resolve_images(None, 'Suntitle/Suntitle', api_image='image\nINJECTED=true')
            lookup.assert_not_called()

    @unittest.skipUnless(shutil.which('docker'), 'Docker Compose is required')
    def test_compose_release_overlay_uses_runtime_image(self):
        """Resolve deployment Compose without inheriting the Rust source Dockerfile."""
        environment = {key: value for key, value in os.environ.items()
                       if key not in {'COMPOSE_FILE', 'SUNTITLE_API_IMAGE', 'SUNTITLE_PROXY_IMAGE', 'SUNTITLE_UI_IMAGE'}}
        with tempfile.TemporaryDirectory() as temporary:
            dotenv = Path(temporary) / '.env.local'
            dotenv.write_text(f'COMPOSE_FILE={ROOT}/compose.yaml:{ROOT}/compose.release.yaml\n'
                              'SUNTITLE_API_IMAGE=custom/api:v1\n'
                              'SUNTITLE_PROXY_IMAGE=custom/proxy:v2\n'
                              'SUNTITLE_UI_IMAGE=custom/ui:v3\n')
            output = subprocess.check_output([
                'docker', 'compose', '--env-file', str(dotenv), 'config', '--format', 'json',
            ], cwd=ROOT, env=environment, text=True)
        services = json.loads(output)['services']
        self.assertEqual(services['kubernetes-bootstrap']['image'], 'custom/api:v1')
        self.assertEqual(services['api']['image'], 'custom/api:v1')
        self.assertEqual(services['ws-proxy']['image'], 'custom/proxy:v2')
        self.assertEqual(services['ui']['image'], 'custom/ui:v3')
        for service in services.values():
            self.assertNotIn('build', service)
        local = json.loads(subprocess.check_output([
            'docker', 'compose', '--env-file', '/dev/null', '-f', 'compose.yaml',
            'config', '--format', 'json',
        ], cwd=ROOT, env=environment, text=True))['services']
        self.assertEqual(local['kubernetes-bootstrap']['build']['target'], 'control-api')
        self.assertEqual(local['ws-proxy']['build']['target'], 'ws-proxy')
        self.assertEqual(local['api']['image'], 'suntitle-control-api:local')
        self.assertEqual(local['ws-proxy']['image'], 'suntitle-ws-proxy:local')

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
