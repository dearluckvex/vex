use std::net::IpAddr;
use std::num::NonZeroUsize;
use std::sync::{Arc, RwLock};

use lru::LruCache;

use super::geoip::GeoIpDb;
use crate::config::model::RoutingRule;

/// Maximum number of cached route decisions.
const ROUTE_CACHE_CAP: usize = 4096;

/// Action to take for a matched connection.
#[derive(Debug, Clone, PartialEq)]
pub enum RouteAction {
    /// Use the configured proxy outbound.
    Proxy,
    /// Connect directly, bypassing proxy.
    Direct,
    /// Drop/reject the connection.
    Reject,
}

/// A compiled match rule for fast evaluation.
#[derive(Debug, Clone)]
pub enum MatchRule {
    /// Exact domain match: "example.com"
    Domain(String),
    /// Domain suffix match: ".example.com" matches "foo.example.com"
    DomainSuffix(String),
    /// Domain keyword match: contains "google"
    DomainKeyword(String),
    /// IPv4/IPv6 CIDR match
    IpCidr { addr: IpAddr, prefix_len: u8 },
    /// GeoIP country code (e.g., "CN", "US")
    GeoIp(String),
    /// Match all (catch-all rule)
    MatchAll,
}

/// A single rule with its action.
#[derive(Debug, Clone)]
pub struct RuleEntry {
    pub rule: MatchRule,
    pub action: RouteAction,
}

/// A set of routing rules, compiled for efficient matching.
#[derive(Debug, Clone)]
pub struct RuleSet {
    rules: Vec<RuleEntry>,
    default_action: RouteAction,
}

impl RuleSet {
    pub fn new() -> Self {
        Self {
            rules: Vec::new(),
            default_action: RouteAction::Proxy,
        }
    }

    /// Set the default action when no rule matches.
    pub fn set_default(&mut self, action: RouteAction) {
        self.default_action = action;
    }

    /// Add a rule to the set.
    pub fn add_rule(&mut self, rule: MatchRule, action: RouteAction) {
        self.rules.push(RuleEntry { rule, action });
    }

    /// Build from config RoutingRules.
    pub fn from_config(rules: &[RoutingRule]) -> Self {
        let mut set = Self::new();
        for r in rules {
            let action = match r.target.to_lowercase().as_str() {
                "direct" => RouteAction::Direct,
                "reject" | "block" => RouteAction::Reject,
                _ => RouteAction::Proxy,
            };

            let rule = match r.rule_type.to_lowercase().as_str() {
                "domain" => MatchRule::Domain(r.pattern.to_lowercase()),
                "domain-suffix" => {
                    let suffix = r.pattern.to_lowercase();
                    MatchRule::DomainSuffix(if suffix.starts_with('.') {
                        suffix
                    } else {
                        format!(".{}", suffix)
                    })
                }
                "domain-keyword" => MatchRule::DomainKeyword(r.pattern.to_lowercase()),
                "ip-cidr" => match parse_cidr(&r.pattern) {
                    Some((addr, prefix_len)) => MatchRule::IpCidr { addr, prefix_len },
                    None => {
                        tracing::warn!("Invalid CIDR: {}", r.pattern);
                        continue;
                    }
                },
                "geoip" => MatchRule::GeoIp(r.pattern.to_uppercase()),
                "match" | "final" => MatchRule::MatchAll,
                _ => {
                    tracing::warn!("Unknown rule type: {}", r.rule_type);
                    continue;
                }
            };

            set.add_rule(rule, action);
        }
        set
    }

    pub fn rules(&self) -> &[RuleEntry] {
        &self.rules
    }
}

impl Default for RuleSet {
    fn default() -> Self {
        Self::new()
    }
}

/// Router evaluates rules against connection targets.
pub struct Router {
    rules: RuleSet,
    geoip: Option<Arc<GeoIpDb>>,
    /// LRU route cache: evicts least-recently-used entries when full (avoids thundering-herd on full flush).
    cache: RwLock<LruCache<String, RouteAction>>,
}

impl Router {
    pub fn new(rules: RuleSet) -> Self {
        Self {
            rules,
            geoip: None,
            cache: RwLock::new(LruCache::new(NonZeroUsize::new(ROUTE_CACHE_CAP).unwrap())),
        }
    }

    pub fn with_geoip(mut self, geoip: Arc<GeoIpDb>) -> Self {
        self.geoip = Some(geoip);
        self
    }

    /// Evaluate routing rules for a given destination.
    /// `host` can be a domain name or IP address string.
    pub fn route(&self, host: &str, port: u16) -> RouteAction {
        // Fast path: check cache using peek() — avoids requiring &mut self under read lock
        if let Ok(cache) = self.cache.read() {
            if let Some(action) = cache.peek(host) {
                return action.clone();
            }
        }

        let action = self.route_uncached(host, port);

        // Store in cache — LruCache::put auto-evicts the LRU entry when full
        if let Ok(mut cache) = self.cache.write() {
            cache.put(host.to_string(), action.clone());
        }

        action
    }

