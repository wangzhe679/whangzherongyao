use std::time::{Duration, SystemTime};

#[derive(Clone, Copy, PartialEq, Eq)]
enum RetryParserMode {
    Current,
    Baseline,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RateLimitReason {
    QuotaExhausted,

    RateLimitExceeded,

    ModelCapacityExhausted,

    ServerError,

    Unknown,
}

pub(crate) fn normalize_image_model_id(model: &str) -> Option<String> {
    let normalized = crate::proxy::common::model_mapping::normalize_to_standard_id(model)?;
    matches!(
        normalized.as_str(),
        "gemini-3.1-flash-image" | "gemini-3-pro-image"
    )
    .then_some(normalized)
}

pub(crate) fn has_explicit_quota_exhausted(body: &str) -> bool {
    body.to_ascii_uppercase().contains("QUOTA_EXHAUSTED")
}

pub(crate) fn is_active_persisted_long_limit(
    _model_key: &str,
    status: &crate::models::account::LiveLimitStatus,
    now: i64,
) -> bool {
    // Existing persisted 529 locks still expire normally; new 529 errors cannot lock.
    matches!(status.status, 429 | 503 | 529) && status.until > now
}

pub(crate) fn is_active_persisted_long_image_limit(
    model_key: &str,
    status: &crate::models::account::LiveLimitStatus,
    now: i64,
) -> bool {
    normalize_image_model_id(model_key).is_some()
        && is_active_persisted_long_limit(model_key, status, now)
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct RateLimitInfo {
    pub reset_time: SystemTime,

    #[allow(dead_code)]
    pub retry_after_sec: u64,

    #[allow(dead_code)]
    pub detected_at: SystemTime,

    #[allow(dead_code)] // Used for logging and diagnostics
    pub reason: RateLimitReason,

    #[allow(dead_code)] // Used for model-level rate limiting
    pub model: Option<String>,
}

pub struct RateLimitTracker {
    pub strict: std::sync::Arc<crate::proxy::model_locks::ModelLocks>,
}

impl RateLimitTracker {
    pub fn new() -> Self {
        Self {
            strict: Default::default(),
        }
    }

    pub fn with_storage(data_dir: std::path::PathBuf) -> Self {
        Self {
            strict: crate::proxy::model_locks::ModelLocks::shared(data_dir),
        }
    }

    pub fn get_remaining_wait(&self, account_id: &str, model: Option<&str>) -> u64 {
        model
            .map(|m| {
                self.strict
                    .wait(account_id, m, chrono::Utc::now().timestamp())
            })
            .unwrap_or(0)
    }

    pub fn get_quota_wait(
        &self,
        _account_id: &str,
        _model: Option<&str>,
        _weekly_only: bool,
    ) -> u64 {
        0 // Quota display windows never create scheduling locks.
    }

    pub fn sync_quota_bucket(
        &self,
        _account_id: &str,
        _model: &str,
        _bucket_id: &str,
        _observed_at: i64,
        _exhausted_until: Option<SystemTime>,
        _weekly: bool,
    ) {
        // Explicit upstream errors are the only source of model cooldowns.
    }

    pub fn mark_success(&self, _account_id: &str) {
        // A success can belong to an older in-flight request. Never unlock or reset its streak.
    }

    pub fn set_lockout_until_with_cap(
        &self,
        _account_id: &str,
        _reset_time: SystemTime,
        _reason: RateLimitReason,
        _model: Option<String>,
        _cap_to_max: bool,
    ) {
        // Disabled: inferred quota windows may neither create nor replace cooldowns.
    }

    pub fn set_lockout_until(
        &self,
        account_id: &str,
        reset_time: SystemTime,
        reason: RateLimitReason,
        model: Option<String>,
    ) {
        self.set_lockout_until_with_cap(account_id, reset_time, reason, model, true);
    }

    pub fn restore_persisted_long_limit(
        &self,
        account_id: &str,
        reset_time: SystemTime,
        detected_at: SystemTime,
        model: &str,
    ) -> bool {
        let until = reset_time
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let detected = detected_at
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        if until <= chrono::Utc::now().timestamp() {
            return false;
        }
        self.strict.restore(
            account_id,
            crate::proxy::model_locks::ModelLock {
                model: crate::proxy::model_locks::model_key(model),
                status: 429,
                reason: "Restored".into(),
                until,
                detected_at: detected,
                lock_type: "exact".into(),
                transient_count: 0,
                message: String::new(),
            },
        );
        true
    }

    pub fn restore_persisted_long_image_limit(
        &self,
        account_id: &str,
        reset_time: SystemTime,
        detected_at: SystemTime,
        model: &str,
    ) -> bool {
        if normalize_image_model_id(model).is_none() {
            return false;
        }
        self.restore_persisted_long_limit(account_id, reset_time, detected_at, model)
    }

    pub fn set_lockout_until_iso_with_cap(
        &self,
        _account_id: &str,
        _reset_time_str: &str,
        _reason: RateLimitReason,
        _model: Option<String>,
        _cap_to_max: bool,
    ) -> bool {
        false // Quota snapshots must never create cooldowns.
    }

    pub fn set_lockout_until_iso(
        &self,
        account_id: &str,
        reset_time_str: &str,
        reason: RateLimitReason,
        model: Option<String>,
    ) -> bool {
        self.set_lockout_until_iso_with_cap(account_id, reset_time_str, reason, model, true)
    }

    pub fn parse_from_error(
        &self,
        account_id: &str,
        status: u16,
        retry_after_header: Option<&str>,
        body: &str,
        model: Option<String>,
        backoff_steps: &[u64], // [NEW] 传入退避配置
    ) -> Option<RateLimitInfo> {
        self.parse_from_error_with_mode(
            account_id,
            status,
            retry_after_header,
            body,
            model,
            backoff_steps,
            RetryParserMode::Current,
        )
    }

    pub fn parse_from_error_baseline(
        &self,
        account_id: &str,
        status: u16,
        retry_after_header: Option<&str>,
        body: &str,
        model: Option<String>,
        backoff_steps: &[u64],
    ) -> Option<RateLimitInfo> {
        self.parse_from_error_with_mode(
            account_id,
            status,
            retry_after_header,
            body,
            model,
            backoff_steps,
            RetryParserMode::Baseline,
        )
    }

    fn parse_from_error_with_mode(
        &self,
        account_id: &str,
        status: u16,
        retry_after_header: Option<&str>,
        body: &str,
        model: Option<String>,
        _backoff_steps: &[u64],
        _parser_mode: RetryParserMode,
    ) -> Option<RateLimitInfo> {
        if !matches!(status, 429 | 503) {
            return None;
        }
        let model = model.filter(|m| !m.trim().is_empty())?;
        let lower = body.to_ascii_lowercase();
        if [
            "all accounts limited",
            "no accounts available",
            "all accounts failed",
            "token pool is empty",
            "all accounts exhausted",
            "all accounts unhealthy",
            "global success limit",
        ]
        .iter()
        .any(|s| lower.contains(s))
        {
            return None;
        }
        let now = chrono::Utc::now().timestamp();
        let deadline = crate::proxy::model_locks::upstream_deadline(body, retry_after_header, now)
            .filter(|until| *until > now);
        let timed_model = model.starts_with("gemini") || model.starts_with("claude");
        let exact = status == 429
            && deadline.is_some()
            && (lower.contains("quota_exhausted")
                || (timed_model && lower.contains("rate_limit_exceeded")));
        let lock = self
            .strict
            .record(account_id, &model, status, body, deadline, exact, now)?;
        Some(RateLimitInfo {
            reset_time: SystemTime::UNIX_EPOCH + Duration::from_secs(lock.until.max(0) as u64),
            retry_after_sec: lock.until.saturating_sub(now).max(0) as u64,
            detected_at: SystemTime::UNIX_EPOCH
                + Duration::from_secs(lock.detected_at.max(0) as u64),
            reason: if exact {
                RateLimitReason::QuotaExhausted
            } else {
                RateLimitReason::RateLimitExceeded
            },
            model: Some(crate::proxy::model_locks::model_key(&model)),
        })
    }

    pub fn get(&self, _account_id: &str) -> Option<RateLimitInfo> {
        None // There are no account-wide cooldowns.
    }

    pub fn clear_model(&self, _account_id: &str, _model: &str) -> bool {
        false // Active cooldowns cannot be cleared; expiry is evaluated at selection time.
    }

    pub fn reconcile_quota_recovery(&self, _account_id: &str, _model: &str) -> bool {
        false // Active cooldowns cannot be cleared; expiry is evaluated at selection time.
    }

    pub fn is_rate_limited(&self, account_id: &str, model: Option<&str>) -> bool {
        // Checking using get_remaining_wait which handles both global and model keys
        self.get_remaining_wait(account_id, model) > 0
    }

    pub fn get_reset_seconds(&self, account_id: &str) -> Option<u64> {
        if let Some(info) = self.get(account_id) {
            info.reset_time
                .duration_since(SystemTime::now())
                .ok()
                .map(|d| d.as_secs())
        } else {
            None
        }
    }

    #[allow(dead_code)]
    pub fn cleanup_expired(&self) -> usize {
        0 // Keep expired records for the next short-lock streak; expired entries never block.
    }

    pub fn clear_account_only(&self, _account_id: &str) -> bool {
        false // Active cooldowns cannot be cleared; expiry is evaluated at selection time.
    }

    pub fn clear(&self, _account_id: &str) -> bool {
        false // Active cooldowns cannot be cleared; expiry is evaluated at selection time.
    }

    pub fn clear_for_optimistic_reset(&self) {
        // Intentionally no force-unlock operation.
    }

    pub fn clear_all(&self) {
        // Intentionally no force-unlock operation.
    }
}

impl Default for RateLimitTracker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod strict_tests {
    use super::*;
    #[test]
    fn strict_invalid_deadlines_fall_back_without_unlocking_and_529_is_ignored() {
        let t = RateLimitTracker::new();
        let model = "gemini-3.1-pro-low";
        for (account, status, body, header) in [
            ("zero", 429, "QUOTA_EXHAUSTED", Some("0")),
            (
                "past",
                429,
                r#"{"reason":"QUOTA_EXHAUSTED","resetTime":"2000-01-01T00:00:00Z"}"#,
                None,
            ),
            ("bad", 503, r#"{"retryDelay":"unknown"}"#, None),
        ] {
            let info = t
                .parse_from_error(account, status, header, body, Some(model.into()), &[])
                .unwrap();
            assert_eq!(info.retry_after_sec, 600);
            assert_eq!(t.strict.get(account, model).unwrap().lock_type, "short");
        }
        assert!(t
            .parse_from_error("new", 529, Some("600"), "busy", Some(model.into()), &[])
            .is_none());
        assert!(t.strict.get("new", model).is_none());
        let existing = t.strict.get("zero", model).unwrap();
        t.parse_from_error(
            "zero",
            429,
            Some("0"),
            "QUOTA_EXHAUSTED",
            Some(model.into()),
            &[],
        )
        .unwrap();
        assert_eq!(t.strict.get("zero", model).unwrap().until, existing.until);
    }

    #[test]
    fn strict_locks_cannot_be_cleared_by_any_legacy_api() {
        let t = RateLimitTracker::new();
        t.parse_from_error(
            "a",
            429,
            Some("3601"),
            "QUOTA_EXHAUSTED",
            Some("gemini-3.1-pro-low".into()),
            &[],
        )
        .unwrap();
        t.mark_success("a");
        t.clear("a");
        t.clear_all();
        t.clear_for_optimistic_reset();
        t.clear_model("a", "gemini-3.1-pro-low");
        t.reconcile_quota_recovery("a", "gemini-3.1-pro-low");
        assert!(t.get_remaining_wait("a", Some("gemini-3.1-pro-low")) >= 3600);
        assert_eq!(t.get_remaining_wait("a", Some("gemini-3.1-flash-lite")), 0);
    }
    #[test]
    fn strict_locks_timed_rate_limit_and_503_are_tracked() {
        let t = RateLimitTracker::new();
        let body = r#"{"error":{"details":[{"reason":"RATE_LIMIT_EXCEEDED","metadata":{"quotaResetDelay":"7m31s"}}]}}"#;
        let l = t
            .parse_from_error(
                "a",
                429,
                None,
                body,
                Some("claude-opus-4-6-thinking".into()),
                &[],
            )
            .unwrap();
        assert_eq!(l.retry_after_sec, 451);
        assert_eq!(t.strict.get("a", "claude").unwrap().lock_type, "exact");
        assert_eq!(
            t.parse_from_error(
                "a",
                503,
                None,
                "busy",
                Some("gemini-3.8-flash-high".into()),
                &[]
            )
            .unwrap()
            .retry_after_sec,
            600
        );
    }
}
