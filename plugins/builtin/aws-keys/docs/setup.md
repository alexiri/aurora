# Connect AWS with an access key pair

This package signs requests to **S3 and EC2** with an IAM access key pair, using AWS
Signature Version 4. Aurora signs each request your agents send; agents never see the keys.
It is the access-key variant. To use short-lived credentials from an assumed IAM role instead,
use the **AWS (assume role)** package (`aws-role`).

**Both packages claim the same S3, EC2 and STS hosts** (`s3.*`, `ec2.*`, `sts.*` of `amazonaws.com`), so an
instance enables one of them for those hosts: the administrator's host-claim decision picks which.
Neither package gives agents access to STS beyond `GetCallerIdentity` (see Limits).

## Create the keys

1. In the IAM console create a dedicated IAM user (or use temporary credentials from your own
   tooling) with **only** the permissions below. Do not reuse a user that can administer the account.
2. Create an access key for it (*Security credentials, Create access key*) and keep the secret
   access key until you have pasted it into Aurora.
3. In Aurora choose *Connect*, paste the **Access key ID** and **Secret access key**, and enter the
   **Region** the agents should work in, for example `eu-west-1`. For temporary credentials also
   paste the **Session token**; they expire and must be reconnected when they do.

## Narrowest IAM policy

The policy below covers exactly the operations this package classifies. Remove what you do not need
and replace the bucket name.

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
The policy statements are the only thing that limits what the keys can do: Aurora's approvals are
a second layer, not a replacement.

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
  (tool "Show which AWS identity the keys belong to"). Answers to `AssumeRole`,
  `GetSessionToken` and `GetFederationToken` carry usable AWS credentials, so every other STS
  action is blocked, whatever the grant.

EC2 and STS use the AWS Query API, where the action is a request parameter (`Action=...`). Aurora
reads it from the **form body of a POST** (`content-type: application/x-www-form-urlencoded`,
uncompressed, the parameter once and not also in the query string). When it can prove that the
action is a `Describe*` call (or `GetCallerIdentity`), the request is a read tool; in any doubt
(a GET, a repeated or missing `Action`, another body type, a compressed or very large body) it
is not, and every other EC2 call is a write tool **held for approval**, showing the `Action`.
Approve `Describe*`; refuse anything else. The IAM policy above is what actually prevents the
others. Other STS requests match no tool and are blocked outright.

## What changed in 1.1.0

- New tools `ec2_describe` (read) and `sts_get_caller_identity` (read), declared before the
  generic `ec2_call`. Requests that used to be the held `ec2_call` (a POST with a `Describe*`
  action) are now `ec2_describe`; **existing custom grants do not list the new tools, so they
  arrive blocked** until you allow them in the agent's Access tab. Full grants cover them.
- The `sts.*` host is now claimed, only to sign `GetCallerIdentity`. The package sets
  `grants.block_untooled`: a request on these keys that matches no tool of this package (for
  example `CreateBucket`, any other STS action) is blocked even under a full grant.

## Limits

- **One region per connection.** The region you enter is the one requests are signed for. A
  request to a host in a different region (for example `s3.us-east-1.amazonaws.com` on a
  `eu-west-1` connection) carries a signature for the wrong region and AWS rejects it. Create a
  second connection for another region.
- Only the regional endpoints `s3.<region>` and `ec2.<region>` of `amazonaws.com` are covered.
  Virtual-hosted S3 URLs (`<bucket>.s3.<region>.amazonaws.com`), the global STS endpoint, S3
  Transfer Acceleration, dual-stack and FIPS endpoints are not.
- The keys are not validated when you connect; a wrong key shows up as an `InvalidClientTokenId`
  or `SignatureDoesNotMatch` error on the first request.
- Rotate the access key regularly; Aurora stores it encrypted and replaces it on reconnect.
