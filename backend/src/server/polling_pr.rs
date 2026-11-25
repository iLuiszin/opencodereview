use super::*;

pub(crate) struct PolledPr {
    pub pr_id: i64,
    pub head: String,
    pub activation_eligible: bool,
    pub queue_eligible: bool,
}

type SeenHeads = std::collections::HashMap<i64, (String, std::time::Instant)>;

/// True once `head` has been the pull request's head for `quiet_seconds`, so a burst of pushes
/// starts one automatic review on its last head instead of one per push. Zero turns it off.
pub(crate) fn head_settled(
    seen: &mut SeenHeads,
    pr_id: i64,
    head: &str,
    quiet_seconds: i64,
    now: std::time::Instant,
) -> bool {
    let quiet = std::time::Duration::from_secs(quiet_seconds.max(0) as u64);
    match seen.get(&pr_id) {
        Some((seen_head, since)) if seen_head == head => now.duration_since(*since) >= quiet,
        _ => {
            seen.insert(pr_id, (head.to_string(), now));
            quiet.is_zero()
        }
    }
}

fn head_settled_now(pr_id: i64, head: &str, quiet_seconds: i64) -> bool {
    static SEEN: std::sync::OnceLock<std::sync::Mutex<SeenHeads>> = std::sync::OnceLock::new();
    let seen = SEEN.get_or_init(Default::default);
    seen.lock()
        .map(|mut seen| head_settled(&mut seen, pr_id, head, quiet_seconds, std::time::Instant::now()))
        .unwrap_or(true)
}

#[cfg(not(test))]
fn list_pr_files(full_name: &str, number: i64) -> Result<Vec<String>, String> {
    github::pr_file_paths(full_name, number)
}

#[cfg(test)]
fn list_pr_files(_: &str, _: i64) -> Result<Vec<String>, String> {
    Err("tests never list pull request files from GitHub".into())
}

/// How long a failed file listing is trusted before the next poll lists again.
pub(crate) const FILE_LISTING_RETRY: std::time::Duration = std::time::Duration::from_secs(600);

/// A pull request's file count for the review limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FileCount {
    pub(crate) count: i64,
    /// Whether `count` holds only the files a review reads, or GitHub's full count.
    pub(crate) reviewable: bool,
}

/// What a file listing gave for one head and base.
#[derive(Debug, Clone, Copy)]
pub(crate) enum FileListing {
    Reviewable(i64),
    /// GitHub stops listing at 3,000 files, so listing again cannot help.
    Incomplete,
    Failed(std::time::Instant),
}

/// The file count the review limit applies to. Above the limit, only the files a review reads
/// count (`counts_toward_file_limit`), from GitHub's file listing, remembered per head and base.
/// A listing that stops short (over 3,000 files) keeps GitHub's full count for that revision, and
/// a failed one keeps it until the listing is retried.
pub(crate) fn file_count_for_limit(
    state: &SharedState,
    repo: &Repo,
    pr: &github::GhPullRequest,
    limit: i64,
    list_files: impl FnOnce(&str, i64) -> Result<Vec<String>, String>,
) -> Option<FileCount> {
    let count = pr.changed_files?;
    let full = FileCount {
        count,
        reviewable: false,
    };
    if count <= limit {
        return Some(full);
    }
    let key = (repo.id, pr.number);
    let revision = (pr.head_ref_oid.clone(), pr.base_ref_oid.clone());
    let cached = state.reviewable_file_counts.lock().ok().and_then(|counts| {
        counts
            .get(&key)
            .filter(|(counted_at, _)| *counted_at == revision)
            .map(|(_, listing)| *listing)
    });
    let listing = match cached {
        Some(FileListing::Failed(at)) if at.elapsed() >= FILE_LISTING_RETRY => None,
        cached => cached,
    }
    .unwrap_or_else(|| {
        let listing = match list_files(&repo.full_name, pr.number) {
            Ok(paths) if (paths.len() as i64) < count => FileListing::Incomplete,
            Ok(paths) => FileListing::Reviewable(
                paths
                    .iter()
                    .filter(|path| crate::review::counts_toward_file_limit(path))
                    .count() as i64,
            ),
            Err(_) => FileListing::Failed(std::time::Instant::now()),
        };
        if let Ok(mut counts) = state.reviewable_file_counts.lock() {
            counts.insert(key, (revision, listing));
        }
        listing
    });
    Some(match listing {
        FileListing::Reviewable(count) => FileCount {
            count,
            reviewable: true,
        },
        FileListing::Incomplete | FileListing::Failed(_) => full,
    })
}

