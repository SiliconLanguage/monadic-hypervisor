#!/bin/bash
set -e
AMI_ID=$(aws ssm get-parameters --names /aws/service/ami-amazon-linux-latest/al2023-ami-kernel-default-arm64 --query 'Parameters[0].Value' --output text)
VPC_ID=$(aws ec2 describe-vpcs --filters "Name=is-default,Values=true" --query "Vpcs[0].VpcId" --output text)
SUBNET_ID=$(aws ec2 describe-subnets --filters "Name=vpc-id,Values=$VPC_ID" --query "Subnets[0].SubnetId" --output text)

echo "Launching Graviton Instance with Local NVMe..."
INSTANCE_ID=$(aws ec2 run-instances \
    --image-id "$AMI_ID" \
    --instance-type c7gd.xlarge \
    --key-name spdk-dev-key \
    --subnet-id "$SUBNET_ID" \
    --iam-instance-profile Name=EC2-MultiArch-Role-Profile \
    --user-data "file://$(dirname "$0")/cloud-init-userdata.yaml" \
    --query 'Instances[0].InstanceId' --output text)

aws ec2 wait instance-running --instance-ids "$INSTANCE_ID"
PUBLIC_IP=$(aws ec2 describe-instances --instance-ids "$INSTANCE_ID" --query 'Reservations[0].Instances[0].PublicIpAddress' --output text)
echo "Provisioning Complete. IP: $PUBLIC_IP"