    /// Perform rule matching without cache.
    fn route_uncached(&self, host: &str, port: u16) -> RouteAction {
        let host_lower = host.to_lowercase();
        let ip: Option<IpAddr> = host.parse().ok();

        for entry in &self.rules.rules {
            let matched = match &entry.rule {
                MatchRule::Domain(domain) => host_lower == *domain,

                MatchRule::DomainSuffix(suffix) => {
                    host_lower.ends_with(suffix.as_str())
                        || host_lower == suffix.trim_start_matches('.')
                }

                MatchRule::DomainKeyword(keyword) => host_lower.contains(keyword.as_str()),

                MatchRule::IpCidr { addr, prefix_len } => {
                    ip.map_or(false, |ip| cidr_match(ip, *addr, *prefix_len))
                }

                MatchRule::GeoIp(country) => {
                    if let (Some(ip), Some(geoip)) = (&ip, &self.geoip) {
                        geoip
                            .lookup(*ip)
                            .map_or(false, |cc| cc.eq_ignore_ascii_case(country))
                    } else {
                        false
                    }
                }

                MatchRule::MatchAll => true,
            };

            if matched {
                tracing::debug!(
                    "Route match: {}:{} -> {:?} (rule: {:?})",
                    host,
                    port,
                    entry.action,
                    entry.rule
                );
                return entry.action.clone();
            }
        }

        self.rules.default_action.clone()
    }
}

/// Parse a CIDR string like "192.168.0.0/16" or "::1/128".
fn parse_cidr(cidr: &str) -> Option<(IpAddr, u8)> {
    let parts: Vec<&str> = cidr.splitn(2, '/').collect();
    if parts.len() != 2 {
        return None;
    }
    let addr: IpAddr = parts[0].parse().ok()?;
    let prefix_len: u8 = parts[1].parse().ok()?;

    match addr {
        IpAddr::V4(_) if prefix_len > 32 => return None,
        IpAddr::V6(_) if prefix_len > 128 => return None,
        _ => {}
    }

    Some((addr, prefix_len))
}

