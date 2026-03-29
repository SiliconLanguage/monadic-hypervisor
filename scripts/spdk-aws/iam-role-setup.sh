#!/bin/bash
echo "Creating IAM Role for passwordless AWS access..."
cat > trust-policy.json <<EOP
{
  "Version": "2012-10-17",
  "Statement": [{ "Effect": "Allow", "Principal": { "Service": "ec2.amazonaws.com" }, "Action": "sts:AssumeRole" }]
}
EOP
aws iam create-role --role-name EC2-MultiArch-Role --assume-role-policy-document file://trust-policy.json
aws iam attach-role-policy --role-name EC2-MultiArch-Role --policy-arn arn:aws:iam::aws:policy/AmazonEC2ContainerRegistryFullAccess
aws iam create-instance-profile --instance-profile-name EC2-MultiArch-Role-Profile
aws iam add-role-to-instance-profile --instance-profile-name EC2-MultiArch-Role-Profile --role-name EC2-MultiArch-Role
rm trust-policy.json
