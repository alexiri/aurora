# Google Drive API v3 notes

Base URL `https://www.googleapis.com/drive/v3` (also in `$GOOGLE_DRIVE_API_URL`); uploads go to
`https://www.googleapis.com/upload/drive/v3`. Aurora adds the credentials: send no token, no `-k`.

## Find files

```sh
curl -s "https://www.googleapis.com/drive/v3/files?q=name+contains+'report'+and+trashed=false&fields=files(id,name,mimeType,parents),nextPageToken&pageSize=50"
```

`q` is Drive's query language (URL-encode it); always ask for `fields` so the answer stays small. Page
with `pageToken=<nextPageToken>` until it is absent. A folder has the mime type
`application/vnd.google-apps.folder`; list its files with `q='<folderId>'+in+parents`.

## Download

- Binary files: `GET /drive/v3/files/<id>?alt=media` (save to a file, do not print it).
- Google Docs, Sheets and Slides cannot be downloaded that way: use
  `GET /drive/v3/files/<id>/export?mimeType=application/pdf` (or `text/plain`, `text/csv`, ...).

## Upload and change

Send JSON metadata with `content-type: application/json`; multipart uploads (`uploadType=multipart`)
carry the JSON metadata as the **first** part, so approvals can show the file name, type and folder.
Sharing (permissions) and every change are held for approval.

## Deleting

Permanent deletion is not offered as a tool. Trash a file with `PATCH /drive/v3/files/<id>` and body
`{"trashed": true}`. A connection with a full grant can still send `DELETE`, which cannot be undone:
do not do it unless the person asked for exactly that.
