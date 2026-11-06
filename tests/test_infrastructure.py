"""Verify deployment packaging, rendered provisioning, and retained PKI identities."""

import importlib.util
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile
import unittest


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
                'compose.yaml', 'ui/src/main.tsx', 'ui/.env.local',
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
                    'compose.yaml', 'ui/src/main.tsx', 'Cargo.toml', 'Cargo.lock',
                    'ws-proxy/Cargo.toml', 'ws-proxy/src/main.rs',
                    'control-api/Cargo.toml', 'control-api/src/main.rs',
                    'suntitle-core/Cargo.toml', 'suntitle-core/src/lib.rs',
                    'diamant/Cargo.toml', 'diamant/src/main.rs',
                    'diamant-ui/Cargo.toml', 'diamant-ui/src/lib.rs',
                })
            (root / 'ui/src/main.tsx').write_text('updated')
            self.assertNotEqual(digest, DEPLOYMENT.build_source_archive(root, second))

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
