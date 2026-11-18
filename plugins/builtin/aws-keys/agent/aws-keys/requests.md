# AWS requests: endpoints, S3, EC2, STS

The connection is for **one region** (the connection's region, with an IAM access key pair or an IAM
role that Aurora assumes itself). Send requests straight to the regional endpoints; Aurora signs them
(Signature Version 4, with short-lived session credentials for a role) and never shows you a key or a
token. Do not use `aws configure`, `aws sts assume-role` or an SDK credential chain, and do not call
`AssumeRole` yourself. What you can do is limited by the policy of the keys or the role. A request to
a region other than the connection's is signed for the wrong region and AWS refuses it.

`$R` below is the connection's region (shown with the account in `guide.md`).

## S3

Use **path-style** URLs: `https://s3.$R.amazonaws.com/<bucket>/<key>`. Virtual-hosted bucket URLs
(`<bucket>.s3.amazonaws.com`) are not covered.

```sh
curl -s "https://s3.$R.amazonaws.com/<bucket>?list-type=2&prefix=logs/&max-keys=100"
curl -s "https://s3.$R.amazonaws.com/<bucket>/<key>" -o /tmp/object
```

`list-type=2` lists objects as XML; continue with `continuation-token=<NextContinuationToken>` while
`IsTruncated` is `true`. Save large objects to a file and look at the head.

## EC2 (Query API)

```sh
curl -s -X POST "https://ec2.$R.amazonaws.com/" \
  -H 'content-type: application/x-www-form-urlencoded' \
  -d 'Action=DescribeInstances&Version=2016-11-15'
```

Send the parameters as the POST **body**, once each, and not also in the query string. Read-only
`Describe*` actions are then allowed without approval; **every other EC2 call, including any GET, is held
for approval**. Answers are XML. Filters are `Filter.1.Name=instance-state-name&Filter.1.Value.1=running`.

## STS

Only `POST` with `Action=GetCallerIdentity` to `https://sts.$R.amazonaws.com/` works (it shows which
account and principal the connection is). Every other STS action is blocked.

```sh
curl -s -X POST "https://sts.$R.amazonaws.com/" \
  -H 'content-type: application/x-www-form-urlencoded' \
  -d 'Action=GetCallerIdentity&Version=2011-06-15'
```

## Errors

`403 AccessDenied` / `UnauthorizedOperation`: the key's or role's IAM policy does not allow it: report the
action and tell the person; do not try other credentials. `SignatureDoesNotMatch` or an endpoint in
another region: you used the wrong region. A refusal by Aurora has a JSON body with `error.code`.
