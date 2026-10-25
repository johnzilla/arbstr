//! Per-client rate limiting (`[server] client_rate_limit_rps`).
//!
//! Each client gets its own token bucket holding up to one second of requests
//! (`rps`), refilled continuously at `rps` per second. A client is its bearer
//! token when the request carries one, so agents behind one host or NAT are
//! limited separately, and otherwise the connecting IP. `X-Forwarded-For` is
//! not trusted: behind a reverse proxy, unauthenticated requests share the
//! proxy's bucket.
//!
//! This sits alongside the global `rate_limit_rps`, which still caps the total.

use std::collections::hash_map::RandomState;
use std::hash::BuildHasher;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

use axum::extract::ConnectInfo;
use axum::http::{header, HeaderValue, Request, StatusCode};
use axum::response::Response;
use dashmap::DashMap;

/// Buckets idle this long are full again and can be dropped.
const IDLE_EVICT: Duration = Duration::from_secs(60);
/// Prune idle buckets once the map grows past this many clients.
const PRUNE_ABOVE: usize = 10_000;

struct Bucket {
    tokens: f64,
    last: Instant,
}

pub struct ClientRateLimiter {
    rps: f64,
    buckets: DashMap<u64, Bucket>,
    // Keys are hashed (randomly seeded) so raw tokens are not kept in the map.
    hasher: RandomState,
}

impl ClientRateLimiter {
    pub fn new(rps: u64) -> Self {
        Self {
            rps: rps as f64,
            buckets: DashMap::new(),
            hasher: RandomState::new(),
        }
    }

    /// Take one token from `client`'s bucket. Returns false when it is empty.
    pub fn check(&self, client: &str, now: Instant) -> bool {
        if self.buckets.len() > PRUNE_ABOVE {
            self.buckets
                .retain(|_, b| now.duration_since(b.last) < IDLE_EVICT);
        }
        let key = self.hasher.hash_one(client);
        let mut bucket = self.buckets.entry(key).or_insert(Bucket {
            tokens: self.rps,
            last: now,
        });
        let elapsed = now.duration_since(bucket.last).as_secs_f64();
        bucket.tokens = (bucket.tokens + elapsed * self.rps).min(self.rps);
        bucket.last = now;
        if bucket.tokens >= 1.0 {
            bucket.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

/// The client a request is counted against: its bearer token, else its peer IP.
pub fn client_key<B>(request: &Request<B>) -> String {
    if let Some(token) = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
    {
        return format!("token:{token}");
    }
    match request.extensions().get::<ConnectInfo<SocketAddr>>() {
        Some(ConnectInfo(addr)) => format!("ip:{}", addr.ip()),
        None => "ip:unknown".to_string(),
    }
}

pub fn too_many_requests() -> Response {
    let mut response = Response::new(axum::body::Body::from(
        serde_json::to_vec(&serde_json::json!({
            "error": {
                "message": "Rate limit exceeded for this client; retry shortly",
                "type": "rate_limit_error",
                "code": "client_rate_limited"
            }
        }))
        .unwrap(),
    ));
    *response.status_mut() = StatusCode::TOO_MANY_REQUESTS;
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    headers.insert(header::RETRY_AFTER, HeaderValue::from_static("1"));
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_a_burst_of_rps_then_refuses() {
        let limiter = ClientRateLimiter::new(3);
        let now = Instant::now();
        assert!(limiter.check("a", now));
        assert!(limiter.check("a", now));
        assert!(limiter.check("a", now));
        assert!(!limiter.check("a", now));
    }

    #[test]
    fn clients_have_separate_buckets() {
        let limiter = ClientRateLimiter::new(1);
        let now = Instant::now();
        assert!(limiter.check("a", now));
        assert!(!limiter.check("a", now));
        assert!(limiter.check("b", now));
    }

    #[test]
    fn refills_over_time_up_to_rps() {
        let limiter = ClientRateLimiter::new(2);
        let now = Instant::now();
        assert!(limiter.check("a", now));
        assert!(limiter.check("a", now));
        assert!(!limiter.check("a", now));
        // Half a second at 2 rps refills one token
        let later = now + Duration::from_millis(500);
        assert!(limiter.check("a", later));
        assert!(!limiter.check("a", later));
        // A long idle period refills only to the burst size
        let much_later = later + Duration::from_secs(30);
        assert!(limiter.check("a", much_later));
        assert!(limiter.check("a", much_later));
        assert!(!limiter.check("a", much_later));
    }

    #[test]
    fn client_key_prefers_bearer_token() {
        let with_token = Request::builder()
            .header("authorization", "Bearer vtk_abc")
            .body(())
            .unwrap();
        assert_eq!(client_key(&with_token), "token:vtk_abc");

        let mut anonymous = Request::builder().body(()).unwrap();
        assert_eq!(client_key(&anonymous), "ip:unknown");
        anonymous
            .extensions_mut()
            .insert(ConnectInfo(SocketAddr::from(([10, 0, 0, 7], 5555))));
        assert_eq!(client_key(&anonymous), "ip:10.0.0.7");
    }
}
