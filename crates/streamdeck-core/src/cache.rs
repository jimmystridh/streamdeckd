//! Cache lifetime and stale-data policy.
//!
//! The decision of whether to serve a cached value, refetch, or keep showing
//! stale data lives here so it can be tested without a network client. The async
//! single-flight wrapper in the daemon uses `CachePolicy` to decide what to do.

/// What a caller should do with a cache slot right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheDecision {
    /// The cached value is inside its lifetime; serve it and do not fetch.
    Fresh,
    /// The value has expired; fetch and serve the result.
    Refresh,
    /// Nothing is cached; fetch and show a loading state until it lands.
    Empty,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CachePolicy {
    /// How long a successful value stays fresh.
    pub ttl_ms: u64,
    /// How long to wait before retrying after a failure while stale data is shown.
    pub error_retry_ms: u64,
}

impl CachePolicy {
    pub const fn new(ttl_ms: u64, error_retry_ms: u64) -> Self {
        Self {
            ttl_ms,
            error_retry_ms,
        }
    }
}

/// A cached value with enough history to render stale and error treatments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cached<T> {
    value: Option<T>,
    policy: CachePolicy,
    fetched_at_ms: Option<u64>,
    expires_at_ms: u64,
    /// Set when the most recent fetch failed but a previous value is still shown.
    stale: bool,
    last_error: Option<String>,
    successes: u64,
    failures: u64,
}

impl<T> Cached<T> {
    pub fn new(policy: CachePolicy) -> Self {
        Self {
            value: None,
            policy,
            fetched_at_ms: None,
            expires_at_ms: 0,
            stale: false,
            last_error: None,
            successes: 0,
            failures: 0,
        }
    }

    pub fn policy(&self) -> CachePolicy {
        self.policy
    }

    pub fn set_policy(&mut self, policy: CachePolicy) {
        self.policy = policy;
    }

    pub fn decide(&self, now_ms: u64) -> CacheDecision {
        match self.value {
            None => CacheDecision::Empty,
            Some(_) if now_ms >= self.expires_at_ms => CacheDecision::Refresh,
            Some(_) => CacheDecision::Fresh,
        }
    }

    pub fn needs_fetch(&self, now_ms: u64) -> bool {
        !matches!(self.decide(now_ms), CacheDecision::Fresh)
    }

    /// The value, whether fresh or stale. Callers pair this with [`Self::is_stale`].
    pub fn peek(&self) -> Option<&T> {
        self.value.as_ref()
    }

    pub fn is_stale(&self) -> bool {
        self.stale
    }

    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    pub fn age_ms(&self, now_ms: u64) -> Option<u64> {
        self.fetched_at_ms
            .map(|fetched| now_ms.saturating_sub(fetched))
    }

    pub fn last_success_ms(&self) -> Option<u64> {
        self.fetched_at_ms
    }

    pub fn expires_at_ms(&self) -> Option<u64> {
        self.value.as_ref().map(|_| self.expires_at_ms)
    }

    pub fn totals(&self) -> (u64, u64) {
        (self.successes, self.failures)
    }

    pub fn store(&mut self, value: T, now_ms: u64) {
        self.store_until(value, now_ms, now_ms + self.policy.ttl_ms);
    }

    /// Stores a value with an explicit expiry, so an upstream `Expires` header can
    /// override the local TTL.
    pub fn store_until(&mut self, value: T, now_ms: u64, expires_at_ms: u64) {
        self.value = Some(value);
        self.fetched_at_ms = Some(now_ms);
        self.expires_at_ms = expires_at_ms.max(now_ms);
        self.stale = false;
        self.last_error = None;
        self.successes += 1;
    }

    /// Marks a `304 Not Modified` response: the value is unchanged and fresh again.
    pub fn revalidate(&mut self, now_ms: u64, expires_at_ms: u64) {
        if self.value.is_some() {
            self.expires_at_ms = expires_at_ms.max(now_ms);
            self.stale = false;
            self.last_error = None;
            self.successes += 1;
        }
    }

    /// Records a failure. An existing value is kept and marked stale; the retry is
    /// deferred by `error_retry_ms` so a broken endpoint is not hammered.
    pub fn fail(&mut self, error: impl Into<String>, now_ms: u64) {
        self.failures += 1;
        self.last_error = Some(error.into());
        if self.value.is_some() {
            self.stale = true;
            self.expires_at_ms = now_ms + self.policy.error_retry_ms;
        }
    }

