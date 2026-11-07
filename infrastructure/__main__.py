"""Deploy the existing Compose stack to EC2 with persistent Docker storage."""

import json
import os
from pathlib import Path

import pulumi
import pulumi_aws as aws

from deployment import build_source_archive, render_user_data, resolve_images

config = pulumi.Config()
images = resolve_images(
    api_image=config.get('apiImage'), proxy_image=config.get('proxyImage'),
    ui_image=config.get('uiImage'),
    image_repo=os.environ.get('IMAGE_REPO'),
    image_tag=os.environ.get('IMAGE_TAG', 'dev'),
)
region = aws.config.region
if not region:
    raise ValueError('Set aws:region before deploying.')

root = Path(__file__).resolve().parent.parent
archive_path = root / 'infrastructure/.build/source.tar.gz'
source_digest = build_source_archive(root, archive_path)
availability_zone = config.get('availabilityZone') or aws.get_availability_zones(state='available').names[0]
amazon_linux_ami = aws.ssm.get_parameter(
    name='/aws/service/ami-amazon-linux-latest/al2023-ami-kernel-default-x86_64',
    with_decryption=False,
)

source_bucket = aws.s3.BucketV2('suntitle-source')
source_privacy = aws.s3.BucketPublicAccessBlock(
    'suntitle-source-private', bucket=source_bucket.id,
    block_public_acls=True, block_public_policy=True,
    ignore_public_acls=True, restrict_public_buckets=True,
)
source_object = aws.s3.BucketObjectv2(
    'suntitle-source-archive', bucket=source_bucket.id,
    key=f'{source_digest}.tar.gz', source=pulumi.FileAsset(str(archive_path)),
)

role = aws.iam.Role('suntitle-instance', assume_role_policy=json.dumps({
    'Version': '2012-10-17',
    'Statement': [{'Effect': 'Allow', 'Principal': {'Service': 'ec2.amazonaws.com'},
                   'Action': 'sts:AssumeRole'}],
}))
source_access = aws.iam.RolePolicy(
    'suntitle-source-read', role=role.id,
    policy=source_object.arn.apply(lambda arn: json.dumps({
        'Version': '2012-10-17',
        'Statement': [{'Effect': 'Allow', 'Action': 's3:GetObject', 'Resource': arn}],
    })),
)
ssm_access = aws.iam.RolePolicyAttachment(
    'suntitle-ssm', role=role.name,
    policy_arn='arn:aws:iam::aws:policy/AmazonSSMManagedInstanceCore',
)
profile = aws.iam.InstanceProfile('suntitle-instance', role=role.name)

allow_tcp = aws.ec2.SecurityGroup(
    'allow_tcp',
    description='Public Compose gateway and SSH; backing services remain private.',
    ingress=[
        aws.ec2.SecurityGroupIngressArgs(
            protocol='tcp', from_port=80, to_port=80, cidr_blocks=['0.0.0.0/0']),
        aws.ec2.SecurityGroupIngressArgs(
            protocol='tcp', from_port=22, to_port=22,
            cidr_blocks=[config.get('sshCidr') or '0.0.0.0/0']),
    ],
    egress=[aws.ec2.SecurityGroupEgressArgs(
        protocol='-1', from_port=0, to_port=0, cidr_blocks=['0.0.0.0/0'])],
    tags={'Name': 'suntitle', 'Type': 'ws-proxy'},
)

address = aws.ec2.Eip('suntitle-address', domain='vpc')
registry_host = (pulumi.Output.from_input(config.get('registryHost'))
                 if config.get('registryHost')
                 else address.public_ip.apply(lambda ip: f'registry.{ip}.nip.io'))
data_volume = aws.ebs.Volume(
    'suntitle-data', availability_zone=availability_zone,
    size=config.get_int('dataVolumeSize') or 100, type='gp3', encrypted=True,
    tags={'Name': 'suntitle-data'},
)
user_data = pulumi.Output.all(
    source_bucket.id, source_object.key, data_volume.id, registry_host,
).apply(lambda values: render_user_data(
    bucket=values[0], key=values[1], region=region, volume_id=values[2],
    registry_host=values[3], compose_version='v5.6.0', images=images,
))

instance = aws.ec2.Instance(
    'ws-proxy',
    ami=amazon_linux_ami.value,
    instance_type=config.get('instanceType') or 't3.large',
    availability_zone=availability_zone,
    associate_public_ip_address=True,
    vpc_security_group_ids=[allow_tcp.id],
    iam_instance_profile=profile.name,
    key_name=config.get('keyName'),
    metadata_options=aws.ec2.InstanceMetadataOptionsArgs(http_tokens='required'),
    root_block_device=aws.ec2.InstanceRootBlockDeviceArgs(
        volume_size=30, volume_type='gp3', encrypted=True),
    user_data=user_data,
    user_data_replace_on_change=True,
    tags={'Name': 'suntitle', 'Type': 'ws-proxy'},
    opts=pulumi.ResourceOptions(
        depends_on=[source_access, source_privacy, ssm_access],
        delete_before_replace=True,
    ),
)
attachment = aws.ec2.VolumeAttachment(
    'suntitle-data', device_name='/dev/sdf', volume_id=data_volume.id,
    instance_id=instance.id, stop_instance_before_detaching=True,
)
association = aws.ec2.EipAssociation(
    'suntitle-address', allocation_id=address.id, instance_id=instance.id,
)

pulumi.export('publicIp', address.public_ip)
pulumi.export('url', address.public_ip.apply(lambda ip: f'http://{ip}'))
pulumi.export('registryHost', registry_host)
pulumi.export('registryUrl', registry_host.apply(lambda host: f'http://{host}'))
pulumi.export('instanceId', instance.id)
pulumi.export('dataVolumeId', data_volume.id)
pulumi.export('sourceDigest', source_digest)
pulumi.export('images', images)
