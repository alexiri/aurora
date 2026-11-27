# Example agent templates

A complete example of an Aurora **template repository**: source directories,
one per template, plus the reference CI that packages each template and
publishes it as a release artifact. No zip is ever committed here.

The templates are small but real; copy the repository and adapt them.

## The templates

- **[schedule-manager](schedule-manager/)** — reads an Indico category and
  your calendar, posts a weekday-morning agenda, and tidies the week ahead
  (with your approval before anything changes).
- **[service-monitor](service-monitor/)** — reads Grafana dashboards, triages
  alerts into a short summary, files Jira issues and runs a morning check.

## Using this repository

1. Replace `example/agent-templates` in `aurora-templates.yaml` with your
   repository (`packages.url`).
2. Put the repository on GitHub (in the workflow, set the `aurora` CLI version
   you pin and its sha256, `AURORA_CLI_SHA256`, from that release's
   `.sha256` file) or GitLab (see `.gitlab-ci.yml`).
3. Open a pull request: CI runs `aurora template check .` on it.
4. Merge: CI packs each template and publishes `<id>-<version>.zip` at the
   rendered `packages.url`. A published version is never replaced — change a
   template by raising `version` in its `template.yaml`.

An instance admin then adds the repository as a template source in Aurora and
reviews each version.

## Layout

```
aurora-templates.yaml        the repository manifest (strict)
schedule-manager/            one directory per template, named by its id
service-monitor/
.github/workflows/aurora-templates.yml   the reference CI
.github/aurora-publish.sh    the publish job's shell logic (POSIX sh + jq)
.gitlab-ci.yml               the GitLab sketch
```

Everything else (this README) is ignored by Aurora; a template is every
directory that directly contains a `template.yaml`.
