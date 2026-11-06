"""Package deployment inputs and render first-boot provisioning.

Pulumi uploads source before EC2 starts. Cloud-init waits for the data attachment,
mounts persistent Docker storage, prepares credentials, builds images, then starts
Compose. Container restart policies own subsequent boot recovery. Provisioning
failure remains visible in cloud-init logs; Pulumi resource creation is not an
application readiness signal.
"""

import gzip
import hashlib
from pathlib import Path
import shlex
import tarfile


# Only build inputs are eligible. Local environments and credentials MUST stay local.
SOURCE_PATHS = (
    'compose.yaml', 'docker', 'kubernetes', 'specs', 'infrastructure/generate-pki.sh',
    'ws-proxy/Cargo.toml', 'ws-proxy/Cargo.lock', 'ws-proxy/src',
    'ws-proxy/diamant-ui', 'oci-artifact/Cargo.toml', 'oci-artifact/README.md',
    'oci-artifact/src', 'ui/Dockerfile', 'ui/.dockerignore', 'ui/package.json',
    'ui/package-lock.json', 'ui/index.html', 'ui/vite.config.ts',
    'ui/tsconfig.json', 'ui/tsconfig.app.json', 'ui/tsconfig.node.json',
    'ui/src', 'ui/public',
)


def build_source_archive(root: Path, destination: Path) -> str:
    """Create a deterministic allowlisted archive and return its SHA-256 digest."""
    files = set()
    for relative in SOURCE_PATHS:
        source = root / relative
        candidates = source.rglob('*') if source.is_dir() else [source]
        for path in candidates:
            parts = path.relative_to(root).parts
            if any(part in {'node_modules', 'target', 'dist', '.git', '__pycache__', 'screenshots', 'videos'}
                   or part.startswith('.env') for part in parts):
                continue
            if path.is_file() and not path.is_symlink():
                files.add(path)
    destination.parent.mkdir(parents=True, exist_ok=True)
    with destination.open('wb') as raw, gzip.GzipFile(fileobj=raw, mode='wb', mtime=0, filename='') as compressed:
        with tarfile.open(fileobj=compressed, mode='w') as archive:
            for path in sorted(files):
                info = archive.gettarinfo(str(path), arcname=str(path.relative_to(root)))
                info.uid = info.gid = info.mtime = 0
                info.uname = info.gname = ''
                info.mode = 0o644
                with path.open('rb') as contents:
                    archive.addfile(info, contents)
    return hashlib.sha256(destination.read_bytes()).hexdigest()


def render_user_data(bucket: str, key: str, region: str, volume_id: str,
                     registry_host: str, compose_version: str, buildx_version: str) -> str:
    """Render shell-safe cloud-init inputs without embedding local credentials."""
    values = {
        'SOURCE_BUCKET': bucket, 'SOURCE_KEY': key, 'AWS_DEFAULT_REGION': region,
        'DATA_VOLUME_ID': volume_id, 'REGISTRY_HOST': registry_host,
        'COMPOSE_VERSION': compose_version, 'BUILDX_VERSION': buildx_version,
    }
    assignments = '\n'.join(f'export {name}={shlex.quote(value)}' for name, value in values.items())
    script = Path(__file__).with_name('bootstrap.sh').read_text()
    return script.replace('# @DEPLOYMENT_ENV@', assignments)