    /// Forces the next `decide` to report a refresh, for a manual refresh press.
    pub fn invalidate(&mut self) {
        self.expires_at_ms = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const POLICY: CachePolicy = CachePolicy::new(5 * 60_000, 5 * 60_000);

    #[test]
    fn an_empty_cache_asks_for_a_fetch_and_has_no_value() {
        let cache: Cached<u32> = Cached::new(POLICY);
        assert_eq!(cache.decide(0), CacheDecision::Empty);
        assert!(cache.needs_fetch(0));
        assert_eq!(cache.peek(), None);
        assert_eq!(cache.age_ms(1_000), None);
    }

    #[test]
    fn a_fresh_value_is_served_without_fetching() {
        let mut cache = Cached::new(POLICY);
        cache.store(7u32, 1_000);

        assert_eq!(cache.decide(1_000), CacheDecision::Fresh);
        assert_eq!(cache.decide(300_999), CacheDecision::Fresh);
        assert!(!cache.needs_fetch(200_000));
        assert_eq!(cache.peek(), Some(&7));
        assert!(!cache.is_stale());
        assert_eq!(cache.age_ms(61_000), Some(60_000));
    }

    #[test]
    fn an_expired_value_asks_for_a_refresh_but_is_still_readable() {
        let mut cache = Cached::new(POLICY);
        cache.store(7u32, 1_000);

        assert_eq!(cache.decide(301_000), CacheDecision::Refresh);
        assert_eq!(
            cache.peek(),
            Some(&7),
            "cold start still has something to draw"
        );
    }

    #[test]
    fn a_failure_keeps_the_previous_value_and_marks_it_stale() {
        let mut cache = Cached::new(POLICY);
        cache.store(7u32, 1_000);
        cache.fail("timeout", 301_000);

        assert_eq!(cache.peek(), Some(&7));
        assert!(cache.is_stale());
        assert_eq!(cache.last_error(), Some("timeout"));
        assert_eq!(
            cache.decide(301_000),
            CacheDecision::Fresh,
            "retry is deferred"
        );
        assert_eq!(cache.decide(601_000), CacheDecision::Refresh);
    }

    #[test]
    fn a_failure_with_nothing_cached_still_reports_empty() {
        let mut cache: Cached<u32> = Cached::new(POLICY);
        cache.fail("no network", 1_000);

        assert_eq!(cache.decide(1_000), CacheDecision::Empty);
        assert_eq!(cache.last_error(), Some("no network"));
        assert!(!cache.is_stale(), "there is no stale value to show");
    }

    #[test]
    fn a_success_clears_a_previous_stale_marker() {
        let mut cache = Cached::new(POLICY);
        cache.store(7u32, 1_000);
        cache.fail("timeout", 301_000);
        cache.store(9u32, 601_000);

        assert!(!cache.is_stale());
        assert_eq!(cache.last_error(), None);
        assert_eq!(cache.peek(), Some(&9));
        assert_eq!(cache.totals(), (2, 1));
    }

    #[test]
    fn an_upstream_expiry_overrides_the_local_ttl() {
        let mut cache = Cached::new(POLICY);
        cache.store_until(7u32, 1_000, 1_800_000);

        assert_eq!(cache.decide(1_000_000), CacheDecision::Fresh);
        assert_eq!(cache.decide(1_800_000), CacheDecision::Refresh);
    }

    #[test]
    fn an_expiry_in_the_past_does_not_produce_a_negative_lifetime() {
        let mut cache = Cached::new(POLICY);
        cache.store_until(7u32, 5_000, 1_000);
        assert_eq!(cache.expires_at_ms(), Some(5_000));
    }

    #[test]
    fn revalidation_refreshes_an_unchanged_value() {
        let mut cache = Cached::new(POLICY);
        cache.store(7u32, 1_000);
        cache.fail("timeout", 301_000);
        cache.revalidate(601_000, 901_000);

        assert!(!cache.is_stale());
        assert_eq!(cache.peek(), Some(&7));
        assert_eq!(cache.decide(900_999), CacheDecision::Fresh);
    }

    #[test]
    fn revalidating_an_empty_cache_does_nothing() {
        let mut cache: Cached<u32> = Cached::new(POLICY);
        cache.revalidate(1_000, 900_000);
        assert_eq!(cache.decide(1_000), CacheDecision::Empty);
    }

    #[test]
    fn invalidate_forces_the_next_press_to_refetch() {
        let mut cache = Cached::new(POLICY);
        cache.store(7u32, 1_000);
        cache.invalidate();

        assert_eq!(cache.decide(1_000), CacheDecision::Refresh);
        assert_eq!(cache.peek(), Some(&7));
    }
}
