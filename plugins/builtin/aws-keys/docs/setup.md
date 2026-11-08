# Connect AWS with an access key pair

This package signs requests to **S3, STS and EC2** with an IAM access key pair, using AWS
Signature Version 4. Aurora signs each request your agents send; agents never see the keys.
It is the access-key variant; assuming a role to mint short-lived credentials is a separate
package that is not available yet.

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
    { "Sid": "Identity", "Effect": "Allow", "Action": "sts:GetCallerIdentity", "Resource": "*" },
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
`sts:GetCallerIdentity` needs no permission at all, and the other policy statements are the only
thing that limits what the keys can do: Aurora's approvals are a second layer, not a replacement.

## What agents can do

- **S3** (path-style URLs, `https://s3.<region>.amazonaws.com/<bucket>/<key>`):
  - Allowed by default: list buckets (`ListBuckets`), list a bucket's objects (`ListObjectsV2`)
    and read or inspect an object (`GetObject`, `HeadObject`).
  - Held for approval: upload or copy an object (`PutObject`, `CopyObject`) and delete an object
    (`DeleteObject`). The approval shows the bucket and the first path segment of the key.
  - Not covered, so never allowed: creating or deleting buckets, multipart uploads, bulk delete
    (`DeleteObjects`), ACL and policy changes.
- **STS** (`https://sts.<region>.amazonaws.com/`): `GetCallerIdentity` to see which identity the keys
  belong to.
- **EC2** (`https://ec2.<region>.amazonaws.com/`): read-only `Describe*` actions such as
  `DescribeInstances`.

STS and EC2 use the AWS Query API, where the action is a request parameter (`Action=...`) and
a GET can run any action. Aurora cannot tell a `Describe*` call from a destructive one by method or
path, so **every STS and EC2 call is held for approval** and the approval shows the `Action`.
Approve `Describe*` and `GetCallerIdentity`; refuse anything else. The IAM policy above is what
actually prevents the others.

## Limits

- **One region per connection.** The region you enter is the one requests are signed for. A
  request to a host in a different region (for example `s3.us-east-1.amazonaws.com` on a
  `eu-west-1` connection) carries a signature for the wrong region and AWS rejects it. Create a
  second connection for another region.
- Only the regional endpoints `s3.<region>`, `sts.<region>` and `ec2.<region>` of `amazonaws.com`
  are covered. Virtual-hosted S3 URLs (`<bucket>.s3.<region>.amazonaws.com`), the global
  `sts.amazonaws.com` endpoint, S3 Transfer Acceleration, dual-stack and FIPS endpoints are not.
- The keys are not validated when you connect; a wrong key shows up as an `InvalidClientTokenId`
  or `SignatureDoesNotMatch` error on the first request.
- Rotate the access key regularly; Aurora stores it encrypted and replaces it on reconnect.
