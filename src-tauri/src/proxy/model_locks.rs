//! Authoritative model cooldowns. Only wall-clock expiry releases a lock.
//! Files are independent of account/quota snapshots so stale account writes cannot unlock.
use dashmap::DashMap;
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

pub const INDEPENDENT_MODELS: [&str; 7] = [
    "claude-sonnet-4-6",
    "claude-opus-4-6-thinking",
    "gemini-3.6-flash-tiered",
    "gemini-3.7-flash-tiered",
    "gemini-3.8-flash-tiered",
    "gemini-3.1-pro-low",
    "gemini-3.1-flash-lite",
];

pub fn model_key(model: &str) -> String {
    let model = model.trim().to_ascii_lowercase();
    let model = model.strip_prefix("models/").unwrap_or(&model);
    let model = model.strip_prefix("[思考]").unwrap_or(model);
    if model == "claude-opus-4-6" {
        return "claude-opus-4-6-thinking".into();
    }
    if model == "claude-sonnet-4-6-thinking" {
        return "claude-sonnet-4-6".into();
    }
    for prefix in ["gemini-3.6-flash", "gemini-3.7-flash", "gemini-3.8-flash"] {
        if model == prefix
            || model
                .strip_prefix(prefix)
                .is_some_and(|suffix| matches!(suffix, "-tiered" | "-high" | "-medium" | "-low"))
        {
            return format!("{prefix}-tiered");
        }
    }
    // Other model IDs remain independent; never infer support for another model.
    model.to_owned()
}

const CLAUDE_POOL: &str = "claude";
const GEMINI_POOL: &str = "gemini-main";
const CLAUDE_MODELS: [&str; 2] = ["claude-sonnet-4-6", "claude-opus-4-6-thinking"];
const GEMINI_MODELS: [&str; 4] = [
    "gemini-3.6-flash-tiered",
    "gemini-3.7-flash-tiered",
    "gemini-3.8-flash-tiered",
    "gemini-3.1-pro-low",
];

fn quota_pool(model: &str) -> Option<&'static str> {
    if model == "claude" || CLAUDE_MODELS.contains(&model) {
        Some(CLAUDE_POOL)
    } else if GEMINI_MODELS.contains(&model) {
        Some(GEMINI_POOL)
    } else {
        None
    }
}

