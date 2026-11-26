-- Devin accounts: reviews may now run on Devin SWE.
ALTER TABLE ai_accounts DROP CONSTRAINT ai_accounts_provider_check;
ALTER TABLE ai_accounts ADD CONSTRAINT ai_accounts_provider_check
  CHECK (provider IN ('claude', 'codex', 'gemini', 'opencode', 'cursor', 'devin'));
