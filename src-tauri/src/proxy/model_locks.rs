//! Authoritative model cooldowns. Only wall-clock expiry releases a lock.
//! Files are independent of account/quota snapshots so stale account writes cannot unlock.
use dashmap::DashMap;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

pub const INDEPENDENT_MODELS: [&str; 6] = [
    "claude-opus-4-6-thinking",
    "gemini-3.6-flash-high",
    "gemini-3.7-flash-high",
    "gemini-3.8-flash-high",
    "gemini-3.1-pro-low",
    "gemini-3.1-flash-lite",
];

pub fn model_key(model: &str) -> String {
    let model = model.trim().to_ascii_lowercase();
    if model.starts_with("claude") {
        return "claude".into();
    }
    if INDEPENDENT_MODELS.contains(&model.as_str()) {
        return model;
    }
    // Other real Gemini model IDs also remain independent. Never collapse Flash into Pro.
    if model.starts_with("gemini-") {
        return model;
    }
    model
}

/// Prefer an absolute upstream reset timestamp, then body countdown, then Retry-After.
/// No guessed 5h/7d periods and no truncation to a backoff cap.
pub fn upstream_deadline(body: &str, retry_after: Option<&str>, now: i64) -> Option<i64> {
    fn duration(value: &serde_json::Value) -> Option<u64> {
        if let Some(n) = value.as_u64() {
            return Some(n);
        }
        let text = value.as_str()?.trim();
        if let Ok(n) = text.parse::<u64>() {
            return Some(n);
        }
        static UNITS: once_cell::sync::Lazy<regex::Regex> = once_cell::sync::Lazy::new(|| {
            regex::Regex::new(r"(?i)(\d+(?:\.\d+)?)\s*(milliseconds?|ms|seconds?|secs?|s|minutes?|mins?|m|hours?|hrs?|h|days?|d)").unwrap()
        });
        let mut total = 0.0_f64;
        let mut end = 0;
        for m in UNITS.captures_iter(text) {
            let whole = m.get(0)?;
            if !text[end..whole.start()].trim().is_empty() {
                return None;
            }
            let n: f64 = m[1].parse().ok()?;
            let unit = m[2].to_ascii_lowercase();
            total += n * if unit == "ms" || unit.starts_with("millisecond") {
                0.001
            } else if unit.starts_with('d') {
                86400.0
            } else if unit.starts_with('h') {
                3600.0
            } else if unit.starts_with('m') {
                60.0
            } else {
                1.0
            };
            end = whole.end();
        }
        (end > 0 && text[end..].trim().is_empty() && total.is_finite()).then(|| total.ceil() as u64)
    }
    fn countdown(value: &serde_json::Value, depth: usize) -> Option<u64> {
        if depth > 12 {
            return None;
        }
        match value {
            serde_json::Value::Object(map) => {
                for (key, value) in map {
                    let key = key.to_ascii_lowercase().replace('_', "");
                    if matches!(
                        key.as_str(),
                        "quotaresetdelay" | "retrydelay" | "retryafter"
                    ) {
                        if let Some(n) = duration(value) {
                            return Some(n);
                        }
                    }
                }
                map.values().find_map(|v| countdown(v, depth + 1))
            }
            serde_json::Value::Array(values) => values.iter().find_map(|v| countdown(v, depth + 1)),
            _ => None,
        }
    }
    fn timestamp(value: &serde_json::Value, depth: usize) -> Option<i64> {
        if depth > 12 {
            return None;
        }
        match value {
            serde_json::Value::Object(map) => {
                for (key, value) in map {
                    let key = key.to_ascii_lowercase().replace('_', "");
                    if matches!(
                        key.as_str(),
                        "quotaresettimestamp" | "quotaresettime" | "resettime"
                    ) {
                        if let Some(s) = value.as_str() {
                            if let Ok(t) = chrono::DateTime::parse_from_rfc3339(s) {
                                return Some(
                                    t.timestamp() + i64::from(t.timestamp_subsec_nanos() > 0),
                                );
                            }
                        }
                        if let Some(n) = value
                            .as_i64()
                            .or_else(|| value.as_str().and_then(|s| s.parse::<i64>().ok()))
                        {
                            return Some(if n > 10_000_000_000 {
                                n.saturating_add(999) / 1000
                            } else {
                                n
                            });
                        }
                    }
                    if let Some(t) = timestamp(value, depth + 1) {
                        return Some(t);
                    }
                }
                None
            }
            serde_json::Value::Array(values) => values.iter().find_map(|v| timestamp(v, depth + 1)),
            _ => None,
        }
    }
    let json_body = body.find('{').map(|i| &body[i..]).unwrap_or(body);
    if let Ok(value) = serde_json::from_str(json_body) {
        if let Some(t) = timestamp(&value, 0) {
            return Some(t);
        }
        if let Some(seconds) = countdown(&value, 0) {
            return Some(now.saturating_add(seconds.min(i64::MAX as u64) as i64));
        }
    }
    let body = body
        .replace("Resets in ", "retry after ")
        .replace("resets in ", "retry after ");
    if let Some(ms) = crate::proxy::upstream::retry::parse_retry_delay(&body, None)
        .or_else(|| crate::proxy::upstream::retry::parse_retry_delay("", retry_after))
    {
        return Some(
            now.saturating_add((ms.saturating_add(999) / 1000).min(i64::MAX as u64) as i64),
        );
    }
    retry_after
        .and_then(|s| chrono::DateTime::parse_from_rfc2822(s).ok())
        .map(|t| t.timestamp())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelLock {
    pub model: String,
    pub status: u16,
    pub reason: String,
    pub until: i64,
    pub detected_at: i64,
    pub lock_type: String,
    pub transient_count: u32,
    pub message: String,
}

#[derive(Default, Serialize, Deserialize)]
struct AccountLocks {
    account_id: String,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    display_account_id: Option<String>,
    models: HashMap<String, ModelLock>,
}

#[derive(Default)]
pub struct ModelLocks {
    accounts: DashMap<String, Arc<Mutex<AccountLocks>>>,
    identities: DashMap<String, String>,
    email_index: DashMap<String, String>,
    aliases: DashMap<String, String>,
    directory: Option<PathBuf>,
    pub storage_failed: AtomicBool,
}

impl ModelLocks {
    fn canonical_id(&self, account: &str) -> String {
        self.aliases
            .get(account)
            .map(|id| id.clone())
            .unwrap_or_else(|| account.to_owned())
    }

    /// Account UUIDs can change on deletion/reimport. Stable email identity must
    /// retain an existing cooldown; only the UI's current account ID changes.
    pub fn bind_identity(&self, account: &str, email: &str) {
        let email = email.trim().to_ascii_lowercase();
        self.identities.insert(account.to_owned(), email.clone());
        let canonical = self
            .email_index
            .get(&email)
            .map(|id| id.clone())
            .unwrap_or_else(|| account.to_owned());
        self.aliases.insert(account.to_owned(), canonical.clone());
        if let Some(entry) = self.accounts.get(&canonical).map(|entry| entry.clone()) {
            let mut value = entry.lock();
            if value.email.as_deref() != Some(email.as_str())
                || value.display_account_id.as_deref() != Some(account)
            {
                value.email = Some(email.clone());
                value.display_account_id = Some(account.to_owned());
                self.persist(&value);
            }
            self.email_index.insert(email, canonical);
        }
    }
    /// GUI, admin server and generation workers share one authoritative ledger.
    pub fn shared(data_dir: PathBuf) -> Arc<Self> {
        static STORES: once_cell::sync::Lazy<Mutex<HashMap<PathBuf, std::sync::Weak<ModelLocks>>>> =
            once_cell::sync::Lazy::new(|| Mutex::new(HashMap::new()));
        let key = data_dir.canonicalize().unwrap_or(data_dir);
        let mut stores = STORES.lock();
        if let Some(store) = stores.get(&key).and_then(std::sync::Weak::upgrade) {
            return store;
        }
        let store = Arc::new(Self::open(key.clone()));
        stores.insert(key, Arc::downgrade(&store));
        store
    }

    pub fn overlay(&self, accounts: &mut [crate::models::Account]) {
        let now = chrono::Utc::now().timestamp();
        for account in accounts {
            self.bind_identity(&account.id, &account.email);
            account
                .live_limited_models
                .retain(|_, lock| lock.until > now);
            for (key, value) in self.account(&account.id) {
                if value.until <= now {
                    continue;
                }
                account.live_limited_models.insert(
                    key.clone(),
                    crate::models::account::LiveLimitStatus {
                        model: key,
                        status: value.status,
                        reason: value.reason,
                        until: value.until,
                        detected_at: value.detected_at,
                        message: Some(value.message),
                    },
                );
            }
        }
    }

    pub fn open(data_dir: PathBuf) -> Self {
        let directory = data_dir.join("model-locks");
        let mut store = Self {
            directory: Some(directory.clone()),
            ..Self::default()
        };
        if let Err(error) = store.load(&directory) {
            tracing::error!(
                "Model-lock storage unavailable; generation blocked: {}",
                error
            );
            store.storage_failed.store(true, Ordering::Release);
        }
        store
    }

    fn load(&mut self, directory: &std::path::Path) -> Result<(), String> {
        std::fs::create_dir_all(directory).map_err(|e| e.to_string())?;
        for item in std::fs::read_dir(directory).map_err(|e| e.to_string())? {
            let path = item.map_err(|e| e.to_string())?.path();
            if path.extension().and_then(|v| v.to_str()) != Some("json") {
                continue;
            }
            let value: AccountLocks =
                serde_json::from_slice(&std::fs::read(&path).map_err(|e| e.to_string())?)
                    .map_err(|e| format!("{}: {}", path.display(), e))?;
            if let Some(email) = &value.email {
                self.email_index
                    .insert(email.clone(), value.account_id.clone());
            }
            if let Some(id) = &value.display_account_id {
                self.aliases.insert(id.clone(), value.account_id.clone());
            }
            self.accounts
                .insert(value.account_id.clone(), Arc::new(Mutex::new(value)));
        }
        Ok(())
    }

    fn persist(&self, account: &AccountLocks) {
        let Some(directory) = &self.directory else {
            return;
        };
        // Account IDs may originate in imported files; hash rather than interpolate a path.
        use sha2::{Digest, Sha256};
        let path = directory.join(format!(
            "{:x}.json",
            Sha256::digest(account.account_id.as_bytes())
        ));
        let result = serde_json::to_vec(account)
            .map_err(|e| e.to_string())
            .and_then(|bytes| crate::utils::fs::write_atomic(path, &bytes))
            .and_then(|()| {
                // Persist the renamed directory entry as well as the file contents.
                #[cfg(unix)]
                std::fs::File::open(directory)
                    .and_then(|file| file.sync_all())
                    .map_err(|e| e.to_string())?;
                Ok(())
            });
        if let Err(error) = result {
            self.storage_failed.store(true, Ordering::Release);
            tracing::error!(
                "Failed to durably save cooldown; generation blocked: {}",
                error
            );
        }
    }

    pub fn wait(&self, account: &str, model: &str, now: i64) -> u64 {
        if self.storage_failed.load(Ordering::Acquire) {
            return u64::MAX / 2;
        }
        self.get(account, model)
            .map(|v| v.until.saturating_sub(now).max(0) as u64)
            .unwrap_or(0)
    }

    pub fn get(&self, account: &str, model: &str) -> Option<ModelLock> {
        let entry = self.accounts.get(&self.canonical_id(account))?.clone();
        let value = entry.lock().models.get(&model_key(model)).cloned();
        value
    }

    pub fn account(&self, account: &str) -> HashMap<String, ModelLock> {
        let Some(entry) = self
            .accounts
            .get(&self.canonical_id(account))
            .map(|e| e.clone())
        else {
            return HashMap::new();
        };
        let value = entry.lock().models.clone();
        value
    }

    pub fn record(
        &self,
        account: &str,
        model: &str,
        status: u16,
        body: &str,
        deadline: Option<i64>,
        exact: bool,
        now: i64,
    ) -> Option<ModelLock> {
        if model.trim().is_empty() || !matches!(status, 429 | 503) {
            return None;
        }
        // Invalid/already elapsed upstream deadlines must not bypass cooldowns.
        let deadline = deadline.filter(|v| *v > now);
        let exact = exact && deadline.is_some();
        let key = model_key(model);
        let canonical = self.canonical_id(account);
        let email = self.identities.get(account).map(|value| value.clone());
        if let Some(email) = &email {
            self.email_index.insert(email.clone(), canonical.clone());
        }
        let entry = self
            .accounts
            .entry(canonical.clone())
            .or_insert_with(|| {
                Arc::new(Mutex::new(AccountLocks {
                    account_id: canonical,
                    email,
                    display_account_id: Some(account.to_owned()),
                    models: HashMap::new(),
                }))
            })
            .clone();
        let mut account = entry.lock();
        let previous = account.models.get(&key);
        // Concurrent errors cannot restart a cooldown or multiply the short-lock streak.
        // A later authoritative deadline may extend an active lock, never shorten it.
        if let Some(previous) = previous.filter(|p| p.until > now) {
            if !exact || deadline.unwrap_or(0) <= previous.until {
                return Some(previous.clone());
            }
        }
        let previous_count = previous.map(|p| p.transient_count).unwrap_or(0);
        let count = if exact {
            previous_count
        } else {
            previous_count.saturating_add(1)
        };
        let until = deadline.unwrap_or(now + if count <= 6 { 600 } else { 1800 });
        let reason = if exact {
            "ExactUpstreamDeadline"
        } else if status == 503 {
            "ServiceUnavailable"
        } else {
            "TransientRateLimit"
        };
        let value = ModelLock {
            model: key.clone(),
            status,
            reason: reason.into(),
            until,
            detected_at: now,
            lock_type: if exact {
                "exact"
            } else if deadline.is_some() {
                "short_timed"
            } else {
                "short"
            }
            .into(),
            transient_count: count,
            message: body.chars().take(600).collect(),
        };
        account.models.insert(key, value.clone());
        self.persist(&account);
        Some(value)
    }

    pub fn restore(&self, account: &str, mut value: ModelLock) {
        let now = chrono::Utc::now().timestamp();
        if value.until <= now {
            return;
        }
        let canonical = self.canonical_id(account);
        let email = self.identities.get(account).map(|value| value.clone());
        if let Some(email) = &email {
            self.email_index.insert(email.clone(), canonical.clone());
        }
        let entry = self
            .accounts
            .entry(canonical.clone())
            .or_insert_with(|| {
                Arc::new(Mutex::new(AccountLocks {
                    account_id: canonical,
                    email,
                    display_account_id: Some(account.to_owned()),
                    models: HashMap::new(),
                }))
            })
            .clone();
        let mut account = entry.lock();
        let key = model_key(&value.model);
        if account
            .models
            .get(&key)
            .is_some_and(|v| v.until >= value.until)
        {
            return;
        }
        if let Some(previous) = account.models.get(&key) {
            value.transient_count = value.transient_count.max(previous.transient_count);
        }
        account.models.insert(key, value);
        self.persist(&account);
    }

    pub fn snapshot(&self, now: i64) -> HashMap<String, HashMap<String, ModelLock>> {
        self.accounts
            .iter()
            .filter_map(|e| {
                let locks: HashMap<_, _> = e
                    .value()
                    .lock()
                    .models
                    .iter()
                    .filter(|(_, v)| v.until > now)
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();
                let id = e
                    .value()
                    .lock()
                    .display_account_id
                    .clone()
                    .unwrap_or_else(|| e.key().clone());
                (!locks.is_empty()).then_some((id, locks))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn strict_reimport_with_new_uuid_keeps_original_deadline_across_restart() {
        let dir = tempfile::tempdir().unwrap();
        let now = chrono::Utc::now().timestamp();
        let store = ModelLocks::open(dir.path().to_owned());
        store.bind_identity("old-id", "User@example.test");
        store.record(
            "old-id",
            "gemini-3.8-flash-high",
            429,
            "quota",
            Some(now + 3601),
            true,
            now,
        );
        drop(store);
        let store = ModelLocks::open(dir.path().to_owned());
        store.bind_identity("new-id", "user@example.test");
        assert_eq!(store.wait("new-id", "gemini-3.8-flash-high", now), 3601);
        assert_eq!(store.wait("new-id", "gemini-3.7-flash-high", now), 0);
        assert!(store.snapshot(now).contains_key("new-id"));
        drop(store);
        let store = ModelLocks::open(dir.path().to_owned());
        assert_eq!(store.wait("new-id", "gemini-3.8-flash-high", now), 3601);
    }
    #[test]
    fn strict_deadlines_preserve_days_fractional_seconds_and_absolute_timestamps() {
        let now = 1_700_000_000;
        assert_eq!(
            upstream_deadline(
                r#"{"metadata":{"quotaResetDelay":"7d1h2m0.1s"}}"#,
                Some("1"),
                now
            ),
            Some(now + 608521)
        );
        assert_eq!(
            upstream_deadline(
                r#"{"metadata":{"quotaResetDelay":"138.127757ms"}}"#,
                None,
                now
            ),
            Some(now + 1)
        );
        assert_eq!(
            upstream_deadline(
                r#"{"metadata":{"quotaResetTimestamp":1700000300123,"quotaResetDelay":"1s"}}"#,
                None,
                now
            ),
            Some(now + 301)
        );
        assert_eq!(upstream_deadline("busy", Some("61"), now), Some(now + 61));
    }
    #[test]
    fn strict_concurrent_errors_do_not_shorten_or_multiply_a_lock() {
        let store = Arc::new(ModelLocks::default());
        let handles: Vec<_> = (0..64)
            .map(|_| {
                let store = store.clone();
                std::thread::spawn(move || {
                    store.record("a", "gemini-3.7-flash-high", 503, "busy", None, false, 1000);
                })
            })
            .collect();
        for handle in handles {
            handle.join().unwrap();
        }
        let value = store.get("a", "gemini-3.7-flash-high").unwrap();
        assert_eq!(value.until, 1600);
        assert_eq!(value.transient_count, 1);
        assert_eq!(store.wait("a", "gemini-3.6-flash-high", 1000), 0);
    }
    #[test]
    fn strict_corrupt_storage_blocks_selection_instead_of_unlocking() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("model-locks")).unwrap();
        std::fs::write(dir.path().join("model-locks/broken.json"), "broken").unwrap();
        let store = ModelLocks::open(dir.path().to_owned());
        assert!(store.storage_failed.load(Ordering::Acquire));
        assert!(store.wait("a", "claude", 1000) > 0);
    }
    #[test]
    fn strict_locks_escalate_only_after_expiry_and_are_model_scoped() {
        let store = ModelLocks::default();
        let mut now = 1000;
        for n in 1..=8 {
            let lock = store
                .record("a", "gemini-3.8-flash-high", 429, "busy", None, false, now)
                .unwrap();
            assert_eq!(lock.transient_count, n);
            assert_eq!(lock.until - now, if n <= 6 { 600 } else { 1800 });
            let duplicate = store
                .record(
                    "a",
                    "gemini-3.8-flash-high",
                    503,
                    "busy",
                    None,
                    false,
                    now + 1,
                )
                .unwrap();
            assert_eq!(duplicate.until, lock.until);
            assert_eq!(duplicate.transient_count, n);
            assert_eq!(store.wait("a", "gemini-3.1-pro-low", now), 0);
            assert_eq!(store.wait("a", "gemini-3.8-flash-high", lock.until), 0);
            now = lock.until;
        }
    }
    #[test]
    fn strict_short_count_survives_idle_exact_locks_and_restart() {
        let dir = tempfile::tempdir().unwrap();
        let store = ModelLocks::open(dir.path().to_owned());
        let model = "gemini-3.1-pro-low";
        let mut now = chrono::Utc::now().timestamp();
        for n in 1..=6 {
            let lock = store
                .record("a", model, 503, "busy", None, false, now)
                .unwrap();
            assert_eq!(lock.transient_count, n);
            assert_eq!(lock.until - now, 600);
            now = lock.until + 7200;
        }
        let exact = store
            .record("a", model, 429, "quota", Some(now + 300), true, now)
            .unwrap();
        assert_eq!(exact.transient_count, 6);
        let mut restored = exact.clone();
        restored.until += 60;
        restored.transient_count = 0;
        store.restore("a", restored);
        drop(store);
        let store = ModelLocks::open(dir.path().to_owned());
        let now = exact.until + 7200;
        let lock = store
            .record("a", model, 503, "busy", None, false, now)
            .unwrap();
        assert_eq!(lock.transient_count, 7);
        assert_eq!(lock.until - now, 1800);
    }

    #[test]
    fn strict_529_does_not_create_or_extend_a_lock() {
        let dir = tempfile::tempdir().unwrap();
        let store = ModelLocks::open(dir.path().to_owned());
        let now = chrono::Utc::now().timestamp();
        assert!(store
            .record("a", "claude", 529, "busy", None, false, now)
            .is_none());
        assert_eq!(
            std::fs::read_dir(dir.path().join("model-locks"))
                .unwrap()
                .count(),
            0
        );
        let lock = store
            .record("a", "claude", 429, "quota", Some(now + 600), true, now)
            .unwrap();
        assert!(store
            .record("a", "claude", 529, "busy", Some(now + 9999), true, now)
            .is_none());
        assert_eq!(store.get("a", "claude").unwrap().until, lock.until);
    }

    #[test]
    fn strict_locks_restart_preserves_deadline_and_count() {
        let dir = tempfile::tempdir().unwrap();
        let now = chrono::Utc::now().timestamp();
        let store = ModelLocks::open(dir.path().to_owned());
        store.record(
            "a",
            "claude-opus-4-6-thinking",
            429,
            "quota",
            Some(now + 7861),
            true,
            now,
        );
        store.record("a", "gemini-3.1-pro-low", 503, "busy", None, false, now);
        drop(store);
        let store = ModelLocks::open(dir.path().to_owned());
        assert_eq!(store.wait("a", "claude-opus-4-6", now), 7861);
        assert_eq!(
            store
                .get("a", "gemini-3.1-pro-low")
                .unwrap()
                .transient_count,
            1
        );
        let v = store
            .record("a", "claude", 429, "late", Some(now + 20), true, now)
            .unwrap();
        assert_eq!(v.until, now + 7861);
    }
}
