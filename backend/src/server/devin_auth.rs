use super::*;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DevinCredentials {
    /// The `credentials.toml` of a `devin auth login`, pasted from any computer.
    credentials: String,
    account_id: Option<i64>,
}

/// Saves pasted Devin credentials to `accountId` (a re-sign-in) or to a new Devin account.
pub(crate) fn save_devin_credentials(request: &Request, state: &SharedState) -> Result<serde_json::Value, String> {
    require_trusted_origin(request)?;
    let input: DevinCredentials = serde_json::from_slice(&request.body).map_err(|_| "Invalid Devin request")?;
    let raw = input.credentials.trim().to_string();
    if let Some(id) = input.account_id {
        if with_db_read_raw(state, |db| db.ai_account(id))?.provider != "devin" {
            return Err("That account is not a Devin account".into());
        }
    }
    let db = state.config.db.open()?;
    let id = match input.account_id {
        Some(id) => id,
        None => {
            let count = db
                .list_ai_accounts()?
                .iter()
                .filter(|account| account.provider == "devin")
                .count();
            let label = if count == 0 {
                "Devin".to_string()
            } else {
                format!("Devin {}", count + 1)
            };
            db.create_ai_account("devin", &label)?.id
        }
    };
    let stored = crate::accounts::store_devin_credentials(id, &raw);
    if stored.is_err() && input.account_id.is_none() {
        let _ = db.delete_ai_account(id);
        let _ = crate::accounts::remove(id);
    }
    stored?;
    db.reset_ai_account_status(id)?;
    Ok(serde_json::json!({ "ok": true }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_paste_request_names_the_credentials_and_their_account() {
        assert!(serde_json::from_str::<DevinCredentials>("{}").is_err());
        assert!(serde_json::from_str::<DevinCredentials>(r#"{"credentials":"x","model":"swe-high"}"#).is_err());
        let request: DevinCredentials = serde_json::from_str(r#"{"credentials":"windsurf_api_key = \"k\""}"#).unwrap();
        assert_eq!(request.account_id, None);
    }
}
