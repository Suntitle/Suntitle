#!/bin/bash
# Mount data before Docker starts. Credentials and named volumes MUST survive
# instance replacement. A failed first boot MUST exit before reporting readiness.
set -euo pipefail
umask 077
# @DEPLOYMENT_ENV@

dnf install -y docker awscli-2 openssl
systemctl stop docker.service docker.socket || true

volume_serial="${DATA_VOLUME_ID//-/}"
volume_device="/dev/disk/by-id/nvme-Amazon_Elastic_Block_Store_${volume_serial}"
for attempt in {1..180}; do
    if [[ -b "$volume_device" ]]; then break; fi
    sleep 2
done
if [[ ! -b "$volume_device" ]]; then
    echo "Persistent volume $DATA_VOLUME_ID did not attach." >&2
    exit 1
fi
if ! blkid "$volume_device" >/dev/null; then
    mkfs.ext4 "$volume_device"
fi
mkdir -p /srv/suntitle
volume_uuid="$(blkid -s UUID -o value "$volume_device")"
if ! findmnt /srv/suntitle >/dev/null; then
    printf 'UUID=%s /srv/suntitle ext4 defaults 0 2\n' "$volume_uuid" >> /etc/fstab
    mount /srv/suntitle
fi
mkdir -p /etc/docker
printf '{"data-root":"/srv/suntitle/docker"}\n' > /etc/docker/daemon.json

plugin_dir=/usr/local/lib/docker/cli-plugins
mkdir -p "$plugin_dir"
curl -fsSL --retry 5 "https://github.com/docker/compose/releases/download/${COMPOSE_VERSION}/docker-compose-linux-x86_64" -o "$plugin_dir/docker-compose"
curl -fsSL --retry 5 "https://github.com/docker/compose/releases/download/${COMPOSE_VERSION}/docker-compose-linux-x86_64.sha256" -o /tmp/compose.sha256
printf '%s  %s\n' "$(awk '{print $1}' /tmp/compose.sha256)" "$plugin_dir/docker-compose" | sha256sum -c -
chmod 755 "$plugin_dir/docker-compose"
usermod -aG docker ec2-user

mkdir -p /opt/suntitle
aws s3 cp "s3://${SOURCE_BUCKET}/${SOURCE_KEY}" /tmp/suntitle.tar.gz
# Content-addressed keys MUST match the bytes received.
printf '%s  /tmp/suntitle.tar.gz\n' "${SOURCE_KEY%.tar.gz}" | sha256sum -c -
tar -xzf /tmp/suntitle.tar.gz -C /opt/suntitle
cd /opt/suntitle
mkdir -p /srv/suntitle/pki /srv/suntitle/private-ca /srv/suntitle/extensions
ln -sfn /srv/suntitle/pki pki
ln -sfn /srv/suntitle/private-ca private-ca
ln -sfn /srv/suntitle/extensions extensions
if [[ ! -s /srv/suntitle/postgres-password ]]; then
    openssl rand -hex 32 > /srv/suntitle/postgres-password
fi
cat > .env.local <<CONFIG
POSTGRES_DB=suntitle
POSTGRES_USER=suntitle
POSTGRES_PASSWORD=$(cat /srv/suntitle/postgres-password)
ADMIN_USERNAME=admin
ADMIN_PASSWORD=${ADMIN_PASSWORD}
REGISTRY_HOST=${REGISTRY_HOST}
COMPOSE_FILE=compose.yaml:compose.release.yaml
API_IMAGE=${API_IMAGE}
PROXY_IMAGE=${PROXY_IMAGE}
UI_IMAGE=${UI_IMAGE}
LOCAL_EXTENSIONS_DIR=/srv/suntitle/extensions
CONFIG

bash infrastructure/generate-pki.sh
# Keep the existing optional fixture mount empty on the deployment host.
mkdir -p ws-proxy/examples/custom_node_e2e/extensions
systemctl enable --now docker
# Pull failures MUST stop provisioning before the stack is changed.
docker compose --env-file .env.local pull
docker compose --env-file .env.local up --no-build -d --wait --wait-timeout 300
docker compose --env-file .env.local ps
