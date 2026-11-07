"""Package deployment inputs and render first-boot provisioning.

Pulumi uploads deployment inputs before EC2 starts. Cloud-init waits for the data
attachment, mounts persistent Docker storage, prepares credentials, pulls selected
container images, then starts Compose.
Container restart policies own subsequent boot recovery. Provisioning failure
remains visible in cloud-init logs; Pulumi resource creation is not an application
readiness signal.
"""

import gzip
import hashlib
from pathlib import Path
import re
import shlex
import tarfile


# Only deployment inputs are eligible. Local environments and credentials MUST stay local.
SOURCE_PATHS = (
    'compose.yaml', 'compose.release.yaml', 'docker/distribution/config.yml',
    'infrastructure/generate-pki.sh',
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
                     registry_host: str, compose_version: str, images: dict[str, str]) -> str:
    """Render shell-safe cloud-init inputs without embedding local credentials."""
    for component in ('api', 'proxy', 'ui'):
        validate_image(images[component])
    values = {
        'SOURCE_BUCKET': bucket, 'SOURCE_KEY': key, 'AWS_DEFAULT_REGION': region,
        'DATA_VOLUME_ID': volume_id, 'REGISTRY_HOST': registry_host,
        'COMPOSE_VERSION': compose_version,
        'SUNTITLE_API_IMAGE': images['api'],
        'SUNTITLE_PROXY_IMAGE': images['proxy'],
        'SUNTITLE_UI_IMAGE': images['ui'],
    }
    assignments = '\n'.join(f'export {name}={shlex.quote(value)}' for name, value in values.items())
    script = Path(__file__).with_name('bootstrap.sh').read_text()
    return script.replace('# @DEPLOYMENT_ENV@', assignments)


def validate_image(image: str) -> None:
    """Require a Docker reference that is safe in Compose dotenv values."""
    if not isinstance(image, str) or not re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9._:/@-]*', image) or '://' in image:
        raise ValueError('Image sources MUST be Docker image references without whitespace or URL schemes.')


def resolve_images(api_image: str | None = None, proxy_image: str | None = None,
                   ui_image: str | None = None) -> dict[str, str]:
    """Select GHCR latest images with validated per-component overrides.

    Selection MUST complete before AWS resources exist and requires no network lookup.
    """
    overrides = {'api': api_image, 'proxy': proxy_image, 'ui': ui_image}
    components = {'api': 'control-api', 'proxy': 'ws-proxy', 'ui': 'ui'}
    images = {name: image if image is not None else f'ghcr.io/suntitle/suntitle/{components[name]}:latest'
              for name, image in overrides.items()}
    for image in images.values():
        validate_image(image)
    return images
