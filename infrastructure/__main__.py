import pulumi
import pulumi_aws as aws
import os
import re
import subprocess

def git_remote_url():
    """
    Return the remote URL of the current repository (origin).
    Raises RuntimeError if git is not available or the remote is missing.
    """
    try:
        # Run from the directory where `pulumi up` was invoked
        return subprocess.check_output(
            ["git", "remote", "get-url", "origin"],
            text=True
        ).strip()
    except Exception as e:
        raise RuntimeError("Could not determine Git remote URL: " + str(e))

def parse_owner_repo(url):
    """
    Accepts URLs such as:
        git@github.com:owner/repo.git
        https://github.com/owner/repo.git
    Returns (owner, repo)
    """
    m = re.search(r'[:/](?P<owner>[^/]+)/(?P<repo>[^/]+?)(?:\.git)?$', url)
    if not m:
        raise ValueError(f"Could not parse owner/repo from URL: {url}")
    return m.group('owner'), m.group('repo').rstrip(".git")

remote_url = git_remote_url()
GITHUB_OWNER, GITHUB_REPO = parse_owner_repo(remote_url)
GITHUB_TOKEN = os.environ.get("GITHUB_TOKEN")

if GITHUB_TOKEN is None:
    raise RuntimeError("GITHUB_TOKEN is not set")

cloud_init = f"""
#cloud-config
package_update: true
packages:
  - tar
  - jq
write_files:
  - path: /etc/systemd/system/ws-proxy.service
    owner: root:root
    permissions: '0644'
    content: |
      [Unit]
      Description=ws-proxy service
      After=network.target
      Wants=network-online.target

      [Service]
      ExecStart=/usr/local/bin/ws-proxy
      Restart=on-failure
      RestartSec=5s
      User=root

      [Install]
      WantedBy=multi-user.target
  - path: /tmp/setup-ws-proxy.sh
    permissions: '0755'
    owner: root:root
    content: |
      #!/bin/bash
      set -e

      echo "Downloading ws-proxy binary..."

      ASSET_ID=$(curl -s -H "Authorization: token {GITHUB_TOKEN}" \
        https://api.github.com/repos/{GITHUB_OWNER}/{GITHUB_REPO}/releases/latest | \
        jq -r '.assets[] | select(.name=="ws-proxy.x86_64-unknown-linux-gnu.tar.xz") | .id')

      curl -L -H "Accept: application/octet-stream" \
        -H "Authorization: token {GITHUB_TOKEN}" \
        https://api.github.com/repos/{GITHUB_OWNER}/{GITHUB_REPO}/releases/assets/$ASSET_ID \
        -o /tmp/ws-proxy.tar.xz

      echo "Extracting archive to /usr/local/bin"
      tar -xJf /tmp/ws-proxy.tar.xz -C /usr/local/bin
      chmod +x /usr/local/bin/ws-proxy

      echo "Reloading systemd daemon..."
      systemctl daemon-reload

      echo "Enabling and starting ws-proxy.service..."
      systemctl enable ws-proxy
      systemctl start ws-proxy
runcmd:
  - /tmp/setup-ws-proxy.sh
output: {{all: '| tee -a /var/log/cloud-init-output.log'}}
"""

allow_tcp = aws.ec2.SecurityGroup("allow_tcp",
    ingress=[
        aws.ec2.SecurityGroupIngressArgs(
            protocol="tcp",
            from_port=3030,
            to_port=3030,
            cidr_blocks=["0.0.0.0/0"],  # Allow SSH access
        ),
        aws.ec2.SecurityGroupIngressArgs(
            protocol="tcp",
            from_port=22,
            to_port=22,
            cidr_blocks=["0.0.0.0/0"],  # Allow SSH access
        )],
    egress=[aws.ec2.SecurityGroupEgressArgs(
            protocol="-1",
            from_port=0,
            to_port=0,
            cidr_blocks=["0.0.0.0/0"],  # Allow SSH access
        )],
    tags={
        "Name": "allow_tcp",
        "Type": "ws-proxy",
    },
)

instance = aws.ec2.Instance("ws-proxy",
    ami="resolve:ssm:/aws/service/ami-amazon-linux-latest/al2023-ami-kernel-default-x86_64",
    instance_type=aws.ec2.InstanceType.T3_MICRO,
    tags={
        "Type": "ws-proxy",
    },
    associate_public_ip_address=True,
    vpc_security_group_ids=[allow_tcp.id],
    user_data=cloud_init,
)

pulumi.export('publicIp', instance.public_ip)
