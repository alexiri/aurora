# Connect AWS through an IAM role

This package lets agents use **S3 and EC2** through an **IAM role** instead of long-lived
keys. Aurora assumes the role (`sts:AssumeRole`) and signs each request with the short-lived
session credentials it gets back (AWS Signature Version 4). Agents never see a key or a token.

The access key you enter is the **source key**: it is used only to assume the role, from inside
Aurora's hook, and is never put on an agent's request. If you would rather use the key itself,
use the *AWS (access keys)* package instead.

## 1. Find your organization's external id

Aurora sends your organization's **external id** with every `AssumeRole` call (the `ExternalId`
parameter). It is a stable, opaque value per organization, shown in Aurora under *Organization
settings*, and it protects you from the confused-deputy problem: another Aurora organization cannot
get your role assumed, even if it learns the role's ARN. Aurora refuses to assume a role when the
external id is not available.

## 2. Create the source IAM user

1. Create a dedicated IAM user with **no permissions** except `sts:AssumeRole` on the one role below.
2. Create an access key for it (*Security credentials, Create access key*).

```json
{
  "Version": "2012-10-17",
  "Statement": [
    { "Effect": "Allow", "Action": "sts:AssumeRole",
      "Resource": "arn:aws:iam::123456789012:role/aurora-agent" }
  ]
}
```

## 3. Create the role and its trust policy

Create the role (for example `aurora-agent`) in the account the agents should work in. Its **trust
policy** must allow the source user **and require your external id**; replace the account id, the
user name and `<external id>`:

```json
{
  "Version": "2012-10-17",
  "Statement": [
    {
      "Effect": "Allow",
      "Principal": { "AWS": "arn:aws:iam::123456789012:user/aurora-source" },
      "Action": "sts:AssumeRole",
      "Condition": { "StringEquals": { "sts:ExternalId": "<external id>" } }
    }
  ]
}
```

Attach a **permissions policy** to the role with only what agents need. The policy below covers
exactly the operations this package classifies; remove what you do not need and replace the bucket
name.

```json
{
  "Version": "2012-10-17",
  "Statement": [
    { "Sid": "ListBuckets", "Effect": "Allow", "Action": "s3:ListAllMyBuckets", "Resource": "*" },
    { "Sid": "ReadBucket", "Effect": "Allow",
      "Action": ["s3:ListBucket"], "Resource": "arn:aws:s3:::example-bucket" },
    { "Sid": "ReadObjects", "Effect": "Allow",
      "Action": ["s3:GetObject"], "Resource": "arn:aws:s3:::example-bucket/*" },
    { "Sid": "Ec2ReadOnly", "Effect": "Allow",
      "Action": ["ec2:DescribeInstances", "ec2:DescribeRegions", "ec2:DescribeSecurityGroups",
                 "ec2:DescribeSubnets", "ec2:DescribeVolumes", "ec2:DescribeVpcs"],
      "Resource": "*" }
  ]
}
```

Add `s3:PutObject` and `s3:DeleteObject` on the bucket's objects only if agents must change objects.
The role's policies are the only thing that limits what agents can do: Aurora's approvals are a second layer, not a replacement.

## 4. Connect it in Aurora

Choose *Connect* and enter:

- **Role ARN**: `arn:aws:iam::<12-digit account id>:role/<name>` (a path such as `role/team/name` is
  fine). Roles in the `aws` and `aws-us-gov` partitions are supported.
- **Region**: for example `eu-west-1`. Aurora calls that region's STS endpoint
  (`sts.<region>.amazonaws.com`) and signs requests for that region.
- **Source access key ID** and **Source secret access key**: the key of the source user.
- **Session duration cap** (optional): seconds between 900 and 43200, default 3600. The role's own
  *maximum session duration* (one hour unless you raised it) must be at least this long, otherwise
  AWS refuses the call; lower the cap or raise the role's maximum.

Aurora assumes the role once when you connect, so a wrong key, role ARN, region or trust policy is
reported immediately (the error names the AWS error code and never shows a key). Afterwards it
assumes the role again whenever the session credentials expire. If AWS starts refusing the call
(the trust policy changed, the source key was deleted or rotated), the connection asks you to
reconnect.

## What agents can do

- **S3** (path-style URLs, `https://s3.<region>.amazonaws.com/<bucket>/<key>`):
  - Allowed by default: list buckets, list a bucket's objects and read or inspect an object.
  - Held for approval: upload or copy an object and delete an object. The approval shows the bucket
    and the first path segment of the key.
  - Not covered, so never allowed: creating or deleting buckets, multipart uploads, bulk delete,
    ACL and policy changes.
- **EC2** (`https://ec2.<region>.amazonaws.com/`): read-only `Describe*` actions.
- **STS is not available to agents.** Only Aurora's own hook calls STS (to assume the role).
  Answers to `AssumeRole`, `GetSessionToken` and `GetFederationToken` carry usable AWS credentials,
  and Aurora cannot yet limit a tool to a harmless action such as `GetCallerIdentity`, so requests
  to `sts.<region>.amazonaws.com` are not signed (follow-up FU-P2-CAT-5a).

EC2 uses the AWS Query API, where the action is a request parameter and a GET can run any action,
so **every EC2 call is held for approval** and the approval shows the `Action`. Approve `Describe*`;
refuse anything else.

## Limits

- **One role and one region per connection.** A request to a host in another region is signed for
  the connection's region and AWS rejects it. Create another connection for another role or region.
- Only the regional endpoints `s3.<region>` and `ec2.<region>` of `amazonaws.com` are covered for
  agents, and Aurora's hook calls only `sts.<region>.amazonaws.com`. Virtual-hosted S3 URLs, the
  global `sts.amazonaws.com` endpoint, the AWS China and isolated partitions, FIPS and dual-stack
  endpoints are not covered.
- The source key must be a long-term IAM user key. Temporary source credentials (role chaining) are
  not supported.
- Aurora's `ExternalId` is the organization's, so one role can serve one organization; give other
  organizations their own role or add their external id to a list in the trust condition.
- **This package and *AWS (access keys)* (`aws-keys`) claim the same S3 and EC2 hosts** (`s3.*`,
  `ec2.*` of `amazonaws.com`), so an instance enables one of them for those hosts: the
  administrator's host-claim decision picks which.
- Rotate the source key regularly; Aurora stores it encrypted and replaces it on reconnect.