/// The review-limit count after removing the prefix already reviewed on the repository's
/// reviewed branches. `None` means the ordinary GitHub file count policy applies unchanged.
fn reviewed_branch_file_count(state: &SharedState, repo: &Repo, pr: &github::GhPullRequest) -> Option<FileCount> {
    if repo.review_reviewed_branches.is_empty() {
        return None;
    }
    let cancel = AtomicBool::new(false);
    let repo_path = state.config.repos_dir.join(&repo.owner).join(&repo.name);
    let head = crate::review::fetch_pr_head(&repo_path, pr.number, &cancel).ok()?;
    if head != pr.head_ref_oid {
        return None;
    }
    let base_ref = pr.base_ref_name.as_deref()?;
    let base_rev = crate::review::resolve_base_rev(&repo_path, base_ref, &cancel).ok()?;
    let current_scope = crate::review::scope::ReviewScope {
        head_sha: head.clone(),
        base_ref: base_ref.to_string(),
        base_sha: base_rev.clone(),
    };
    let merge_base = crate::review::scope::merge_base(&repo_path, &base_rev, &head, &cancel).ok()?;
    let prefix = crate::review::scope::reviewed_prefix_or_full(
        &repo_path,
        &repo.review_reviewed_branches,
        &merge_base,
        &head,
        &cancel,
    )
    .ok()
    .flatten()?;
    let prior_head = with_db_read_raw(state, |db| {
        let prior = db.pr_scope(repo.id, pr.number)?;
        let last = db.last_reviewed_sha_for_number(repo.id, pr.number)?;
        Ok::<_, String>(prior.filter(|scope| last.as_deref() == Some(scope.head_sha.as_str())))
    })
    .ok()
    .flatten()
    .filter(|prior| crate::review::scope::can_review_incrementally(&repo_path, prior, &current_scope, &cancel))
    .map(|scope| scope.head_sha);
    let count_base = match prior_head {
        Some(prior)
            if prior != prefix.base_sha
                && crate::review::is_ancestor(&repo_path, &prefix.base_sha, &prior, &cancel) =>
        {
            prior
        }
        _ => prefix.base_sha,
    };
    let count = crate::review::changed_file_targets_local(&repo_path, &format!("{count_base}..{head}"), &cancel)
        .ok()?
        .iter()
        .filter(|path| crate::review::counts_toward_file_limit(path))
        .count() as i64;
    Some(FileCount {
        count,
        reviewable: true,
    })
}