fn pool_models(pool: &str) -> &'static [&'static str] {
    match pool {
        CLAUDE_POOL => &CLAUDE_MODELS,
        GEMINI_POOL => &GEMINI_MODELS,
        _ => &[],
    }
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
    // Only the authoritative deadline parser changes. Retry waiting keeps its
    // existing parser. A hint must contain complete duration tokens: accepting
    // just `2h` from `6d2h` would release a quota lock six days too early.
    static TEXT_DELAY: once_cell::sync::Lazy<regex::Regex> = once_cell::sync::Lazy::new(|| {
        regex::Regex::new(
                r#"(?i)(?:quota[ \t]+will[ \t]+reset[ \t]+(?:after|in)|retry[ \t]+after|reset[ \t]+after|try[ \t]+again[ \t]+in|backoff[ \t]+for|(?:^|[\s(])wait|resets[ \t]+in)[ \t]+((?:\d+(?:\.\d+)?[ \t]*(?:milliseconds?|ms|seconds?|secs?|s|minutes?|mins?|m|hours?|hrs?|h|days?|d)[ \t]*)+)(?:$|[.,;)\]}"\s])"#,
            )
            .unwrap()
    });
    for hint in TEXT_DELAY.captures_iter(body) {
        if let Some(seconds) = duration(&serde_json::Value::String(hint[1].trim().to_owned())) {
            return Some(now.saturating_add(seconds.min(i64::MAX as u64) as i64));
        }
    }
    if let Some(seconds) =
        retry_after.and_then(|value| duration(&serde_json::Value::String(value.trim().to_owned())))
    {
        return Some(now.saturating_add(seconds.min(i64::MAX as u64) as i64));
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

#[derive(Default, Serialize)]
pub struct ModelLockStatistics {
    pub locked: usize,
    pub true_429: usize,
    pub short_locks: usize,
    // Index 0 is (0, 1d], index 5 is (5d, 6d]. No deadline is rounded or changed.
    pub expires_in_days: [usize; 6],
    pub over_six_days: usize,
    pub earliest_until: Option<i64>,
    pub latest_until: Option<i64>,
}

impl ModelLockStatistics {
    fn record(&mut self, lock: &ModelLock, now: i64) {
        let remaining = lock.until.saturating_sub(now);
        if remaining <= 0 {
            return;
        }
        self.locked += 1;
        if lock.status == 429 && lock.lock_type == "exact" {
            self.true_429 += 1;
        } else {
            self.short_locks += 1;
        }
        let day = (remaining - 1) / 86_400;
        if day < 6 {
            self.expires_in_days[day as usize] += 1;
        } else {
            self.over_six_days += 1;
        }
        self.earliest_until = Some(
            self.earliest_until
                .map_or(lock.until, |v| v.min(lock.until)),
        );
        self.latest_until = Some(self.latest_until.map_or(lock.until, |v| v.max(lock.until)));
    }
}

#[derive(Default)]
pub struct ActiveLockSummary {
    pub account_count: usize,
    pub model_count: usize,
    pub models: [ModelLockStatistics; 7],
}

#[derive(Default, Serialize, Deserialize)]
struct AccountLocks {
    account_id: String,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    display_account_id: Option<String>,
    models: HashMap<String, ModelLock>,
    /// New readers recover the actual per-model short records from this field.
    /// `models` on disk remains a projection that older releases understand.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    independent_models: Option<HashMap<String, ModelLock>>,
    /// Only confirmed 429 quota deadlines enter this ledger. Short-lock streaks
    /// stay in `models`, independently for every model, including after expiry.
    #[serde(default)]
    quota_pools: HashMap<String, ModelLock>,
}

impl AccountLocks {
    /// Duplicate UUIDs for one email are observations of the same account,
    /// not new errors. Preserve the later deadline and highest streak only.
    fn merge_records(&mut self, other: &mut Self) {
        for (key, value) in std::mem::take(&mut other.models) {
            match self.models.entry(key) {
                std::collections::hash_map::Entry::Vacant(entry) => {
                    entry.insert(value);
                }
                std::collections::hash_map::Entry::Occupied(mut entry) => {
                    let count = entry.get().transient_count.max(value.transient_count);
                    if value.until > entry.get().until {
                        entry.insert(value);
                    }
                    entry.get_mut().transient_count = count;
                }
            }
        }
        for (pool, value) in std::mem::take(&mut other.quota_pools) {
            if self
                .quota_pools
                .get(&pool)
                .is_none_or(|previous| previous.until < value.until)
            {
                self.quota_pools.insert(pool, value);
            }
        }
    }

    fn short_count(&self, key: &str) -> u32 {
        let own = self.models.get(key).map(|v| v.transient_count).unwrap_or(0);
        let legacy = if CLAUDE_MODELS.contains(&key) {
            self.models
                .get("claude")
                .map(|v| v.transient_count)
                .unwrap_or(0)
        } else {
            0
        };
        // The unknown legacy source cannot be reconstructed. Seed both models
        // conservatively once; new errors only increment the originating model.
        own.max(legacy)
    }

    fn effective(&self, key: &str) -> Option<&ModelLock> {
        let own = self.models.get(key);
        let shared = quota_pool(key).and_then(|pool| self.quota_pools.get(pool));
        // Old versions could not identify the source of a Claude short lock.
        // Preserve that restriction to expiry without sharing new short locks.
        let legacy = (key.starts_with("claude") && key != "claude")
            .then(|| self.models.get("claude"))
            .flatten();
        [own, shared, legacy]
            .into_iter()
            .flatten()
            .max_by_key(|v| v.until)
    }

    fn keys(&self) -> std::collections::HashSet<&str> {
        let mut keys: std::collections::HashSet<_> =
            self.models.keys().map(String::as_str).collect();
        if keys.remove("claude") {
            keys.extend(CLAUDE_MODELS);
        }
        for pool in self.quota_pools.keys() {
            keys.extend(pool_models(pool).iter().copied());
        }
        keys
    }

    fn view(&self, active_after: Option<i64>) -> HashMap<String, ModelLock> {
        self.keys()
            .into_iter()
            .filter_map(|key| {
                let mut value = self.effective(key)?.clone();
                if active_after.is_some_and(|now| value.until <= now) {
                    return None;
                }
                value.model = key.to_owned();
                value.transient_count = self.short_count(key);
                Some((key.to_owned(), value))
            })
            .collect()
    }

    fn migrate(&mut self) -> bool {
        if let Some(own) = self.independent_models.take() {
            self.models = own;
        }
        let mut changed = false;
        let old = std::mem::take(&mut self.models);
        for (key, mut value) in old {
            let canonical = model_key(&key);
            changed |= canonical != key || value.model != canonical;
            let key = canonical;
            value.model = key.clone();
            if value.status == 429
                && value.lock_type == "exact"
                && value.reason != "Restored"
                && value.until > 0
            {
                if let Some(pool) = quota_pool(&key) {
                    if self
                        .quota_pools
                        .get(pool)
                        .is_none_or(|p| p.until < value.until)
                    {
                        self.quota_pools.insert(pool.to_owned(), value.clone());
                    }
                    // Keep the individual streak, but its true lock now lives in the pool.
                    value.until = 0;
                    changed = true;
                }
            }
            match self.models.entry(key) {
                std::collections::hash_map::Entry::Vacant(entry) => {
                    entry.insert(value);
                }
                std::collections::hash_map::Entry::Occupied(mut entry) => {
                    let count = entry.get().transient_count.max(value.transient_count);
                    if value.until > entry.get().until {
                        entry.insert(value);
                    }
                    entry.get_mut().transient_count = count;
                }
            }
        }
        changed
    }

    fn compatibility_models(&self) -> HashMap<String, ModelLock> {
        let mut models = self.view(None);
        // Older versions always looked up Claude through one `claude` key.
        let claude = CLAUDE_MODELS
            .iter()
            .filter_map(|key| self.effective(key))
            .chain(self.models.get("claude"))
            .max_by_key(|v| v.until);
        if let Some(value) = claude {
            let mut value = value.clone();
            value.model = "claude".into();
            value.transient_count = CLAUDE_MODELS
                .iter()
                .map(|key| self.short_count(key))
                .max()
                .unwrap_or(0);
            models.insert("claude".into(), value);
        }
        for prefix in ["gemini-3.6-flash", "gemini-3.7-flash", "gemini-3.8-flash"] {
            if let Some(value) = models.get(&format!("{prefix}-tiered")) {
                let mut value = value.clone();
                value.model = format!("{prefix}-high");
                models.insert(value.model.clone(), value);
            }
        }
        models
    }
}

#[derive(Serialize)]
struct PersistedAccountLocks<'a> {
    account_id: &'a str,
    email: &'a Option<String>,
    display_account_id: &'a Option<String>,
    models: HashMap<String, ModelLock>,
    independent_models: &'a HashMap<String, ModelLock>,
    quota_pools: &'a HashMap<String, ModelLock>,
}

#[derive(Default)]
pub struct ModelLocks {
    accounts: DashMap<String, Arc<Mutex<AccountLocks>>>,
    identities: DashMap<String, String>,
    email_index: DashMap<String, String>,
    aliases: DashMap<String, String>,
    /// Identity changes wait for an already selected ledger to be locked.
    /// Normal account writes remain parallel and release this read gate before fsync.
    identity_gate: RwLock<()>,
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
        if email.is_empty() {
            return; // Unknown identities remain UUID-scoped.
        }
        // The usual binding is already current. Keep it on the shared read
        // path so one account's disk flush cannot delay other accounts.
        {
            let _identity_read = self.identity_gate.read();
            if self
                .identities
                .get(account)
                .is_some_and(|value| value.as_str() == email)
            {
                let canonical = self.canonical_id(account);
                if self
                    .email_index
                    .get(&email)
                    .is_some_and(|value| value.as_str() == canonical)
                    && (canonical == account || !self.accounts.contains_key(account))
                {
                    match self.accounts.get(&canonical).map(|entry| entry.clone()) {
                        Some(entry) => {
                            let value = entry.lock();
                            if value.email.as_deref() == Some(email.as_str())
                                && value.display_account_id.as_deref() == Some(account)
                            {
                                return;
                            }
                        }
                        None => return,
                    }
                }
            }
        }
        let _identity_change = self.identity_gate.write();
        let previous_id = self.canonical_id(account);
        self.identities.insert(account.to_owned(), email.clone());
        let canonical = self
            .email_index
            .get(&email)
            .map(|id| id.clone())
            .unwrap_or_else(|| account.to_owned());
        if previous_id != canonical {
            if let Some(previous) = self.accounts.get(&previous_id).map(|entry| entry.clone()) {
                let mut previous = previous.lock();
                // An unknown email can be bound for the first time. A known
                // different email must never donate its model restrictions.
                if previous.email.as_deref().is_none_or(|value| value == email) {
                    let target = self
                        .accounts
                        .entry(canonical.clone())
                        .or_insert_with(|| {
                            Arc::new(Mutex::new(AccountLocks {
                                account_id: canonical.clone(),
                                email: Some(email.clone()),
                                ..AccountLocks::default()
                            }))
                        })
                        .clone();
                    let mut target = target.lock();
                    target.merge_records(&mut previous);
                    target.email = Some(email.clone());
                    target.display_account_id = Some(account.to_owned());
                    self.persist(&target);
                    // Keep the retired UUID's file compatible too. In
                    // particular, a pre-email ledger must not reappear as a
                    // separate unknown account after the next restart.
                    previous.email = Some(email.clone());
                    previous.models = target.models.clone();
                    previous.quota_pools = target.quota_pools.clone();
                    self.persist(&previous);
                    self.accounts.remove(&previous_id);
                    for mut alias in self.aliases.iter_mut() {
                        if alias.value() == &previous_id {
                            *alias.value_mut() = canonical.clone();
                        }
                    }
                    self.aliases.insert(previous_id, canonical.clone());
                }
            }
        }
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
        }
        // Reserve the email identity even before its first error creates a
        // ledger, so two UUIDs cannot start independent records for one email.
        self.email_index.insert(email, canonical);
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
        let mut records = Vec::new();
        for item in std::fs::read_dir(directory).map_err(|e| e.to_string())? {
            let path = item.map_err(|e| e.to_string())?.path();
            if path.extension().and_then(|v| v.to_str()) != Some("json") {
                continue;
            }
            let mut value: AccountLocks =
                serde_json::from_slice(&std::fs::read(&path).map_err(|e| e.to_string())?)
                    .map_err(|e| format!("{}: {}", path.display(), e))?;
            let migrated = value.migrate();
            records.push((value, migrated));
        }
        // A stable internal UUID makes file enumeration order irrelevant.
        records.sort_by(|(left, _), (right, _)| left.account_id.cmp(&right.account_id));
        for (mut value, migrated) in records {
            value.email = value
                .email
                .take()
                .map(|email| email.trim().to_ascii_lowercase())
                .filter(|email| !email.is_empty());
            let canonical = value
                .email
                .as_ref()
                .and_then(|email| self.email_index.get(email).map(|entry| entry.clone()))
                .unwrap_or_else(|| value.account_id.clone());
            if let Some(email) = &value.email {
                self.email_index.insert(email.clone(), canonical.clone());
                self.identities
                    .insert(value.account_id.clone(), email.clone());
            }
            self.aliases
                .insert(value.account_id.clone(), canonical.clone());
            if let Some(id) = &value.display_account_id {
                self.aliases.insert(id.clone(), canonical.clone());
                if let Some(email) = &value.email {
                    self.identities.insert(id.clone(), email.clone());
                }
            }
            if let Some(target) = self.accounts.get(&canonical).map(|entry| entry.clone()) {
                let mut target = target.lock();
                if target.email != value.email {
                    return Err(format!(
                        "Conflicting model-lock emails for account {}",
                        value.account_id
                    ));
                }
                target.merge_records(&mut value);
                self.persist(&target);
                continue;
            }
            if migrated {
                self.persist(&value);
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
        let persisted = PersistedAccountLocks {
            account_id: &account.account_id,
            email: &account.email,
            display_account_id: &account.display_account_id,
            models: account.compatibility_models(),
            independent_models: &account.models,
            quota_pools: &account.quota_pools,
        };
        let result = serde_json::to_vec(&persisted)
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
        let identity_read = self.identity_gate.read();
        let Some(entry) = self
            .accounts
            .get(&self.canonical_id(account))
            .map(|e| e.clone())
        else {
            return 0;
        };
        let account = entry.lock();
        drop(identity_read);
        if self.storage_failed.load(Ordering::Acquire) {
            return u64::MAX / 2;
        }
        account
            .effective(&model_key(model))
            .map(|v| v.until.saturating_sub(now).max(0) as u64)
            .unwrap_or(0)
    }

    pub fn get(&self, account: &str, model: &str) -> Option<ModelLock> {
        let identity_read = self.identity_gate.read();
        let entry = self.accounts.get(&self.canonical_id(account))?.clone();
        let account = entry.lock();
        drop(identity_read);
        let key = model_key(model);
        let mut value = account.effective(&key)?.clone();
        value.model = key.clone();
        value.transient_count = account.short_count(&key);
        Some(value)
    }

    pub fn account(&self, account: &str) -> HashMap<String, ModelLock> {
        let identity_read = self.identity_gate.read();
        let Some(entry) = self
            .accounts
            .get(&self.canonical_id(account))
            .map(|e| e.clone())
        else {
            return HashMap::new();
        };
        let account = entry.lock();
        drop(identity_read);
        let value = account.view(None);
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
        let exact = status == 429 && exact && deadline.is_some();
        let key = model_key(model);
        let identity_read = self.identity_gate.read();
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
                    independent_models: None,
                    quota_pools: HashMap::new(),
                }))
            })
            .clone();
        let mut account = entry.lock();
        drop(identity_read);
        let previous = account.models.get(&key);
        let previous_count = account.short_count(&key);
        if exact {
            if let Some(pool) = quota_pool(&key) {
                let until = deadline.unwrap();
                if account
                    .quota_pools
                    .get(pool)
                    .is_none_or(|v| v.until < until)
                {
                    account.quota_pools.insert(
                        pool.to_owned(),
                        ModelLock {
                            model: key.clone(),
                            status,
                            reason: "ExactUpstreamDeadline".into(),
                            until,
                            detected_at: now,
                            lock_type: "exact".into(),
                            transient_count: 0,
                            message: body.chars().take(600).collect(),
                        },
                    );
                    self.persist(&account);
                }
                let mut effective = account.effective(&key).unwrap().clone();
                effective.model = key;
                effective.transient_count = previous_count;
                return Some(effective);
            }
        }
        // Concurrent errors cannot restart a cooldown or multiply the short-lock streak.
        // A later authoritative deadline may extend an active lock, never shorten it.
        if let Some(previous) = previous.filter(|p| p.until > now) {
            if !exact || deadline.unwrap_or(0) <= previous.until {
                let mut value = account.effective(&key).unwrap().clone();
                value.model = key;
                value.transient_count = previous_count;
                return Some(value);
            }
        }
        // An in-flight error during a shared true lock must not manufacture a
        // short lock that outlives the already authoritative quota deadline.
        if let Some(active) = account.effective(&key).filter(|v| v.until > now) {
            if !exact {
                let mut value = active.clone();
                value.model = key;
                value.transient_count = previous_count;
                return Some(value);
            }
        }
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
        let identity_read = self.identity_gate.read();
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
                    independent_models: None,
                    quota_pools: HashMap::new(),
                }))
            })
            .clone();
        let mut account = entry.lock();
        drop(identity_read);
        let key = model_key(&value.model);
        value.model = key.clone();
        if value.status == 429 && value.lock_type == "exact" {
            if let Some(pool) = quota_pool(&key) {
                if account
                    .quota_pools
                    .get(pool)
                    .is_none_or(|p| p.until < value.until)
                {
                    account.quota_pools.insert(pool.to_owned(), value.clone());
                }
                // Restored snapshots may carry a model's previous short-lock streak.
                if let Some(previous) = account.models.get_mut(&key) {
                    previous.transient_count = previous.transient_count.max(value.transient_count);
                } else if value.transient_count > 0 {
                    value.until = 0;
                    account.models.insert(key, value);
                }
                self.persist(&account);
                return;
            }
        }
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
        let _identity_read = self.identity_gate.read();
        self.accounts
            .iter()
            .filter_map(|e| {
                let account = e.value().lock();
                let locks = account.view(Some(now));
                let id = account
                    .display_account_id
                    .clone()
                    .unwrap_or_else(|| e.key().clone());
                (!locks.is_empty()).then_some((id, locks))
            })
            .collect()
    }

    /// Dashboard totals need neither cloned error bodies nor account disk reads.
    pub fn active_counts(&self, now: i64) -> (usize, usize) {
        let _identity_read = self.identity_gate.read();
        let mut accounts = 0;
        let mut models = 0;
        for entry in self.accounts.iter() {
            let account = entry.value().lock();
            let count = account
                .keys()
                .into_iter()
                .filter(|key| account.effective(key).is_some_and(|v| v.until > now))
                .count();
            accounts += usize::from(count > 0);
            models += count;
        }
        (accounts, models)
    }

    /// Read-only aggregation: fixed counters, borrowed lock records, no bodies
    /// or account lists copied, and no cooldown writes or scheduler changes.
    pub fn active_summary(&self, now: i64) -> ActiveLockSummary {
        let _identity_read = self.identity_gate.read();
        let mut summary = ActiveLockSummary::default();
        for entry in self.accounts.iter() {
            let account = entry.value().lock();
            let mut active = false;
            for key in account.keys() {
                let Some(lock) = account.effective(key).filter(|v| v.until > now) else {
                    continue;
                };
                active = true;
                summary.model_count += 1;
                if let Some(index) = INDEPENDENT_MODELS.iter().position(|model| *model == key) {
                    summary.models[index].record(lock, now);
                }
            }
            summary.account_count += usize::from(active);
        }
        summary
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strict_dashboard_lock_statistics_use_effective_deadlines_and_exact_day_boundaries() {
        let store = ModelLocks::default();
        let now = 10_000;
        let day = 86_400;
        for (index, seconds) in [
            day,
            day + 1,
            2 * day,
            3 * day,
            4 * day,
            5 * day,
            6 * day,
            6 * day + 1,
        ]
        .into_iter()
        .enumerate()
        {
            store
                .record(
                    &format!("true-{index}"),
                    CLAUDE_MODELS[0],
                    429,
                    "quota",
                    Some(now + seconds),
                    true,
                    now,
                )
                .unwrap();
        }
        // A generic 429 with a precise transient time stays a model-only short lock.
        store
            .record(
                "fake",
                CLAUDE_MODELS[0],
                429,
                "busy",
                Some(now + 600),
                false,
                now,
            )
            .unwrap();
        store
            .record("service", CLAUDE_MODELS[1], 503, "busy", None, false, now)
            .unwrap();
        store
            .record(
                "expired",
                CLAUDE_MODELS[0],
                429,
                "quota",
                Some(now + 1),
                true,
                now - 10,
            )
            .unwrap();
        let summary = store.active_summary(now + 1);
        assert_eq!((summary.account_count, summary.model_count), (10, 18));
        let sonnet = &summary.models[0];
        assert_eq!(
            (sonnet.locked, sonnet.true_429, sonnet.short_locks),
            (9, 8, 1)
        );
        assert_eq!(sonnet.expires_in_days, [3, 1, 1, 1, 1, 2]);
        assert_eq!(sonnet.over_six_days, 0);
        assert_eq!(sonnet.earliest_until, Some(now + 600));
        assert_eq!(sonnet.latest_until, Some(now + 6 * day + 1));
        assert_eq!(summary.models[1].short_locks, 1);
        assert_eq!(summary.models[1].true_429, 8);
        let boundaries = store.active_summary(now);
        assert_eq!(boundaries.models[0].expires_in_days, [3, 2, 1, 1, 1, 1]);
        assert_eq!(boundaries.models[0].over_six_days, 1);
        // Summarizing does not change stored locks or the selected deadline.
        assert_eq!(
            store.wait("true-7", CLAUDE_MODELS[1], now),
            (6 * day + 1) as u64
        );
    }

    #[test]
    fn strict_aliases_preserve_separate_model_identities() {
        assert_eq!(
            model_key("[思考]claude-opus-4-6"),
            "claude-opus-4-6-thinking"
        );
        assert_eq!(
            model_key("models/claude-opus-4-6"),
            "claude-opus-4-6-thinking"
        );
        assert_eq!(model_key("claude-sonnet-4-6"), "claude-sonnet-4-6");
        assert_eq!(model_key("claude-sonnet-4-6-thinking"), "claude-sonnet-4-6");
        for version in ["3.6", "3.7", "3.8"] {
            let prefix = format!("gemini-{version}-flash");
            for suffix in ["", "-high", "-medium", "-low", "-tiered"] {
                assert_eq!(
                    model_key(&format!("{prefix}{suffix}")),
                    format!("{prefix}-tiered")
                );
            }
        }
        assert_eq!(model_key("gemini-3.1-pro-high"), "gemini-3.1-pro-high");
        assert_eq!(
            model_key("gemini-3.8-flash-image"),
            "gemini-3.8-flash-image"
        );
    }

    #[test]
    fn strict_true_locks_share_only_with_same_account_pool_and_deadline() {
        let store = ModelLocks::default();
        let now = 1000;
        for source in CLAUDE_MODELS.into_iter().chain(GEMINI_MODELS) {
            let account = source;
            store.record(account, source, 429, "quota", Some(now + 900), true, now);
            let pool = quota_pool(source).unwrap();
            for model in INDEPENDENT_MODELS {
                let expected = if quota_pool(model) == Some(pool) {
                    900
                } else {
                    0
                };
                assert_eq!(
                    store.wait(account, model, now),
                    expected,
                    "{source} -> {model}"
                );
                assert_eq!(store.wait("unrelated", model, now), 0);
                assert_eq!(store.wait(account, model, now + 900), 0);
            }
        }
        store.record(
            "lite",
            "gemini-3.1-flash-lite",
            429,
            "quota",
            Some(now + 900),
            true,
            now,
        );
        assert_eq!(store.wait("lite", "gemini-3.1-flash-lite", now), 900);
        assert_eq!(store.wait("lite", "gemini-3.1-pro-low", now), 0);
    }

    #[test]
    fn strict_fake_and_503_locks_never_share_even_with_deadline() {
        let store = ModelLocks::default();
        for (index, model) in INDEPENDENT_MODELS.iter().enumerate() {
            let account = index.to_string();
            store.record(&account, model, 429, "busy", Some(2000), false, 1000);
            for peer in INDEPENDENT_MODELS {
                assert_eq!(
                    store.wait(&account, peer, 1000),
                    if peer == *model { 1000 } else { 0 }
                );
            }
            store.record(
                &account,
                model,
                503,
                "QUOTA_EXHAUSTED",
                Some(9000),
                true,
                2000,
            );
            for peer in INDEPENDENT_MODELS {
                assert_eq!(
                    store.wait(&account, peer, 2000),
                    if peer == *model { 7000 } else { 0 }
                );
            }
        }
    }

    #[test]
    fn strict_pool_preserves_independent_short_streaks_after_expiry() {
        let store = ModelLocks::default();
        let mut now = 1000;
        for count in 1..=6 {
            let value = store
                .record("a", CLAUDE_MODELS[0], 503, "busy", None, false, now)
                .unwrap();
            assert_eq!(value.transient_count, count);
            now = value.until;
        }
        store.record(
            "a",
            CLAUDE_MODELS[1],
            429,
            "quota",
            Some(now + 300),
            true,
            now,
        );
        assert_eq!(store.get("a", CLAUDE_MODELS[0]).unwrap().transient_count, 6);
        assert_eq!(store.get("a", CLAUDE_MODELS[1]).unwrap().transient_count, 0);
        store.record("a", CLAUDE_MODELS[0], 503, "busy", None, false, now);
        now += 300;
        let sonnet = store
            .record("a", CLAUDE_MODELS[0], 503, "busy", None, false, now)
            .unwrap();
        let opus = store
            .record("a", CLAUDE_MODELS[1], 503, "busy", None, false, now)
            .unwrap();
        assert_eq!((sonnet.transient_count, sonnet.until - now), (7, 1800));
        assert_eq!((opus.transient_count, opus.until - now), (1, 600));
    }

    #[test]
    fn strict_legacy_migration_keeps_unknown_claude_short_and_flash_deadlines() {
        let dir = tempfile::tempdir().unwrap();
        let now = chrono::Utc::now().timestamp();
        let lock = |model: &str, exact: bool, until: i64| ModelLock {
            model: model.into(),
            status: 429,
            reason: if exact {
                "ExactUpstreamDeadline"
            } else {
                "TransientRateLimit"
            }
            .into(),
            until,
            detected_at: now,
            lock_type: if exact { "exact" } else { "short" }.into(),
            transient_count: 5,
            message: "legacy".into(),
        };
        let value = serde_json::json!({
            "account_id":"a", "email":"same@example.test", "models": {
                "claude":lock("claude", false, now + 600),
                "gemini-3.7-flash-high":lock("gemini-3.7-flash-high", true, now + 800),
                "gemini-3.8-flash-high":lock("gemini-3.8-flash-high", false, now + 700),
            }
        });
        std::fs::create_dir(dir.path().join("model-locks")).unwrap();
        use sha2::{Digest, Sha256};
        let file = dir
            .path()
            .join("model-locks")
            .join(format!("{:x}.json", Sha256::digest(b"a")));
        std::fs::write(file, serde_json::to_vec(&value).unwrap()).unwrap();
        let store = ModelLocks::open(dir.path().to_owned());
        for model in CLAUDE_MODELS {
            assert_eq!(store.wait("a", model, now), 600);
        }
        for model in GEMINI_MODELS {
            assert_eq!(store.wait("a", model, now), 800);
        }
        let short = store
            .record("a", CLAUDE_MODELS[0], 503, "busy", None, false, now + 600)
            .unwrap();
        assert_eq!(short.transient_count, 6);
        assert_eq!(store.wait("a", CLAUDE_MODELS[1], now + 600), 0);
        assert_eq!(
            store
                .account("a")
                .get(GEMINI_MODELS[1])
                .unwrap()
                .transient_count,
            5
        );
        assert_eq!(store.active_counts(now), (1, 6));
        assert_eq!(store.snapshot(now)["a"].len(), 6);
    }

    #[test]
    fn strict_concurrent_pool_errors_use_one_later_deadline_without_streaks() {
        let store = Arc::new(ModelLocks::default());
        let handles: Vec<_> = (0..64)
            .map(|index| {
                let store = store.clone();
                std::thread::spawn(move || {
                    store.record(
                        "a",
                        GEMINI_MODELS[index % 4],
                        429,
                        "quota",
                        Some(2000 + index as i64),
                        true,
                        1000,
                    );
                })
            })
            .collect();
        for handle in handles {
            handle.join().unwrap();
        }
        for model in GEMINI_MODELS {
            let lock = store.get("a", model).unwrap();
            assert_eq!(lock.until, 2063);
            assert_eq!(lock.transient_count, 0);
        }
        assert_eq!(store.active_counts(1000), (1, 4));
    }

    #[test]
    fn strict_persisted_compatibility_locks_keep_old_readers_safe_without_merging_new_streaks() {
        #[derive(Deserialize)]
        struct LegacyAccountLocks {
            models: HashMap<String, ModelLock>,
        }
        let dir = tempfile::tempdir().unwrap();
        let start = chrono::Utc::now().timestamp();
        let store = ModelLocks::open(dir.path().to_owned());
        store.record("a", CLAUDE_MODELS[0], 503, "busy", None, false, start);
        let now = start + 600;
        store.record("a", CLAUDE_MODELS[0], 503, "busy", None, false, now);
        store.record("a", CLAUDE_MODELS[1], 503, "busy", None, false, now);
        store.record(
            "a",
            CLAUDE_MODELS[1],
            429,
            "quota",
            Some(now + 5000),
            true,
            now,
        );
        store.record("a", GEMINI_MODELS[0], 503, "busy", None, false, now);
        store.record(
            "a",
            GEMINI_MODELS[1],
            429,
            "quota",
            Some(now + 4000),
            true,
            now,
        );
        let file = std::fs::read_dir(dir.path().join("model-locks"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let persisted = std::fs::read(file).unwrap();
        let legacy: LegacyAccountLocks = serde_json::from_slice(&persisted).unwrap();
        assert_eq!(legacy.models["claude"].until, now + 5000);
        assert_eq!(legacy.models["claude"].lock_type, "exact");
        for model in [
            "gemini-3.6-flash-high",
            "gemini-3.7-flash-high",
            "gemini-3.8-flash-high",
            "gemini-3.1-pro-low",
        ] {
            assert_eq!(legacy.models[model].until, now + 4000);
            assert_eq!(legacy.models[model].lock_type, "exact");
        }
        drop(store);
        let store = ModelLocks::open(dir.path().to_owned());
        assert_eq!(store.get("a", CLAUDE_MODELS[0]).unwrap().transient_count, 2);
        assert_eq!(store.get("a", CLAUDE_MODELS[1]).unwrap().transient_count, 1);
        assert_eq!(store.wait("a", "claude-opus-4-6", now), 5000);
        assert_eq!(store.wait("a", GEMINI_MODELS[3], now), 4000);
        let now = now + 5000;
        let sonnet = store
            .record("a", CLAUDE_MODELS[0], 503, "busy", None, false, now)
            .unwrap();
        let opus = store
            .record("a", CLAUDE_MODELS[1], 503, "busy", None, false, now)
            .unwrap();
        let flash_a = store
            .record("a", GEMINI_MODELS[0], 503, "busy", None, false, now)
            .unwrap();
        let flash_b = store
            .record("a", GEMINI_MODELS[1], 503, "busy", None, false, now)
            .unwrap();
        assert_eq!((sonnet.transient_count, opus.transient_count), (3, 2));
        assert_eq!((flash_a.transient_count, flash_b.transient_count), (2, 1));
        assert_eq!(store.active_counts(now), (1, 4));
    }
    #[test]
    fn strict_duplicate_email_ledgers_preserve_later_deadlines_in_either_file_order() {
        let now = chrono::Utc::now().timestamp();
        let lock = |model: &str, status, exact: bool, until, count| ModelLock {
            model: model.into(),
            status,
            reason: if exact {
                "ExactUpstreamDeadline"
            } else {
                "ServiceUnavailable"
            }
            .into(),
            until,
            detected_at: now,
            lock_type: if exact { "exact" } else { "short_timed" }.into(),
            transient_count: count,
            message: "existing upstream error".into(),
        };
        for reverse in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("model-locks");
            std::fs::create_dir(&path).unwrap();
            let values = [
                serde_json::json!({
                    "account_id":"a-canonical", "email":" USER@example.test ", "models": {
                        "gemini-3.7-flash-high":lock("gemini-3.7-flash-high", 429, true, now + 2 * 86400, 2),
                        "gemini-3.6-flash-high":lock("gemini-3.6-flash-high", 503, false, now - 1, 8),
                        "claude-sonnet-4-6":lock("claude-sonnet-4-6", 503, false, now + 600, 3),
                    }
                }),
                serde_json::json!({
                    "account_id":"b-current", "email":"user@example.test", "models": {
                        "gemini-3.1-pro-low":lock("gemini-3.1-pro-low", 429, true, now + 6 * 86400, 6),
                        "gemini-3.8-flash-high":lock("gemini-3.8-flash-high", 503, false, now + 9 * 86400, 5),
                        "gemini-3.1-flash-lite":lock("gemini-3.1-flash-lite", 503, false, now + 3 * 86400, 7),
                        "gemini-3.6-flash-high":lock("gemini-3.6-flash-high", 503, false, now - 2, 4),
                    }
                }),
                serde_json::json!({
                    "account_id":"c-unrelated", "email":"other@example.test", "models": {
                        "gemini-3.1-pro-low":lock("gemini-3.1-pro-low", 429, true, now + 86400, 1),
                    }
                }),
            ];
            for index in 0..values.len() {
                let value = &values[if reverse {
                    values.len() - 1 - index
                } else {
                    index
                }];
                std::fs::write(
                    path.join(format!("fixture-{index}.json")),
                    serde_json::to_vec(value).unwrap(),
                )
                .unwrap();
            }
            let store = ModelLocks::open(dir.path().to_owned());
            assert!(!store.storage_failed.load(Ordering::Acquire));
            store.bind_identity("b-current", "user@example.test");
            for account in ["a-canonical", "b-current"] {
                for model in [GEMINI_MODELS[0], GEMINI_MODELS[1], GEMINI_MODELS[3]] {
                    assert_eq!(store.wait(account, model, now), 6 * 86400);
                    assert_eq!(store.get(account, model).unwrap().lock_type, "exact");
                }
                let fake = store.get(account, GEMINI_MODELS[2]).unwrap();
                assert_eq!(
                    (fake.until, fake.status, fake.transient_count),
                    (now + 9 * 86400, 503, 5)
                );
                assert_eq!(fake.lock_type, "short_timed");
                let lite = store.get(account, "gemini-3.1-flash-lite").unwrap();
                assert_eq!(
                    (lite.until, lite.status, lite.transient_count),
                    (now + 3 * 86400, 503, 7)
                );
                assert_eq!(store.wait(account, CLAUDE_MODELS[0], now), 600);
                assert_eq!(store.wait(account, CLAUDE_MODELS[1], now), 0);
                assert_eq!(
                    store
                        .get(account, GEMINI_MODELS[0])
                        .unwrap()
                        .transient_count,
                    8
                );
            }
            assert_eq!(store.wait("c-unrelated", GEMINI_MODELS[3], now), 86400);
            assert_eq!(store.active_counts(now), (2, 10));
            drop(store);
            let store = ModelLocks::open(dir.path().to_owned());
            store.bind_identity("reimported", "USER@example.test");
            assert_eq!(store.wait("reimported", GEMINI_MODELS[3], now), 6 * 86400);
            assert_eq!(store.wait("b-current", GEMINI_MODELS[2], now), 9 * 86400);
            assert_eq!(
                store.wait("reimported", "gemini-3.1-flash-lite", now),
                3 * 86400
            );
            let next = store
                .record(
                    "reimported",
                    GEMINI_MODELS[0],
                    503,
                    "busy",
                    None,
                    false,
                    now + 6 * 86400,
                )
                .unwrap();
            assert_eq!(
                (next.transient_count, next.until - (now + 6 * 86400)),
                (9, 1800)
            );
        }
    }

    #[test]
    fn strict_identity_binding_merges_current_uuid_existing_longer_ledger_before_redirecting() {
        let dir = tempfile::tempdir().unwrap();
        let now = chrono::Utc::now().timestamp();
        let store = ModelLocks::open(dir.path().to_owned());
        store.bind_identity("old", "same@example.test");
        store.record(
            "old",
            GEMINI_MODELS[0],
            429,
            "quota",
            Some(now + 600),
            true,
            now,
        );
        // A current UUID may already have a legacy ledger before its email is loaded.
        store.record(
            "current",
            GEMINI_MODELS[3],
            429,
            "quota",
            Some(now + 6 * 86400),
            true,
            now,
        );
        store.restore(
            "current",
            ModelLock {
                model: "gemini-3.1-flash-lite".into(),
                status: 503,
                reason: "ServiceUnavailable".into(),
                until: now + 1800,
                detected_at: now,
                lock_type: "short".into(),
                transient_count: 7,
                message: "busy".into(),
            },
        );
        store.bind_identity("current", "same@example.test");
        assert_eq!(store.accounts.len(), 1);
        for account in ["old", "current"] {
            for model in GEMINI_MODELS {
                assert_eq!(store.wait(account, model, now), 6 * 86400);
            }
            let lite = store.get(account, "gemini-3.1-flash-lite").unwrap();
            assert_eq!(
                (lite.until, lite.status, lite.transient_count),
                (now + 1800, 503, 7)
            );
            assert_eq!(lite.lock_type, "short");
        }
        drop(store);
        let store = ModelLocks::open(dir.path().to_owned());
        assert_eq!(store.accounts.len(), 1);
        store.bind_identity("new", "same@example.test");
        assert_eq!(store.wait("new", GEMINI_MODELS[0], now), 6 * 86400);
        assert_eq!(
            store
                .get("new", "gemini-3.1-flash-lite")
                .unwrap()
                .transient_count,
            7
        );
    }

    #[test]
    fn strict_duplicate_identity_binding_and_concurrent_errors_keep_every_longer_deadline() {
        let store = Arc::new(ModelLocks::default());
        store.bind_identity("original", "same@example.test");
        let barrier = Arc::new(std::sync::Barrier::new(32));
        let handles: Vec<_> = (0..32)
            .map(|index| {
                let store = store.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    let account = format!("current-{index}");
                    store.record(
                        &account,
                        GEMINI_MODELS[3],
                        429,
                        "quota",
                        Some(2000 + index),
                        true,
                        1000,
                    );
                    barrier.wait();
                    store.bind_identity(&account, "same@example.test");
                    store.record(
                        &account,
                        GEMINI_MODELS[0],
                        429,
                        "quota",
                        Some(3000 + index),
                        true,
                        1000,
                    );
                })
            })
            .collect();
        for handle in handles {
            handle.join().unwrap();
        }
        assert_eq!(store.accounts.len(), 1);
        for model in GEMINI_MODELS {
            assert_eq!(store.wait("original", model, 1000), 2031);
        }
        for index in 0..32 {
            assert_eq!(
                store.wait(&format!("current-{index}"), GEMINI_MODELS[3], 1000),
                2031
            );
        }
        assert_eq!(store.wait("different", GEMINI_MODELS[3], 1000), 0);
    }

    #[test]
    fn strict_duplicate_email_merge_does_not_promote_unproven_restored_lock() {
        let dir = tempfile::tempdir().unwrap();
        let now = chrono::Utc::now().timestamp();
        let path = dir.path().join("model-locks");
        std::fs::create_dir(&path).unwrap();
        for (account, model, reason, days) in [
            ("old", GEMINI_MODELS[3], "Restored", 8),
            (
                "current",
                "gemini-3.7-flash-high",
                "ExactUpstreamDeadline",
                6,
            ),
        ] {
            std::fs::write(
                path.join(format!("{account}.json")),
                serde_json::to_vec(&serde_json::json!({
                    "account_id":account, "email":"same@example.test", "models": {
                        model:ModelLock {
                            model: model.into(), status:429, reason:reason.into(),
                            until:now + days * 86400, detected_at:now,
                            lock_type:"exact".into(), transient_count:3, message:String::new(),
                        }
                    }
                }))
                .unwrap(),
            )
            .unwrap();
        }
        for _ in 0..2 {
            let store = ModelLocks::open(dir.path().to_owned());
            store.bind_identity("reimported", "same@example.test");
            let restored = store.get("reimported", GEMINI_MODELS[3]).unwrap();
            assert_eq!(
                (restored.until, restored.reason.as_str()),
                (now + 8 * 86400, "Restored")
            );
            for model in &GEMINI_MODELS[..3] {
                assert_eq!(store.wait("reimported", model, now), 6 * 86400);
            }
            assert_eq!(store.wait("reimported", "gemini-3.1-flash-lite", now), 0);
        }
    }

    #[test]
    fn strict_empty_emails_do_not_merge_unknown_accounts() {
        let dir = tempfile::tempdir().unwrap();
        let now = chrono::Utc::now().timestamp();
        let path = dir.path().join("model-locks");
        std::fs::create_dir(&path).unwrap();
        for (account, email, seconds) in [("a", "", 600), ("b", "  ", 1800)] {
            std::fs::write(
                path.join(format!("{account}.json")),
                serde_json::to_vec(&serde_json::json!({
                    "account_id":account, "email":email, "models": {
                        "gemini-3.1-pro-low":ModelLock {
                            model:GEMINI_MODELS[3].into(), status:429, reason:"ExactUpstreamDeadline".into(),
                            until:now + seconds, detected_at:now, lock_type:"exact".into(),
                            transient_count:0, message:String::new(),
                        }
                    }
                }))
                .unwrap(),
            )
            .unwrap();
        }
        let store = ModelLocks::open(dir.path().to_owned());
        store.bind_identity("a", " ");
        store.bind_identity("b", "");
        assert_eq!(store.accounts.len(), 2);
        assert!(store.email_index.is_empty());
        assert_eq!(store.wait("a", GEMINI_MODELS[3], now), 600);
        assert_eq!(store.wait("b", GEMINI_MODELS[3], now), 1800);
    }

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
        assert_eq!(store.wait("new-id", "gemini-3.7-flash-high", now), 3601);
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
    fn strict_text_deadlines_preserve_complete_days_and_fractional_durations() {
        let now = 1_700_000_000;
        for (body, seconds) in [
            ("QUOTA_EXHAUSTED; retry after 6d2h", 525_600),
            ("QUOTA_EXHAUSTED; retry after 6d", 518_400),
            ("QUOTA_EXHAUSTED; retry after 1.5h", 5_400),
            ("quota will reset in 6 days 2 hours.", 525_600),
            ("Resets in 1h26m0.754378s. Please retry later.", 5_161),
            (
                r#"{"error":{"message":"Please retry after 6d2h."}}"#,
                525_600,
            ),
            ("try again in 138.127757ms", 1),
            ("(wait 2m30.1s)", 151),
        ] {
            assert_eq!(
                upstream_deadline(body, Some("1"), now),
                Some(now + seconds),
                "{body}"
            );
        }
        for body in [
            "retry after 6dgarbage2h",
            "retry after 6d2x",
            "retry after -6d2h",
        ] {
            assert_eq!(upstream_deadline(body, None, now), None, "{body}");
            assert_eq!(upstream_deadline(body, Some("61"), now), Some(now + 61));
        }
        assert_eq!(
            upstream_deadline("busy", Some("6d2h"), now),
            Some(now + 525_600)
        );
        assert_eq!(
            upstream_deadline("busy", Some("1.5h"), now),
            Some(now + 5_400)
        );
        assert_eq!(
            upstream_deadline("busy", Some("Tue, 14 Nov 2023 22:15:00 GMT"), now),
            Some(now + 100)
        );
    }

    #[test]
    fn strict_existing_saved_deadline_is_not_recalculated_by_text_parser_fix() {
        let dir = tempfile::tempdir().unwrap();
        let now = chrono::Utc::now().timestamp();
        let store = ModelLocks::open(dir.path().to_owned());
        // Even a historical deadline inconsistent with its saved message is
        // preserved: this fix changes future parsing, not persisted locks.
        let original = now + 7_200;
        store
            .record(
                "existing",
                GEMINI_MODELS[3],
                429,
                "QUOTA_EXHAUSTED; retry after 6d2h",
                Some(original),
                true,
                now,
            )
            .unwrap();
        assert_eq!(
            upstream_deadline("QUOTA_EXHAUSTED; retry after 6d2h", None, now),
            Some(now + 525_600)
        );
        for model in GEMINI_MODELS {
            assert_eq!(store.get("existing", model).unwrap().until, original);
        }
        drop(store);
        let store = ModelLocks::open(dir.path().to_owned());
        for model in GEMINI_MODELS {
            assert_eq!(store.get("existing", model).unwrap().until, original);
        }
    }

    #[test]
    fn strict_upgrade_legacy_pro_low_true_lock_keeps_longest_same_account_deadline() {
        let dir = tempfile::tempdir().unwrap();
        let now = chrono::Utc::now().timestamp();
        let locks_dir = dir.path().join("model-locks");
        std::fs::create_dir_all(&locks_dir).unwrap();
        for (account, entries) in [
            (
                "a",
                vec![
                    (GEMINI_MODELS[3], 6 * 86_400, true),
                    ("gemini-3.6-flash-high", 2 * 86_400, true),
                ],
            ),
            ("b", vec![("gemini-3.7-flash-tiered", 8 * 86_400, true)]),
            ("fake", vec![(GEMINI_MODELS[3], 600, false)]),
        ] {
            let models: HashMap<String, ModelLock> = entries
                .into_iter()
                .map(|(model, seconds, exact)| {
                    (
                        model.to_owned(),
                        ModelLock {
                            model: model.to_owned(),
                            status: 429,
                            reason: if exact {
                                "QuotaExhausted"
                            } else {
                                "TransientRateLimit"
                            }
                            .into(),
                            until: now + seconds,
                            detected_at: now,
                            lock_type: if exact { "exact" } else { "short" }.into(),
                            transient_count: if exact { 0 } else { 3 },
                            message: "legacy fixture".into(),
                        },
                    )
                })
                .collect();
            std::fs::write(
                locks_dir.join(format!("{account}.json")),
                serde_json::to_vec(&serde_json::json!({"account_id":account,"models":models}))
                    .unwrap(),
            )
            .unwrap();
        }
        let store = ModelLocks::open(dir.path().to_owned());
        for model in GEMINI_MODELS {
            assert_eq!(store.wait("a", model, now), 6 * 86_400);
            assert_eq!(store.wait("b", model, now), 8 * 86_400);
        }
        assert_eq!(store.wait("fake", GEMINI_MODELS[3], now), 600);
        for model in &GEMINI_MODELS[..3] {
            assert_eq!(store.wait("fake", model, now), 0);
        }
        drop(store);
        let store = ModelLocks::open(dir.path().to_owned());
        for model in GEMINI_MODELS {
            assert_eq!(store.wait("a", model, now), 6 * 86_400);
            assert_eq!(store.wait("b", model, now), 8 * 86_400);
        }
        assert_eq!(
            store.get("fake", GEMINI_MODELS[3]).unwrap().transient_count,
            3
        );
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
