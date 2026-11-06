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
valid. The instance builds images after Pulumi finishes; startup takes time.
No GitHub token or local Docker is required.

The current checkout's build inputs are uploaded to private S3. Local `.env`
files, certificates, and extension payloads are excluded. The instance generates
its own Kubernetes certificates and database password.

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
Source updates replace the instance with downtime. Local extension payloads go
in `/srv/suntitle/extensions`, mounted at `/opt/suntitle/extensions` in the proxy.

To permanently delete everything, including database and registry data:

```bash
pulumi -C infrastructure stack --show-urns
pulumi -C infrastructure state unprotect 'DATA_VOLUME_URN'
just infra-destroy
```

Replace `DATA_VOLUME_URN` with the `suntitle-data` EBS volume's URN.
