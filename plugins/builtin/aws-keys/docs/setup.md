# Connect AWS

This package signs requests to **S3 and EC2** with AWS Signature Version 4. Aurora signs each
request your agents send; agents never see a key or a token.

Connecting asks **"How do you want to connect?"** and offers two ways:

- **Assume a role (recommended).** Aurora gets short-lived credentials for an IAM role whose trust
  policy requires this organization's external id. The access key you enter is only the *source
  key*: it is used to assume the role from inside Aurora's hook and is never put on an agent's
  request.
- **Access keys.** The IAM key pair is used directly on every request. The simplest way to start,
  but the keys are long-lived.

(Before Aurora merged them, assuming a role was a separate "AWS (assume role)" app. Existing
connections were moved to **AWS** with their grants intact; see *Upgrading* below.)

Neither way gives agents access to STS beyond `GetCallerIdentity` (see *What agents can do*).

## Option 1: assume a role

### 1. Find your organization's external id

Aurora sends your organization's **external id** with every `AssumeRole` call (the `ExternalId`
parameter). It is a stable, opaque value per organization, shown in Aurora under *Organization
settings*, and it protects you from the confused-deputy problem: another Aurora organization cannot
get your role assumed, even if it learns the role's ARN. Aurora refuses to assume a role when the
external id is not available.

### 2. Create the source IAM user

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

### 3. Create the role and its trust policy

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

Attach a **permissions policy** to the role with only what agents need (see *Narrowest IAM policy*
below). The role's policies are the only thing that limits what agents can do: Aurora's approvals are
a second layer, not a replacement.

### 4. Connect it in Aurora

Choose *Connect*, *Assume a role* and enter:

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
reconnect. The source key must be a long-term IAM user key: temporary source credentials (role
chaining) are not supported. Aurora's `ExternalId` is the organization's, so one role can serve one
organization; give other organizations their own role or add their external id to a list in the
trust condition. Rotate the source key regularly; Aurora stores it encrypted and replaces it on
reconnect.

## Option 2: access keys

1. In the IAM console create a dedicated IAM user (or use temporary credentials from your own
   tooling) with **only** the permissions below. Do not reuse a user that can administer the account.
2. Create an access key for it (*Security credentials, Create access key*) and keep the secret
   access key until you have pasted it into Aurora.
3. In Aurora choose *Connect*, *Access keys*, paste the **Access key ID** and **Secret access
   key**, and enter the **Region** the agents should work in, for example `eu-west-1`. For
   temporary credentials also paste the **Session token**; they expire and must be reconnected
   when they do.

The keys are not validated when you connect; a wrong key shows up as an `InvalidClientTokenId`
or `SignatureDoesNotMatch` error on the first request. Rotate the access key regularly; Aurora
stores it encrypted and replaces it on reconnect.

## Narrowest IAM policy

The policy below covers exactly the operations this package classifies. Remove what you do not need
and replace the bucket name. Attach it to the IAM user (access keys) or to the role (assume a role).

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

Add `s3:PutObject` and `s3:DeleteObject` on the bucket's objects only if agents must change objects
(a `CopyObject` needs `s3:GetObject` on its source object as well, and `s3:PutObject` on its
destination). Reading a specific object version needs `s3:GetObjectVersion`, which this policy does
not grant.
The policy statements are the only thing that limits what the keys or the role can do: Aurora's
approvals are a second layer, not a replacement.

## What agents can do

- **S3** (path-style URLs, `https://s3.<region>.amazonaws.com/<bucket>/<key>`):
  - Allowed by default: list buckets (`ListBuckets`), list a bucket's objects (`ListObjectsV2`)
    and read or inspect an object (`GetObject`, `HeadObject`).
  - Held for approval: upload or copy an object (`PutObject`, `CopyObject`) and delete an object
    (`DeleteObject`). The approval shows the bucket and the first path segment of the key.
  - Not covered, so never allowed: creating or deleting buckets, multipart uploads, bulk delete
    (`DeleteObjects`), ACL and policy changes.
- **EC2** (`https://ec2.<region>.amazonaws.com/`): read-only `Describe*` actions such as
  `DescribeInstances` are allowed by default (tool "Describe EC2 resources").
- **STS** (`https://sts.<region>.amazonaws.com/`): only `GetCallerIdentity` is available to agents
  (tool "Show which AWS identity the connection belongs to"). Aurora's own hook calls STS to assume
  the role. Answers to `AssumeRole`, `GetSessionToken` and `GetFederationToken` carry usable AWS
  credentials, so every other STS action is blocked, whatever the grant.

EC2 and STS use the AWS Query API, where the action is a request parameter (`Action=...`). Aurora
reads it from the **form body of a POST** (`content-type: application/x-www-form-urlencoded`,
uncompressed, the parameter once and not also in the query string). When it can prove that the
action is a `Describe*` call (or `GetCallerIdentity`), the request is a read tool; in any doubt
(a GET, a repeated or missing `Action`, another body type, a compressed or very large body) it
is not, and every other EC2 call is a write tool **held for approval**, showing the `Action`.
Approve `Describe*`; refuse anything else. The IAM policy above is what actually prevents the
others. Other STS requests match no tool and are blocked outright.

## Upgrading from the separate AWS (assume role) package (instance admins)

Earlier versions shipped the role variant as its own package (`aws-role`). Version 2.0 of this
package replaces it. The new version adds hooks and STS egress, which is a **permission widening**:
after the upgrade, approve the new `aws-keys` version once under *Admin, Plugins*. When you do,
Aurora moves every connection of the old app to **AWS** as an *Assume a role* connection (it keeps
its id, its stored source key, its grants and its history), and disables the old package. Nothing
needs reconnecting. Until you approve, the old package keeps working. The two packages used to
claim the same hosts, so a fresh instance needed a host-claim decision; that conflict is gone.

## What changed in 1.1.0

- New tools `ec2_describe` (read) and `sts_get_caller_identity` (read), declared before the
  generic `ec2_call`. Requests that used to be the held `ec2_call` (a POST with a `Describe*`
  action) are now `ec2_describe`; **existing custom grants do not list the new tools, so they
  arrive blocked** until you allow them in the agent's Access tab. Full grants cover them.
- The `sts.*` host is now claimed, only to sign `GetCallerIdentity`. The package sets
  `grants.block_untooled`: a request on this connection that matches no tool of this package (for
  example `CreateBucket`, any other STS action) is blocked even under a full grant.

## Limits

- **One region per connection.** The region you enter is the one requests are signed for. A
  request to a host in a different region (for example `s3.us-east-1.amazonaws.com` on a
  `eu-west-1` connection) carries a signature for the wrong region and AWS rejects it. Create a
  second connection for another role or region.
- Only the regional endpoints `s3.<region>` and `ec2.<region>` of `amazonaws.com` are covered for
  agents (STS: `GetCallerIdentity` only), and Aurora's hook calls only `sts.<region>.amazonaws.com`.
  Virtual-hosted S3 URLs (`<bucket>.s3.<region>.amazonaws.com`), the global STS endpoint, the AWS
  China and isolated partitions, S3 Transfer Acceleration, dual-stack and FIPS endpoints are not.