pub(crate) fn poll_one_pr(
    state: &SharedState,
    repo: &Repo,
    settings: &crate::db::Settings,
    target_logins: &std::collections::HashSet<String>,
    current_teams: &std::collections::HashSet<String>,
    pr: &github::GhPullRequest,
    allow_auto_start: bool,
) -> Result<PolledPr, String> {
    let matched_keyword = settings
        .auto_skip_keywords
        .iter()
        .find(|keyword| pr.title.to_ascii_lowercase().contains(&keyword.to_ascii_lowercase()));
    let activated = with_db_read_raw(state, |db| db.pr_comment_activated(repo.id, pr.number))?;
    let target_mismatch =
        !activated && settings.review_assigned_only && !pr_assigned_to(pr, target_logins, current_teams);
    let off_main = repo.default_branch.as_deref().is_some_and(|default| {
        pr.base_ref_name
            .as_deref()
            .is_some_and(|base| !base.is_empty() && base != default)
    });
    let file_count = reviewed_branch_file_count(state, repo, pr)
        .or_else(|| file_count_for_limit(state, repo, pr, settings.review_max_files, list_pr_files));
    let too_many_files = file_count.is_some_and(|files| files.count > settings.review_max_files);
    let computed_lane = author_activation_lane(
        pr.is_draft && repo.skip_draft_prs,
        activated,
        matched_keyword.is_some() || target_mismatch,
        too_many_files,
        settings.review_auto_enabled,
    );
    let status_reason = if too_many_files {
        Some(format!(
            "Skipped: {} {} exceeds the {}-file review limit. Change Maximum PR files in Settings.",
            file_count.map(|files| files.count).unwrap_or_default(),
            if file_count.is_some_and(|files| files.reviewable) {
                "reviewable changed files"
            } else {
                "changed files"
            },
            settings.review_max_files
        ))
    } else if off_main {
        Some(format!(
            "Stacked PR: reviewing `{}` → `{}` with immediate base context",
            pr.head_ref_name.as_deref().unwrap_or("head"),
            pr.base_ref_name.as_deref().unwrap_or_default()
        ))
    } else if target_mismatch {
        Some("Only PRs assigned to you or your teams are enabled".to_string())
    } else {
        matched_keyword
            .map(|keyword| format!("Matched auto-skip keyword: {keyword}"))
            .or_else(|| (!settings.review_auto_enabled).then(|| "Manual AI review configured".to_string()))
    };
    let previous_scope = with_db_raw(state, |db| db.pr_scope(repo.id, pr.number))?;
    let incoming_scope = pr
        .base_ref_name
        .as_ref()
        .zip(pr.base_ref_oid.as_ref())
        .map(|(base_ref, base_sha)| crate::review::scope::ReviewScope {
            head_sha: pr.head_ref_oid.clone(),
            base_ref: base_ref.clone(),
            base_sha: base_sha.clone(),
        });
    let base_scope_changed = previous_scope
        .as_ref()
        .zip(incoming_scope.as_ref())
        .is_some_and(|(before, after)| {
            (before.base_ref != after.base_ref || before.base_sha != after.base_sha)
                && !crate::review::scope::equivalent(
                    &state.config.repos_dir.join(&repo.owner).join(&repo.name),
                    before,
                    after,
                    &AtomicBool::new(false),
                )
        });
    let github_updated_at = pr.updated_at.as_deref().and_then(github::iso8601_to_epoch_ms);
    let (pr_id, effective_lane, decided) = with_db_raw(state, |db| {
        let prior = db.pr_poll_state(repo.id, pr.number)?;
        let pr_id = db.upsert_pull_request_with_github_updated_at(
            &crate::db::PullRequestInput {
                repo_id: repo.id,
                number: pr.number,
                title: pr.title.clone(),
                author: pr.author.login.clone(),
                author_avatar_url: pr
                    .author
                    .avatar_url
                    .clone()
                    .or_else(|| Some(github::avatar_url(&pr.author.login))),
                is_draft: pr.is_draft,
                head_sha: pr.head_ref_oid.clone(),
                base_sha: pr.base_ref_oid.clone(),
                base_ref: pr.base_ref_name.clone(),
                head_ref: pr.head_ref_name.clone(),
                default_branch: repo.default_branch.clone(),
                url: pr.url.clone(),
                lane: computed_lane.to_string(),
                status_reason: status_reason.clone(),
            },
            github_updated_at,
        )?;
        // Only a reviewable count is stored for the job-start check; GitHub's full count would
        // skip a pull request whose reviewable files fit a lowered limit.
        db.set_pr_changed_file_count(
            pr_id,
            file_count.filter(|files| files.reviewable).map(|files| files.count),
        )?;
        let has_active_review_job = state
            .review_jobs
            .lock()
            .map(|jobs| jobs.contains_key(&pr_id))
            .unwrap_or(false);
        if let Some((old_head, old_lane, _, old_status_reason)) = prior {
            let manually_paused = manual_review_pause_applies(
                &old_head,
                &old_lane,
                old_status_reason.as_deref(),
                &pr.head_ref_oid,
                computed_lane,
            );
            let managed = matches!(
                old_lane.as_str(),
                "reviewing"
                    | "queued"
                    | "ready"
                    | "failed"
                    | "posted"
                    | "auto_posted"
                    | "reviewed_cleanly"
                    | "approved"
                    | "skipped"
                    | "size_skipped"
                    | "dismissed"
                    | "closed"
            );
            if poll_scope_needs_reset(
                &old_head,
                &pr.head_ref_oid,
                &old_lane,
                computed_lane,
                base_scope_changed,
                activated,
            ) {
                if let Ok(jobs) = state.review_jobs.lock() {
                    if let Some(job) = jobs.get(&pr_id) {
                        supersede_active_review(db, pr_id, job)?;
                    }
                }
                reset_polled_review(db, pr_id, base_scope_changed, computed_lane, status_reason.as_deref())?;
            } else if (!managed
                || matches!(old_lane.as_str(), "queued" | "failed")
                    && matches!(computed_lane, "auto_skipped" | "size_skipped"))
                && old_lane != computed_lane
                && !manually_paused
            {
                db.set_pr_lane(pr_id, computed_lane, status_reason.as_deref())?;
            }
        }
        let (_, lane, last_decision_sha) = db
            .pr_state(repo.id, pr.number)?
            .unwrap_or_else(|| (pr.head_ref_oid.clone(), computed_lane.to_string(), None));
        let decided = last_decision_sha.as_deref() == Some(pr.head_ref_oid.as_str());
        if decided && !has_active_review_job && matches!(lane.as_str(), "open" | "queued" | "reviewing") {
            db.restore_decided_pr_lane(pr_id)?;
            let (_, restored_lane, _) = db
                .pr_state(repo.id, pr.number)?
                .unwrap_or_else(|| (pr.head_ref_oid.clone(), lane.clone(), last_decision_sha.clone()));
            return Ok((pr_id, restored_lane, decided));
        }
        Ok((pr_id, lane, decided))
    })?;
    let _ = with_db_raw(state, |db| {
        if effective_lane == "manual_required" && !decided {
            db.upsert_pending_action(
                "manual_review_needed",
                repo.id,
                pr_id,
                "Manual review needed",
                status_reason.as_deref().unwrap_or("Manual AI review required"),
            )
        } else if matches!(computed_lane, "auto_skipped" | "size_skipped") {
            db.resolve_pending_for_pr("manual_review_needed", pr_id)
        } else {
            Ok(())
        }
    });
    if let Some(detail) = status_reason
        .as_deref()
        .filter(|_| computed_lane == "size_skipped" && matched_keyword.is_none())
    {
        let full_name = format!("{}/{}", repo.owner, repo.name);
        let repo_path = state.config.repos_dir.join(&repo.owner).join(&repo.name);
        let _ = with_db_raw(state, |db| {
            publish_skipped_review_result(db, pr_id, None, &full_name, pr.number, &repo_path, "file-limit", detail)
        });
    }
    if allow_auto_start
        && settings.review_auto_enabled
        && effective_lane == "open"
        && !decided
        && !state
            .review_jobs
            .lock()
            .map(|jobs| jobs.contains_key(&pr_id))
            .unwrap_or(true)
        && head_settled_now(pr_id, &pr.head_ref_oid, settings.review_quiet_seconds)
    {
        let _ = start_review_job(state, pr_id, false);
    }
    Ok(PolledPr {
        pr_id,
        head: pr.head_ref_oid.clone(),
        // Only verified author commands consume this eligibility. Assignment
        // still filters ordinary polling until activation has been persisted.
        activation_eligible: matched_keyword.is_none() && !too_many_files && pr.changed_files.is_some(),
        queue_eligible: computed_lane != "draft"
            && !matches!(computed_lane, "auto_skipped" | "size_skipped")
            && pr.changed_files.is_some(),
    })
}

