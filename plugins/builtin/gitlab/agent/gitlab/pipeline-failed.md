# Find why a pipeline failed and retry it

Use this for "why is the pipeline of project X red" or "retry the failed pipeline". `$H` is the GitLab
address shown with the account in `guide.md`; `$P` is the project's numeric id or its full path with
slashes encoded as `%2F` (`group%2Fsubgroup%2Fproject`). Send no token, no `-k`. Reading needs the
read tools; the retry is a write and may need approval (see `tools.md`: `retry_pipeline`).

## 1. Find the pipeline

```sh
curl -s "https://$H/api/v4/projects/$P/pipelines?ref=main&status=failed&per_page=5"
```

Newest first. Without `ref`, pipelines of every branch and merge request are listed. Take the `id` of
the one the person means (ask when several fit). `GET /api/v4/projects/$P/pipelines/<id>` shows its
`status`, `ref`, `sha` and `web_url`.

## 2. Find the failed jobs

```sh
curl -s "https://$H/api/v4/projects/$P/pipelines/<id>/jobs?scope[]=failed"
```

Each job has `id`, `name`, `stage` and `allow_failure`. A failed job with `allow_failure: true` did not
fail the pipeline; look for the others first.

## 3. Read the log

```sh
curl -s "https://$H/api/v4/projects/$P/jobs/<job id>/trace" | tail -n 80
```

The log can be very large: save it to a file and read the end (the error is usually in the last lines).
Report the failing command, the error text and the likely cause. Do not retry before saying what failed:
a failure caused by the code (tests, lint, build) is not fixed by a retry; a runner or network failure
(`runner_system_failure`, timeouts, "Could not resolve host") may be.

## 4. Retry (only when the person asks, or agrees)

```sh
curl -s -X POST "https://$H/api/v4/projects/$P/pipelines/<id>/retry"
```

This retries the failed jobs of that pipeline; it needs a write grant and may wait for approval. The answer
is the pipeline with its new `status`. Check again with step 1 or `GET .../pipelines/<id>` after a moment
and tell the person the status and `web_url`. Do not retry in a loop.
