#!/bin/sh
# Publish every template in a repository folder. POSIX sh + jq + gh + the
# aurora CLI.
#
#   .github/aurora-publish.sh [repository folder]     (default: .)
#
# For each template from `aurora template list <folder> --json`:
#   1. pack it to dist/<id>-<version>.zip;
#   2. if the release <id>-v<version> exists, download that release's
#      <id>-<version>.zip and compare it with cmp. When they differ the job
#      fails: a published version is never replaced;
#   3. otherwise create the release, with CHANGELOG.md as the notes when the
#      template has one.
# It never uses --clobber and never deletes a release.
set -eu

folder=${1:-.}
dist=dist
mkdir -p "$dist"

# The list goes to a file first: piped straight into the loop, a failing `list`
# would publish nothing and still end in success (sh has no pipefail).
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
aurora template list "$folder" --json >"$work/list.json"
jq -c '.[]' "$work/list.json" >"$work/items"

while IFS= read -r item; do
    id=$(printf '%s' "$item" | jq -r '.id')
    version=$(printf '%s' "$item" | jq -r '.version')
    dir=$(printf '%s' "$item" | jq -r '.dir')
    name=$(printf '%s' "$item" | jq -r '.name')

    zip="$dist/$id-$version.zip"
    aurora template pack "$folder/$dir" -o "$zip"

    tag="$id-v$version"
    asset="$id-$version.zip"
    if gh release view "$tag" >/dev/null 2>&1; then
        gh release download "$tag" -p "$asset" -O "$dist/downloaded-$asset"
        if ! cmp -s "$zip" "$dist/downloaded-$asset"; then
            printf '%s changed without a new version: raise version in %s/template.yaml\n' \
                "$id" "$dir" >&2
            rm -f "$dist/downloaded-$asset"
            exit 1
        fi
        rm -f "$dist/downloaded-$asset"
        printf '%s %s: release %s already exists and matches; nothing to publish.\n' \
            "$id" "$version" "$tag"
    else
        set -- --target "$GITHUB_SHA" --title "$name $version"
        if [ -f "$folder/$dir/CHANGELOG.md" ]; then
            set -- "$@" --notes-file "$folder/$dir/CHANGELOG.md"
        fi
        gh release create "$tag" "$zip" "$@"
        printf '%s %s: published %s at %s.\n' "$id" "$version" "$asset" "$tag"
    fi
done <"$work/items"
