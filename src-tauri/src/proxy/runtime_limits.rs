//! Shared desktop/Web resource controls. Permits live until the response body ends.
use axum::{
    body::Body,
    extract::Request,
    http::{header, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use futures::{FutureExt, StreamExt};
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use std::{
    collections::VecDeque,
    sync::atomic::{AtomicU64, AtomicUsize, Ordering},
    time::{Duration, Instant},
};

#[derive(Default)]
struct Window {
    completed: VecDeque<Instant>,
    reserved: usize,
}
pub struct RuntimeLimits {
    successes: AtomicUsize,
    concurrency: AtomicUsize,
    active: AtomicUsize,
    memory_limit: AtomicU64,
    memory_used: AtomicU64,
    window: Mutex<Window>,
}
pub static LIMITS: Lazy<RuntimeLimits> = Lazy::new(|| RuntimeLimits {
    successes: AtomicUsize::new(1000),
    concurrency: AtomicUsize::new(800),
    active: AtomicUsize::new(0),
    memory_limit: AtomicU64::new(0),
    memory_used: AtomicU64::new(0),
    window: Mutex::new(Window::default()),
});
impl RuntimeLimits {
    pub fn update(&self, config: &crate::proxy::ProxyConfig) {
        self.successes.store(
            config.max_successes_per_minute.max(1) as usize,
            Ordering::Release,
        );
        self.concurrency.store(
            config.max_concurrent_requests.max(1) as usize,
            Ordering::Release,
        );
        self.memory_limit.store(
            if config.max_memory_gb.is_finite() && config.max_memory_gb > 0.0 {
                (config.max_memory_gb * 1073741824.0) as u64
            } else {
                0
            },
            Ordering::Release,
        );
    }
    pub fn info(&self) -> serde_json::Value {
        let used = self.memory_used.load(Ordering::Acquire);
        let limit = self.memory_limit.load(Ordering::Acquire);
        serde_json::json!({"current_gb":used as f64/1073741824.0,"limit_gb":limit as f64/1073741824.0,
            "in_flight":self.active.load(Ordering::Acquire),"max_concurrent_requests":self.concurrency.load(Ordering::Acquire),
            "max_successes_per_minute":self.successes.load(Ordering::Acquire),"memory_pressure":limit>0 && used>=limit})
    }
    pub(crate) fn reserve(&'static self) -> Result<Permit, &'static str> {
        let limit = self.memory_limit.load(Ordering::Acquire);
        if limit > 0 && self.memory_used.load(Ordering::Acquire) >= limit {
            return Err("Gateway memory limit reached");
        }
        let mut window = self.window.lock();
        while window
            .completed
            .front()
            .is_some_and(|t| t.elapsed() >= Duration::from_secs(60))
        {
            window.completed.pop_front();
        }
        if window.completed.len() + window.reserved >= self.successes.load(Ordering::Acquire) {
            return Err("Global success limit reached (1 minute window)");
        }
        // Serialized with reservations; lowering the limit immediately affects new requests.
        if self.active.load(Ordering::Acquire) >= self.concurrency.load(Ordering::Acquire) {
            return Err("Too many concurrent requests");
        }
        window.reserved += 1;
        self.active.fetch_add(1, Ordering::AcqRel);
        Ok(Permit {
            owner: self,
            successful: false,
        })
    }
}
pub(crate) struct Permit {
    owner: &'static RuntimeLimits,
    successful: bool,
}
impl Drop for Permit {
    fn drop(&mut self) {
        let mut window = self.owner.window.lock();
        window.reserved = window.reserved.saturating_sub(1);
        if self.successful {
            window.completed.push_back(Instant::now());
        }
        self.owner.active.fetch_sub(1, Ordering::AcqRel);
    }
}
pub async fn sample_memory(cancel: tokio_util::sync::CancellationToken) {
    // One sampler per admin server; no signature/cache invalidation is performed.
    loop {
        let value = tokio::task::spawn_blocking(|| {
            let mut system = sysinfo::System::new();
            let pid = sysinfo::Pid::from_u32(std::process::id());
            system.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[pid]));
            system.process(pid).map(|p| p.memory()).unwrap_or(0)
        })
        .await
        .unwrap_or(0);
        LIMITS.memory_used.store(value, Ordering::Release);
        tokio::select! { _ = cancel.cancelled() => break, _ = tokio::time::sleep(Duration::from_secs(2)) => {} }
    }
}
pub async fn middleware(request: Request, next: Next) -> Response {
    let path = request.uri().path();
    let generation = request.method() == axum::http::Method::POST
        && (matches!(
            path,
            "/v1/messages"
                | "/v1/chat/completions"
                | "/v1/responses"
                | "/v1/completions"
                | "/responses"
                | "/responses/compact"
                | "/v1/audio/transcriptions"
                | "/internal/warmup"
                | "/v1/images/generations"
                | "/v1/images/edits"
        ) || (path.starts_with("/v1beta/models/")
            && (path.contains(":generateContent") || path.contains(":streamGenerateContent"))));
    if !generation {
        return next.run(request).await;
    }
    let permit = match LIMITS.reserve() {
        Ok(permit) => permit,
        Err(message) => {
            return (
                StatusCode::TOO_MANY_REQUESTS,
                [(header::RETRY_AFTER, "1")],
                Json(serde_json::json!({"error":{"type":"rate_limit_error","message":message}})),
            )
                .into_response()
        }
    };
    let response = match std::panic::AssertUnwindSafe(next.run(request)).catch_unwind().await {
        Ok(response) => response,
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error":{"type":"internal_error","message":"Gateway request failed; see server diagnostics"}}))).into_response(),
    };
    wrap_response(response, permit)
}

pub(crate) fn wrap_response(response: Response, mut permit: Permit) -> Response {
    let success = response.status().is_success();
    let sse = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("text/event-stream"));
    let (parts, body) = response.into_parts();
    let stream = async_stream::stream! {
        let mut stream = body.into_data_stream();
        let mut failed = !success;
        let mut tail = String::new();
        loop {
            match std::panic::AssertUnwindSafe(stream.next()).catch_unwind().await {
                Ok(Some(Ok(bytes))) => {
                    if sse {
                        tail.push_str(&String::from_utf8_lossy(&bytes));
                        while let Some(end) = tail.find('\n') {
                            let line = tail[..end].trim_end_matches('\r');
                            if line.strip_prefix("event:").is_some_and(|v| v.trim() == "error") { failed = true; }
                            if let Some(data) = line.strip_prefix("data:") {
                                if let Ok(value) = serde_json::from_str::<serde_json::Value>(data.trim()) {
                                    if value.get("error").is_some() || value.get("type").and_then(|v| v.as_str()) == Some("error") { failed = true; }
                                }
                            }
                            tail.drain(..=end);
                        }
                        // Bound malformed/unframed SSE memory without accepting it as success.
                        if tail.len() > 1024 * 1024 { failed = true; tail.clear(); }
                    }
                    yield Ok::<_, std::io::Error>(bytes);
                }
                Ok(Some(Err(error))) => { failed = true; yield Err(std::io::Error::other(error.to_string())); break; }
                Ok(None) => break,
                Err(_) => { failed = true; yield Err(std::io::Error::other("Gateway stream panic")); break; }
            }
        }
        permit.successful = !failed;
        drop(permit);
    };
    Response::from_parts(parts, Body::from_stream(stream))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn limits(successes: usize, concurrency: usize) -> &'static RuntimeLimits {
        Box::leak(Box::new(RuntimeLimits {
            successes: AtomicUsize::new(successes),
            concurrency: AtomicUsize::new(concurrency),
            active: AtomicUsize::new(0),
            memory_limit: AtomicU64::new(0),
            memory_used: AtomicU64::new(0),
            window: Mutex::new(Window::default()),
        }))
    }
    #[tokio::test]
    async fn stream_holds_permit_and_cancel_does_not_count_as_success() {
        let state = limits(2, 1);
        let pending = futures::stream::pending::<Result<axum::body::Bytes, std::io::Error>>();
        let response = wrap_response(
            Response::new(Body::from_stream(pending)),
            state.reserve().unwrap(),
        );
        assert!(state.reserve().is_err());
        drop(response);
        assert_eq!(state.active.load(Ordering::Acquire), 0);
        assert!(state.window.lock().completed.is_empty());
        let response = wrap_response(Response::new(Body::from("ok")), state.reserve().unwrap());
        axum::body::to_bytes(response.into_body(), 100)
            .await
            .unwrap();
        assert_eq!(state.window.lock().completed.len(), 1);
        let permit = state.reserve().unwrap();
        assert!(state.reserve().is_err());
        drop(permit);
        assert_eq!(state.window.lock().completed.len(), 1);
    }
    #[tokio::test]
    async fn error_sse_and_error_body_release_capacity() {
        let state = limits(1, 1);
        let response = Response::builder()
            .header(header::CONTENT_TYPE, "text/event-stream")
            .body(Body::from("event: error\ndata: {\"type\":\"error\"}\n\n"))
            .unwrap();
        axum::body::to_bytes(
            wrap_response(response, state.reserve().unwrap()).into_body(),
            1000,
        )
        .await
        .unwrap();
        assert!(state.window.lock().completed.is_empty());
        let broken = futures::stream::once(async {
            Err::<axum::body::Bytes, _>(std::io::Error::other("broken"))
        });
        let response = wrap_response(
            Response::new(Body::from_stream(broken)),
            state.reserve().unwrap(),
        );
        assert!(axum::body::to_bytes(response.into_body(), 100)
            .await
            .is_err());
        assert_eq!(state.active.load(Ordering::Acquire), 0);
        assert!(state.window.lock().completed.is_empty());
    }
    #[test]
    fn lowering_limits_memory_pressure_and_window_expiry_apply_immediately() {
        let state = limits(5, 5);
        let a = state.reserve().unwrap();
        let b = state.reserve().unwrap();
        state.concurrency.store(1, Ordering::Release);
        drop(a);
        assert!(state.reserve().is_err());
        drop(b);
        state.memory_limit.store(10, Ordering::Release);
        state.memory_used.store(10, Ordering::Release);
        assert!(state.reserve().is_err());
        state.memory_used.store(0, Ordering::Release);
        state.successes.store(1, Ordering::Release);
        state
            .window
            .lock()
            .completed
            .push_back(Instant::now() - Duration::from_secs(61));
        assert!(state.reserve().is_ok());
    }
}
