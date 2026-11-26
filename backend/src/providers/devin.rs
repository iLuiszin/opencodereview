//! Devin CLI execution for review stages that run on a Devin account.
use std::ffi::OsString;
use std::path::Path;

use crate::providers::claude::Failure;

/// The default model; Settings can pick others from `engines`.
pub(crate) const MODEL: &str = "swe-high";

/// `credentials.toml` written by `devin auth login`; anything larger is not a Devin credential.
pub(crate) const MAX_CREDENTIALS_BYTES: usize = 64 * 1024;

/// Arguments for one non-interactive execution that reads its prompt from `prompt_file` and
/// prints its final answer on stdout. The agent runs in this process: the runner container has no
/// background daemon, and a surviving one would stop every later stage.
pub(crate) fn print_args(prompt_file: &Path, model: &str) -> Vec<OsString> {
    [
        "-p",
        "--process-model",
        "in-process",
        "--model",
        model,
        "--permission-mode",
        "dangerous",
        "--respect-workspace-trust",
        "false",
        "--prompt-file",
    ]
    .iter()
    .map(Into::into)
    .chain([prompt_file.as_os_str().to_owned()])
    .collect()
}

/// The value of a top-level `key = "value"` line, as `credentials.toml` keeps each field.
fn field(raw: &str, key: &str) -> Option<String> {
    raw.lines().find_map(|line| {
        let (name, value) = line.trim().split_once('=')?;
        let value = value.trim();
        (name.trim() == key
            && value.starts_with('"')
            && value.ends_with('"')
            && value.len() >= 2
            && !value.contains('\\'))
        .then(|| value[1..value.len() - 1].to_string())
    })
}

/// Checks that `raw` is the `credentials.toml` of a signed-in Devin account.
pub(crate) fn validate_credentials(raw: &str) -> Result<(), String> {
    if raw.len() > MAX_CREDENTIALS_BYTES {
        return Err("Devin authentication is too large".into());
    }
    if field(raw, "windsurf_api_key").is_none_or(|key| key.len() < 16) {
        return Err("Devin authentication has no API key; sign in again".into());
    }
    Ok(())
}

/// Credential values in a `credentials.toml` that must never appear in review output.
pub(crate) fn secrets(raw: &str) -> Vec<String> {
    raw.lines()
        .filter_map(|line| {
            line.trim()
                .split_once('=')
                .and_then(|(key, _)| field(raw, key.trim()))
                .filter(|secret| secret.len() >= 16)
        })
        .collect()
}

/// Why an execution failed, read only from the CLI's own diagnostics on stderr, never from the
/// final answer, so reviewed code that mentions "429" or a login can't trip it.
pub(crate) fn classify(stdout: &str, stderr: &str) -> Failure {
    let _ = stdout;
    let lower = stderr.to_ascii_lowercase();
    if ["usage limit", "hit your limit", "out of credits", "credit balance"]
        .iter()
        .any(|word| lower.contains(word))
    {
        return Failure::UsageLimit { resets_at: None };
    }
    if [
        "not logged in",
        "please run devin auth login",
        "invalid api key",
        "invalid token",
        "unauthorized",
        "authentication failed",
    ]
    .iter()
    .any(|word| lower.contains(word))
    {
        return Failure::Auth;
    }
    if ["429", "rate limit", "overloaded", "503"]
        .iter()
        .any(|word| lower.contains(word))
    {
        return Failure::Overloaded;
    }
    if lower.contains("model") && (lower.contains("not found") || lower.contains("not available")) {
        return Failure::Model;
    }
    Failure::Other
}

/// Short, safe diagnostic for a failed process; raw CLI output stays private.
pub(crate) fn failure(stdout: &str, stderr: &str) -> String {
    match classify(stdout, stderr) {
        Failure::Other if stdout.trim().is_empty() && stderr.trim().is_empty() => {
            "Devin process exited without diagnostics; verify runner health and authentication".into()
        }
        Failure::Other => "Devin execution failed; verify authentication".into(),
        Failure::Overloaded => "Devin is temporarily overloaded; retry later".into(),
        Failure::Model => "Devin model is unavailable for this account".into(),
        failure => failure.encode(),
    }
}

/// The answer `devin -p` prints; its final message is the whole stdout.
pub(crate) fn final_message(stdout: &str) -> Result<String, String> {
    let text = stdout.trim();
    if text.is_empty() {
        Err("Devin final response is missing".into())
    } else {
        Ok(text.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn print_args_run_one_turn_on_the_chosen_model() {
        let args = print_args(Path::new("/work/prompt.md"), MODEL);
        let args = args.iter().map(|arg| arg.to_str().unwrap()).collect::<Vec<_>>();
        assert!(args.contains(&"-p") && args.contains(&"--process-model"));
        assert!(args.windows(2).any(|pair| pair == ["--model", "swe-high"]));
        assert!(args.windows(2).any(|pair| pair == ["--permission-mode", "dangerous"]));
        assert!(args
            .windows(2)
            .any(|pair| pair == ["--respect-workspace-trust", "false"]));
        assert_eq!(args.last(), Some(&"/work/prompt.md"));
    }

    #[test]
    fn credentials_need_a_devin_api_key_and_expose_it_for_redaction() {
        let raw = "windsurf_api_key = \"fixture-api-key-value-0123456789\"\napi_server_url = \"https://server.codeium.com\"\n";
        assert!(validate_credentials(raw).is_ok());
        assert_eq!(
            secrets(raw),
            vec![
                "fixture-api-key-value-0123456789".to_string(),
                "https://server.codeium.com".to_string()
            ]
        );
        assert!(validate_credentials("api_server_url = \"https://server.codeium.com\"\n").is_err());
        assert!(validate_credentials("windsurf_api_key = \"short\"\n").is_err());
        assert!(validate_credentials("windsurf_api_key = unquoted\n").is_err());
        assert!(validate_credentials("").is_err());
        assert_eq!(
            field(
                "api_server_url = \"https://server.codeium.com\"\nwindsurf_api_key = \"k\"\n",
                "api_server_url"
            )
            .as_deref(),
            Some("https://server.codeium.com")
        );
    }

    #[test]
    fn failures_come_from_cli_diagnostics_not_the_answer() {
        let answer = "the report mentions 429, a usage limit and invalid api key\n";
        assert_eq!(classify(answer, ""), Failure::Other);
        assert_eq!(
            classify("", "error: You've hit your usage limit for this plan"),
            Failure::UsageLimit { resets_at: None }
        );
        assert_eq!(
            classify("", "devin: not logged in; run devin auth login"),
            Failure::Auth
        );
        assert_eq!(
            classify("", "request failed: 429 Too Many Requests"),
            Failure::Overloaded
        );
        assert_eq!(classify("", "model swe-high not found"), Failure::Model);
    }

    #[test]
    fn the_final_message_is_the_whole_answer() {
        assert_eq!(final_message("{\"report\":{}}\n").unwrap(), "{\"report\":{}}");
        assert!(final_message("  \n").is_err());
    }
}