#[cfg(test)]
mod head_settled_tests {
    use super::{head_settled, SeenHeads};
    use std::time::{Duration, Instant};

    #[test]
    fn zero_quiet_seconds_starts_on_the_first_poll() {
        let mut seen = SeenHeads::new();
        assert!(head_settled(&mut seen, 1, "a", 0, Instant::now()));
    }

    #[test]
    fn a_head_starts_only_after_staying_unchanged_for_the_quiet_period() {
        let mut seen = SeenHeads::new();
        let start = Instant::now();
        assert!(!head_settled(&mut seen, 1, "a", 120, start));
        assert!(!head_settled(&mut seen, 1, "a", 120, start + Duration::from_secs(119)));
        assert!(head_settled(&mut seen, 1, "a", 120, start + Duration::from_secs(120)));
    }

    #[test]
    fn a_new_head_restarts_the_quiet_period() {
        let mut seen = SeenHeads::new();
        let start = Instant::now();
        assert!(!head_settled(&mut seen, 1, "a", 120, start));
        assert!(!head_settled(&mut seen, 1, "b", 120, start + Duration::from_secs(100)));
        assert!(!head_settled(&mut seen, 1, "b", 120, start + Duration::from_secs(200)));
        assert!(head_settled(&mut seen, 1, "b", 120, start + Duration::from_secs(220)));
    }

    #[test]
    fn pull_requests_are_timed_independently() {
        let mut seen = SeenHeads::new();
        let start = Instant::now();
        assert!(!head_settled(&mut seen, 1, "a", 60, start));
        assert!(!head_settled(&mut seen, 2, "a", 60, start + Duration::from_secs(50)));
        assert!(head_settled(&mut seen, 1, "a", 60, start + Duration::from_secs(60)));
        assert!(!head_settled(&mut seen, 2, "a", 60, start + Duration::from_secs(60)));
    }
}
