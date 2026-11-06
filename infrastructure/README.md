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
Pulumi resolves the release tag into images under
`ghcr.io/suntitle/suntitle`: `control-api:<tag>`, `ws-proxy:<tag>`, and `ui:<tag>`.
The release workflow publishes these images. The API image also runs Kubernetes
bootstrap. EC2 pulls images and runs Compose without compiling or building images.

Use `infrastructure:releaseRepository` to select a public fork (`owner/repo`);
its default image prefix becomes `ghcr.io/owner/repo`. The selected images MUST
be readable by the instance. Registry authentication is not configured here;
public GHCR packages permit anonymous pulls.

For custom image sources, set any of:

```bash
pulumi -C infrastructure config set infrastructure:apiImage registry.example.com/suntitle/api:v2
pulumi -C infrastructure config set infrastructure:proxyImage registry.example.com/suntitle/proxy:v2
pulumi -C infrastructure config set infrastructure:uiImage registry.example.com/suntitle/ui:v2
```

References may use tags or digests (`registry.example.com/image@sha256:...`).
An override replaces just that component. If all three images are supplied,
Pulumi skips GitHub discovery and needs no release tag. The `images` stack output
shows the exact selected references. Custom images MUST support the existing
Compose commands, ports, health checks, and mounted configuration; API images
MUST include the `control_api` bootstrap commands.

Run `just infra-up` to select the latest release. To pin a release or roll back:

```bash
pulumi -C infrastructure config set infrastructure:releaseVersion v1.2.3
```

Remove a pin to follow latest again:

```bash
pulumi -C infrastructure config rm infrastructure:releaseVersion
```

Setting `releaseVersion` to `latest` also enables discovery. Discovery failures
stop deployment before AWS resources change; image pull failures stop instance
provisioning. Mutable custom image tags are pulled on provisioning; changing the
remote tag alone does not change Pulumi inputs. Use a new tag or digest to trigger
an instance replacement.

The checkout's Compose configuration and certificate setup are uploaded to private
S3. Rust/UI source, binary installers, local `.env` files, certificates, and
extension payloads are excluded. The instance creates its own credentials.
Startup continues after Pulumi finishes; inspect cloud-init for readiness.

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
