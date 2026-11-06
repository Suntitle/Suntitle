"""Package deployment inputs and render first-boot provisioning.

Pulumi uploads deployment inputs before EC2 starts. Cloud-init waits for the data
attachment, mounts persistent Docker storage, prepares credentials, installs
released binaries in a runtime image, builds the UI image, then starts Compose.
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


# Only build inputs are eligible. Local environments and credentials MUST stay local.
SOURCE_PATHS = (
    'compose.yaml', 'compose.release.yaml', 'docker/distribution/config.yml',
    'docker/ws-proxy/Dockerfile.release', 'infrastructure/generate-pki.sh',
    'infrastructure/install-release.sh',
    'ui/Dockerfile', 'ui/.dockerignore', 'ui/package.json',
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
                     registry_host: str, compose_version: str, buildx_version: str,
                     release_version: str, release_repository: str) -> str:
    """Render shell-safe cloud-init inputs without embedding local credentials."""
    validate_release(release_version, release_repository)
    values = {
        'SOURCE_BUCKET': bucket, 'SOURCE_KEY': key, 'AWS_DEFAULT_REGION': region,
        'DATA_VOLUME_ID': volume_id, 'REGISTRY_HOST': registry_host,
        'COMPOSE_VERSION': compose_version, 'BUILDX_VERSION': buildx_version,
        'SUNTITLE_RELEASE_VERSION': release_version,
        'SUNTITLE_RELEASE_REPOSITORY': release_repository,
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
    assets = {asset['name'] for asset in release.get('assets', [])}
    required = {
        f'{component}.x86_64-unknown-linux-musl.tar.xz{suffix}'
        for component in ['ws-proxy', 'control-api'] for suffix in ['', '.sha256']
    }
    missing = required - assets
    if missing:
        raise ValueError(f'Release {version} is missing deployment assets: {", ".join(sorted(missing))}')
    return version
