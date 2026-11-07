# Phase 0 fixture packages (docs/09 section 13)

Run `bash scripts/fixtures/build.sh`. Output goes to `dist/` (gitignored).

`test-key/` holds an ed25519 pair used ONLY to sign these fixtures. The private key is committed
on purpose; it must never be trusted outside test configurations. All hosts are neutral
`*.example.test` names. A `mutate` file in an invalid fixture selects an archive-level defect
(see `scripts/fixtures/mkfixture.py`). `expected-invalid.txt` maps invalid fixtures to codes.
