# AWS deployment

Pulumi installs Docker on Amazon Linux and runs the existing `compose.yaml`.
Traefik exposes the UI, `/api`, WebSocket routes, and registry on port 80.
Backing services stay private. Diamant runs on client hosts.

## Deploy

Requires Pulumi, uv, an AWS login, and a default public subnet in `eu-north-1`.
For a fresh deployment using local Pulumi state, choose your own stack name
(`alice-dev` in this example):

```bash
pulumi login --local
just infra-init alice-dev
just infra-region eu-north-1
# Export temporary credentials if using aws login.
eval "$(aws configure export-credentials --profile default --format env)"
just infra-up
just infra-ip
```

For an existing stack, use `just infra-select alice-dev` instead of initializing it.
Replace `alice-dev` with your stack name. `dev` is only the recipes' default.
Pulumi stores this stack's configuration in `Pulumi.alice-dev.yaml`, including
its encryption salt. Keep the file and your passphrase backed up; personal stack
files are ignored by Git. Local deployment state is stored separately under
`~/.pulumi` and also needs a backup.

Subsequent deployments only need `just infra-up` while AWS credentials remain
valid. Each `just infra-up` resolves the latest published stable GitHub release.
Pulumi records the concrete tag in instance user data, so a newer release triggers
an instance replacement. An unchanged release does not trigger an update.
The release MUST include `ws-proxy.x86_64-unknown-linux-musl.tar.xz` and
`control-api.x86_64-unknown-linux-musl.tar.xz`, each with its `.sha256` file.
The updated release workflow publishes these assets. Older releases without
an API archive cannot be deployed by this path.

The instance downloads the resolved release from `Suntitle/Suntitle`, verifies
both checksums, and installs the binaries in an Alpine runtime image.
It does not compile Rust. Releases use a musl toolchain in Docker on the
`ubuntu-latest` GitHub runner. The release build rejects shared-library dependencies
and dynamic loaders, then checks each executable starts. The headless API, proxy,
and Diamant binaries do not require glibc. The optional Qt GUI is excluded.
Use a public fork by setting `infrastructure:releaseRepository` to `owner/repo`.
No GitHub token or local Docker is required; private release downloads are not
configured. Missing assets or checksum failures stop provisioning.

The current checkout's Compose configuration, installation scripts, and UI build
inputs are uploaded to private S3. Rust source, local `.env` files, certificates,
and extension payloads are excluded. The instance builds the UI image and
creates its own Kubernetes certificates and database password. Startup continues
after Pulumi finishes; inspect cloud-init to check application readiness.

Run `just infra-up` to deploy the latest release. Discovery failures or missing
release assets stop deployment before AWS resources are changed.
To pin a version or roll back, optionally set a specific tag:

```bash
pulumi -C infrastructure config set infrastructure:releaseVersion v1.2.3
```

Remove a previous pin to follow latest again:

```bash
pulumi -C infrastructure config rm infrastructure:releaseVersion
```

Setting `releaseVersion` to `latest` also enables automatic discovery. Keep the
checkout's deployment configuration and UI compatible with the selected release.
Release tags SHOULD remain immutable.

## Connect and inspect

On macOS, install the SSM client once:

```bash
brew install --cask session-manager-plugin
just infra-ssh
```

On the instance:

```bash
sudo cloud-init status --long
sudo tail -f /var/log/cloud-init-output.log
cd /opt/suntitle
sudo docker compose --env-file .env.local ps -a
sudo docker compose --env-file .env.local logs --follow
```

Open `http://<public-ip>`. Get the registry hostname with
`pulumi -C infrastructure stack output registryHost`. It defaults to
`registry.<public-ip>.nip.io`; set `infrastructure:registryHost` to use your DNS.
Client Diamant uses `--proxy-url ws://<public-ip>/register --artifact-plain-http`.
The deployment retains the local HTTP/WebSocket exposure and Vite server.

## Data and destruction

The Elastic IP and protected EBS disk survive instance replacement. Docker data,
Kubernetes credentials, and extension payloads live under `/srv/suntitle`.
Release version or deployment input updates replace the instance with downtime.
Local extension payloads go in `/srv/suntitle/extensions`, mounted at `/opt/suntitle/extensions` in the proxy.

To permanently delete everything, including database and registry data:

```bash
pulumi -C infrastructure stack --show-urns
pulumi -C infrastructure state unprotect 'DATA_VOLUME_URN'
just infra-destroy
```

Replace `DATA_VOLUME_URN` with the `suntitle-data` EBS volume's URN.
