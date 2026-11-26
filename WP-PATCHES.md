# Patches on top of OpenCodeReview

Branches `wp/v<release>` are an upstream release tag plus these commits, replayed on every new
release by a nightly job that runs the backend tests before it deploys anything.

- **`review_parallel_chunks` really sets how many chunks one review runs at once.** Upstream caps it
  at 6 in `MAX_PROCESSES_PER_REVIEW`; here the cap is 20, the setting's own range. Unset still
  means 6.
- **`review_quiet_seconds` waits for a quiet head.** An automatic review starts only once the pull
  request's head has stayed the same for that many seconds (0 to 3600; 0, the default, is off), so
  a burst of pushes is reviewed once, on its last head.
- **A `devin` provider runs reviews on Devin SWE.** Same account model as the other providers
  (credential `devin/credentials.toml`, pasted from a `devin auth login` in Settings → Providers;
  model `swe-high`, no separate effort). The runner copies the credential into a private home and
  runs `devin -p --process-model in-process --model swe-high --permission-mode dangerous
  --respect-workspace-trust false --prompt-file /work/prompt.md`, parsing the final answer with the
  lenient report contract; usage-limit and auth failures classify for failover like the other
  providers. The pinned, checksum-verified CLI (`static.devin.ai`) ships in the server and runner
  images, and the egress proxy admits `server.codeium.com` and `api.devin.ai`.
- `docker-compose.fork.yml` builds the server from this branch instead of pulling the release
  image; `scripts/wp-test.sh` runs the backend tests against a throwaway Postgres.
