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
valid. Pulumi defaults to these images:

- `ghcr.io/suntitle/suntitle/control-api:latest`
- `ghcr.io/suntitle/suntitle/ws-proxy:latest`
- `ghcr.io/suntitle/suntitle/ui:latest`

No GitHub release lookup is required. The API image also runs Kubernetes
bootstrap. EC2 pulls images and runs Compose without compiling or building images.
The selected images MUST be readable by the instance. Registry authentication is
not configured here; public GHCR packages permit anonymous pulls.

For custom image sources, set any of:

```bash
pulumi -C infrastructure config set infrastructure:apiImage registry.example.com/suntitle/api:v2
pulumi -C infrastructure config set infrastructure:proxyImage registry.example.com/suntitle/proxy:v2
pulumi -C infrastructure config set infrastructure:uiImage registry.example.com/suntitle/ui:v2
```

References may use tags or digests (`registry.example.com/image@sha256:...`).
An override replaces just that component; the remaining components use their
GHCR `latest` defaults. The `images` stack output shows the exact selected
references. Custom images MUST support the existing Compose commands, ports,
health checks, and mounted configuration; API images MUST include the
`control_api` bootstrap commands.

To pin a version or roll back, set the image overrides to explicit tags or digests.
Remove an override with `pulumi -C infrastructure config rm infrastructure:apiImage`
(or `proxyImage` / `uiImage`) to restore its default. The former `releaseVersion`
and `releaseRepository` settings are no longer used.

Image pull failures stop instance provisioning. Mutable tags, including `latest`,
are pulled on provisioning; changing the remote tag alone does not change Pulumi
inputs or trigger an update when running `just infra-up`. Use a new image tag or
digest to trigger an instance replacement.

The checkout's Compose configuration and certificate setup are uploaded to private
S3. Rust/UI source, binary installers, local `.env` files, certificates, and
extension payloads are excluded. The instance creates its own credentials.
Startup continues after Pulumi finishes; inspect cloud-init for readiness.

## Connect and inspect

On macOS, install the SSM client once:

```bash
brew install --cask session-manager-plugin
```

Connect to the selected Pulumi stack's EC2 instance using your AWS credentials:

```bash
just infra-ssh
```

On the instance, check provisioning status and follow the bootstrap command output:

```bash
sudo cloud-init status --long
sudo tail -n 200 -f /var/log/cloud-init-output.log
```

Press `Ctrl-C` to stop following. For cloud-init's internal diagnostics, read
`sudo less /var/log/cloud-init.log`. These logs belong to the instance;
`pulumi up` can finish before provisioning completes.

Once provisioning succeeds, inspect the application containers and logs:

```bash
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

The Elastic IP and EBS disk survive instance replacement. Docker data,
Kubernetes credentials, and extension payloads live under `/srv/suntitle`.
Image reference or deployment input updates replace the instance with downtime.
Local extension payloads go in `/srv/suntitle/extensions`, mounted at `/opt/suntitle/extensions` in the proxy.

To delete the deployment, including database, registry, and extension data:

```bash
just infra-down
```

`infra-down` aliases `infra-destroy`; both destroy the full stack. Existing stacks
created with volume protection need a one-time state update before destruction:

```bash
pulumi -C infrastructure stack --show-urns
pulumi -C infrastructure state unprotect 'DATA_VOLUME_URN'
just infra-destroy
```

Replace `DATA_VOLUME_URN` with the `suntitle-data` EBS volume's URN.