/// Check if an IP matches a CIDR.
fn cidr_match(ip: IpAddr, network: IpAddr, prefix_len: u8) -> bool {
    match (ip, network) {
        (IpAddr::V4(ip), IpAddr::V4(net)) => {
            if prefix_len == 0 {
                return true;
            }
            let ip_bits = u32::from(ip);
            let net_bits = u32::from(net);
            let mask = u32::MAX.checked_shl(32 - prefix_len as u32).unwrap_or(0);
            (ip_bits & mask) == (net_bits & mask)
        }
        (IpAddr::V6(ip), IpAddr::V6(net)) => {
            if prefix_len == 0 {
                return true;
            }
            let ip_bits = u128::from(ip);
            let net_bits = u128::from(net);
            let mask = u128::MAX.checked_shl(128 - prefix_len as u32).unwrap_or(0);
            (ip_bits & mask) == (net_bits & mask)
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_route_domain_exact() {
        let mut rules = RuleSet::new();
        rules.add_rule(MatchRule::Domain("example.com".into()), RouteAction::Direct);
        let router = Router::new(rules);

        assert_eq!(router.route("example.com", 443), RouteAction::Direct);
        assert_eq!(router.route("other.com", 443), RouteAction::Proxy);
    }

    #[test]
    fn test_route_domain_suffix() {
        let mut rules = RuleSet::new();
        rules.add_rule(
            MatchRule::DomainSuffix(".google.com".into()),
            RouteAction::Proxy,
        );
        rules.add_rule(MatchRule::DomainSuffix(".cn".into()), RouteAction::Direct);
        rules.set_default(RouteAction::Proxy);
        let router = Router::new(rules);

        assert_eq!(router.route("www.google.com", 443), RouteAction::Proxy);
        assert_eq!(router.route("google.com", 443), RouteAction::Proxy);
        assert_eq!(router.route("baidu.cn", 80), RouteAction::Direct);
        assert_eq!(router.route("example.org", 80), RouteAction::Proxy);
    }

    #[test]
    fn test_route_domain_keyword() {
        let mut rules = RuleSet::new();
        rules.add_rule(
            MatchRule::DomainKeyword("google".into()),
            RouteAction::Proxy,
        );
        let router = Router::new(rules);

        assert_eq!(router.route("www.google.com", 443), RouteAction::Proxy);
        assert_eq!(router.route("googleapis.com", 443), RouteAction::Proxy);
        assert_eq!(router.route("example.com", 443), RouteAction::Proxy); // default
    }

    #[test]
    fn test_route_ip_cidr_v4() {
        let mut rules = RuleSet::new();
        rules.add_rule(
            MatchRule::IpCidr {
                addr: "192.168.0.0".parse().unwrap(),
                prefix_len: 16,
            },
            RouteAction::Direct,
        );
        rules.add_rule(
            MatchRule::IpCidr {
                addr: "10.0.0.0".parse().unwrap(),
                prefix_len: 8,
            },
            RouteAction::Direct,
        );
        let router = Router::new(rules);

        assert_eq!(router.route("192.168.1.1", 80), RouteAction::Direct);
        assert_eq!(router.route("192.168.255.255", 80), RouteAction::Direct);
        assert_eq!(router.route("192.169.0.1", 80), RouteAction::Proxy);
        assert_eq!(router.route("10.1.2.3", 80), RouteAction::Direct);
        assert_eq!(router.route("11.0.0.1", 80), RouteAction::Proxy);
    }

    #[test]
    fn test_route_ip_cidr_v6() {
        let mut rules = RuleSet::new();
        rules.add_rule(
            MatchRule::IpCidr {
                addr: "::1".parse().unwrap(),
                prefix_len: 128,
            },
            RouteAction::Direct,
        );
        rules.add_rule(
            MatchRule::IpCidr {
                addr: "fd00::".parse().unwrap(),
                prefix_len: 8,
            },
            RouteAction::Direct,
        );
        let router = Router::new(rules);

        assert_eq!(router.route("::1", 80), RouteAction::Direct);
        assert_eq!(router.route("fd12::1", 80), RouteAction::Direct);
        assert_eq!(router.route("2001:db8::1", 80), RouteAction::Proxy);
    }

    #[test]
    fn test_route_match_all() {
        let mut rules = RuleSet::new();
        rules.add_rule(MatchRule::DomainSuffix(".cn".into()), RouteAction::Direct);
        rules.add_rule(MatchRule::MatchAll, RouteAction::Proxy);
        let router = Router::new(rules);

        assert_eq!(router.route("baidu.cn", 80), RouteAction::Direct);
        assert_eq!(router.route("anything.com", 443), RouteAction::Proxy);
    }

    #[test]
    fn test_route_reject() {
        let mut rules = RuleSet::new();
        rules.add_rule(MatchRule::DomainKeyword("ads".into()), RouteAction::Reject);
        let router = Router::new(rules);

        assert_eq!(router.route("ads.example.com", 80), RouteAction::Reject);
    }

    #[test]
    fn test_from_config() {
        let rules = vec![
            RoutingRule {
                rule_type: "domain-suffix".into(),
                pattern: "cn".into(),
                target: "direct".into(),
                enabled: true,
            },
            RoutingRule {
                rule_type: "ip-cidr".into(),
                pattern: "192.168.0.0/16".into(),
                target: "direct".into(),
                enabled: true,
            },
            RoutingRule {
                rule_type: "domain-keyword".into(),
                pattern: "google".into(),
                target: "proxy".into(),
                enabled: true,
            },
            RoutingRule {
                rule_type: "match".into(),
                pattern: "".into(),
                target: "proxy".into(),
                enabled: true,
            },
        ];

        let ruleset = RuleSet::from_config(&rules);
        assert_eq!(ruleset.rules().len(), 4);

        let router = Router::new(ruleset);
        assert_eq!(router.route("baidu.cn", 80), RouteAction::Direct);
        assert_eq!(router.route("192.168.1.1", 443), RouteAction::Direct);
        assert_eq!(router.route("www.google.com", 443), RouteAction::Proxy);
    }

    #[test]
    fn test_parse_cidr() {
        assert_eq!(
            parse_cidr("192.168.0.0/16"),
            Some(("192.168.0.0".parse().unwrap(), 16))
        );
        assert_eq!(parse_cidr("::1/128"), Some(("::1".parse().unwrap(), 128)));
        assert_eq!(parse_cidr("invalid"), None);
        assert_eq!(parse_cidr("192.168.0.0/33"), None);
    }

    #[test]
    fn test_cidr_match() {
        assert!(cidr_match(
            "192.168.1.1".parse().unwrap(),
            "192.168.0.0".parse().unwrap(),
            16
        ));
        assert!(!cidr_match(
            "192.169.0.1".parse().unwrap(),
            "192.168.0.0".parse().unwrap(),
            16
        ));
        assert!(cidr_match(
            "10.0.0.0".parse().unwrap(),
            "0.0.0.0".parse().unwrap(),
            0
        ));
    }

    #[test]
    fn test_case_insensitive() {
        let mut rules = RuleSet::new();
        rules.add_rule(
            MatchRule::Domain("Example.COM".to_lowercase()),
            RouteAction::Direct,
        );
        let router = Router::new(rules);

        assert_eq!(router.route("EXAMPLE.COM", 80), RouteAction::Direct);
        assert_eq!(router.route("example.com", 80), RouteAction::Direct);
    }
}
