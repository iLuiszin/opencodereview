# Devin provider patch

- [x] providers/devin.rs (args, credentials validation, secrets, classify/failure, tests)
- [x] engines.rs: devin in PROVIDERS + model list (swe-high)
- [x] runner.rs: devin session, spawn/classify/final-text arms, PROVIDERS
- [x] egress.rs: allow server.codeium.com, api.devin.ai (+ fix deny test)
- [x] accounts: credentials path, store, has_credential, sandbox_sync mirror, usage arm
- [x] db/accounts.rs: provider allowlist + unsupported cleanup
- [x] review mod.rs/chunks.rs: provider match arms; db/usage_report.rs provider_of_model
- [x] server: devin_auth.rs paste route + routes.rs
- [x] docker/Dockerfile: pinned devin CLI install; Dockerfile.runner chmod
- [x] frontend: types, setupApi, AccessConnect, AccountsPanel, ProvidersPanel, EnginePicker, EngineAccounts, OnboardingAccess, usage format
- [x] WP-PATCHES.md bullet
- [x] tests, lint/fmt, commit, push, PR, report

## Review

Tests run:
- Backend (docker rust:1.98.1, targeted filters `providers:: sandbox:: accounts:: server::devin db::engines_tests db::accounts`): 23 passed, 3 failed — the 3 `db::engines_tests` fail only at connect to the throwaway Postgres (`Connection reset by peer`), including when run serially; environment, not code (identical tests untouched by the diff).
- Devin provider unit tests (args, credentials, classify, final message): all pass after the classify fix (stderr-only diagnostics).
- egress unit tests pass (one timing-dependent test flaked once under heavy load, passed on rerun).
- Frontend: pnpm typecheck, pnpm lint, 35 affected component tests pass; check:file-size passes.
- cargo fmt --all applied.
