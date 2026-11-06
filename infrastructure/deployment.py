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
import json
from pathlib import Path
import re
import shlex
import tarfile
from urllib.error import URLError
from urllib.request import Request, urlopen


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


def validate_release(version: str, repository: str) -> None:
    """Require a pinned release tag and an owner/repository GitHub path."""
    if not re.fullmatch(r'v[0-9][A-Za-z0-9._-]*', version):
        raise ValueError('releaseVersion MUST be a pinned tag beginning with v and a digit.')
    if not re.fullmatch(r'[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+', repository):
        raise ValueError('releaseRepository MUST use owner/repository format.')


def resolve_release(version: str | None, repository: str) -> str:
    """Resolve latest during Pulumi evaluation so a newer tag changes instance user data.

    Explicit tags bypass discovery. Lookup failures MUST stop deployment before
    resource creation. Installation always receives a concrete validated tag.
    """
    if version and version != 'latest':
        validate_release(version, repository)
        return version
    validate_release('v0', repository)
    request = Request(
        f'https://api.github.com/repos/{repository}/releases/latest',
        headers={'Accept': 'application/vnd.github+json',
                 'User-Agent': 'suntitle-pulumi'},
    )
    try:
        with urlopen(request, timeout=30) as response:
            release = json.load(response)
    except (URLError, TimeoutError, ValueError) as error:
        raise ValueError(f'Cannot resolve the latest release of {repository}: {error}') from error
    version = release.get('tag_name')
    if not isinstance(version, str):
        raise ValueError(f'The latest release of {repository} has no tag_name.')
    validate_release(version, repository)
    return version


def validate_image(image: str) -> None:
    """Require a Docker reference that is safe in Compose dotenv values."""
    if not isinstance(image, str) or not re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9._:/@-]*', image) or '://' in image:
        raise ValueError('Image sources MUST be Docker image references without whitespace or URL schemes.')


def resolve_images(version: str | None, repository: str,
                   api_image: str | None = None, proxy_image: str | None = None,
                   ui_image: str | None = None) -> tuple[str | None, dict[str, str]]:
    """Select release images with per-component overrides before AWS resources exist.

    Full overrides MUST skip GitHub discovery. Partial overrides share one resolved
    release tag for remaining components. The returned tag is None for full overrides.
    """
    overrides = {'api': api_image, 'proxy': proxy_image, 'ui': ui_image}
    for image in overrides.values():
        if image is not None:
            validate_image(image)
    release = resolve_release(version, repository) if None in overrides.values() else None
    components = {'api': 'control-api', 'proxy': 'ws-proxy', 'ui': 'ui'}
    images = {name: image if image is not None else f'ghcr.io/{repository.lower()}/{components[name]}:{release}'
              for name, image in overrides.items()}
    return release, images
