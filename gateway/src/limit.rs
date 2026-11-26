//! Per-client rate limits: one token bucket per client key, in memory.
//!
//! Nothing here is written to disk or logged. A client key is an IPv4
//! address or an IPv6 /64: one household or one host gets one bucket,
//! whichever of its addresses it uses.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv6Addr};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::config::Bucket;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ClientKey {
    V4([u8; 4]),
    /// The first 64 bits of an IPv6 address.
    V6Prefix([u8; 8]),
}

impl From<IpAddr> for ClientKey {
    fn from(addr: IpAddr) -> Self {
        fn v6(addr: Ipv6Addr) -> ClientKey {
            match addr.to_ipv4_mapped() {
                Some(v4) => ClientKey::V4(v4.octets()),
                None => {
                    let mut prefix = [0u8; 8];
                    prefix.copy_from_slice(&addr.octets()[..8]);
                    ClientKey::V6Prefix(prefix)
                }
            }
        }
        match addr {
            IpAddr::V4(v4) => ClientKey::V4(v4.octets()),
            IpAddr::V6(addr) => v6(addr),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Allowed,
    /// Refused; a token is back after this long.
    Limited {
        retry_after: Duration,
    },
}

#[derive(Debug)]
struct State {
    tokens: f64,
    updated: Instant,
}

#[derive(Debug)]
pub struct RateLimiter {
    bucket: Bucket,
    idle: Duration,
    max_clients: usize,
    clients: Mutex<HashMap<ClientKey, State>>,
}

impl RateLimiter {
    pub fn new(bucket: Bucket, idle: Duration, max_clients: usize) -> Self {
        Self {
            bucket,
            idle,
            max_clients,
            clients: Mutex::new(HashMap::new()),
        }
    }

    fn refill_per_sec(&self) -> f64 {
        f64::from(self.bucket.per_minute) / 60.0
    }

    /// Charge one request to `key` at `now`.
    pub fn check_at(&self, key: ClientKey, now: Instant) -> Decision {
        let burst = f64::from(self.bucket.burst);
        let rate = self.refill_per_sec();
        let mut clients = self
            .clients
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !clients.contains_key(&key) && clients.len() >= self.max_clients {
            // The table is full. Drop idle buckets; if none are idle, the
            // new client waits rather than the table growing.
            let idle = self.idle;
            clients.retain(|_, s| now.saturating_duration_since(s.updated) < idle);
            if clients.len() >= self.max_clients {
                return Decision::Limited { retry_after: idle };
            }
        }
        let state = clients.entry(key).or_insert(State {
            tokens: burst,
            updated: now,
        });
        let elapsed = now.saturating_duration_since(state.updated).as_secs_f64();
        state.tokens = (state.tokens + elapsed * rate).min(burst);
        state.updated = now;
        if state.tokens >= 1.0 {
            state.tokens -= 1.0;
            Decision::Allowed
        } else {
            let wait = (1.0 - state.tokens) / rate;
            Decision::Limited {
                retry_after: Duration::from_secs_f64(wait),
            }
        }
    }

    pub fn check(&self, key: ClientKey) -> Decision {
        self.check_at(key, Instant::now())
    }

    /// Drop buckets idle past the limit. Called on a timer.
    pub fn sweep_at(&self, now: Instant) {
        let idle = self.idle;
        self.clients
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|_, s| now.saturating_duration_since(s.updated) < idle);
    }

    pub fn tracked(&self) -> usize {
        self.clients
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limiter(per_minute: u32, burst: u32, max_clients: usize) -> RateLimiter {
        RateLimiter::new(
            Bucket { per_minute, burst },
            Duration::from_secs(600),
            max_clients,
        )
    }
    fn key(n: u8) -> ClientKey {
        ClientKey::V4([192, 0, 2, n])
    }

    #[test]
    fn burst_then_refused_then_refilled() {
        let l = limiter(60, 3, 10);
        let t0 = Instant::now();
        for _ in 0..3 {
            assert_eq!(l.check_at(key(1), t0), Decision::Allowed);
        }
        let Decision::Limited { retry_after } = l.check_at(key(1), t0) else {
            panic!("fourth request in a burst of 3 must be refused");
        };
        // 60 a minute is one a second.
        assert!(retry_after > Duration::from_millis(900) && retry_after <= Duration::from_secs(1));
        assert_eq!(
            l.check_at(key(1), t0 + Duration::from_secs(1)),
            Decision::Allowed
        );
        // Another client is not charged for the first one's requests.
        assert_eq!(l.check_at(key(2), t0), Decision::Allowed);
    }

    #[test]
    fn refill_stops_at_the_burst() {
        let l = limiter(60, 2, 10);
        let t0 = Instant::now();
        assert_eq!(l.check_at(key(1), t0), Decision::Allowed);
        let later = t0 + Duration::from_secs(3600);
        assert_eq!(l.check_at(key(1), later), Decision::Allowed);
        assert_eq!(l.check_at(key(1), later), Decision::Allowed);
        assert!(matches!(
            l.check_at(key(1), later),
            Decision::Limited { .. }
        ));
    }

    #[test]
    fn a_full_table_refuses_new_clients_until_one_is_idle() {
        let l = limiter(60, 5, 2);
        let t0 = Instant::now();
        assert_eq!(l.check_at(key(1), t0), Decision::Allowed);
        assert_eq!(l.check_at(key(2), t0), Decision::Allowed);
        assert!(matches!(l.check_at(key(3), t0), Decision::Limited { .. }));
        assert_eq!(l.tracked(), 2);
        // Known clients are still served.
        assert_eq!(l.check_at(key(1), t0), Decision::Allowed);
        // Once the others have been idle long enough, a new client gets in.
        let later = t0 + Duration::from_secs(601);
        assert_eq!(l.check_at(key(3), later), Decision::Allowed);
        assert_eq!(l.tracked(), 1);
    }

    #[test]
    fn sweep_drops_idle_buckets() {
        let l = limiter(60, 5, 10);
        let t0 = Instant::now();
        l.check_at(key(1), t0);
        l.check_at(key(2), t0 + Duration::from_secs(500));
        l.sweep_at(t0 + Duration::from_secs(700));
        assert_eq!(l.tracked(), 1);
    }

    #[test]
    fn ipv6_is_keyed_by_its_slash_64_and_mapped_ipv4_as_ipv4() {
        let a: IpAddr = "2001:db8:1:2::1".parse().unwrap();
        let b: IpAddr = "2001:db8:1:2:ffff:ffff:ffff:ffff".parse().unwrap();
        let c: IpAddr = "2001:db8:1:3::1".parse().unwrap();
        assert_eq!(ClientKey::from(a), ClientKey::from(b));
        assert_ne!(ClientKey::from(a), ClientKey::from(c));
        let mapped: IpAddr = "::ffff:192.0.2.9".parse().unwrap();
        assert_eq!(ClientKey::from(mapped), ClientKey::V4([192, 0, 2, 9]));
    }
}
