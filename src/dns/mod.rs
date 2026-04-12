pub mod parser;

use std::net::IpAddr;

#[derive(Debug, Clone)]
pub struct DnsQuery {
    pub domain: String,
    pub query_type: DnsQueryType,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DnsQueryType {
    A,      // IPv4
    AAAA,   // IPv6
    CNAME,  // Alias
    MX,     // Mail
    NS,     // NameServer
    TXT,    // Text
    Other(u16),
}

impl DnsQueryType {
    pub fn from_u16(val: u16) -> Self {
        match val {
            1 => DnsQueryType::A,
            28 => DnsQueryType::AAAA,
            5 => DnsQueryType::CNAME,
            15 => DnsQueryType::MX,
            2 => DnsQueryType::NS,
            16 => DnsQueryType::TXT,
            _ => DnsQueryType::Other(val),
        }
    }

    pub fn to_u16(&self) -> u16 {
        match self {
            DnsQueryType::A => 1,
            DnsQueryType::AAAA => 28,
            DnsQueryType::CNAME => 5,
            DnsQueryType::MX => 15,
            DnsQueryType::NS => 2,
            DnsQueryType::TXT => 16,
            DnsQueryType::Other(val) => *val,
        }
    }
}

#[derive(Debug, Clone)]
pub struct DnsRule {
    pub pattern: String,          // 域名模式（支持通配符）
    pub action: DnsRuleAction,    // 规则动作
}

#[derive(Debug, Clone, PartialEq)]
pub enum DnsRuleAction {
    Redirect(IpAddr),             // 重定向到指定 IP
    Block,                         // 阻止查询
    Pass,                          // 通过（默认）
}

pub struct DnsRuleEngine {
    rules: Vec<DnsRule>,
}

impl DnsRuleEngine {
    pub fn new() -> Self {
        Self { rules: Vec::new() }
    }

    pub fn add_rule(&mut self, rule: DnsRule) {
        self.rules.push(rule);
    }

    pub fn load_rules(&mut self, rules: Vec<DnsRule>) {
        self.rules = rules;
    }

    pub fn match_rule(&self, domain: &str) -> Option<DnsRuleAction> {
        for rule in &self.rules {
            if self.pattern_match(&rule.pattern, domain) {
                return Some(rule.action.clone());
            }
        }
        None
    }

    fn pattern_match(&self, pattern: &str, domain: &str) -> bool {
        // 简单的通配符匹配
        // *.example.com 匹配 sub.example.com
        // example.com 精确匹配

        if pattern == "*" {
            return true; // 匹配所有
        }

        if pattern == domain {
            return true; // 精确匹配
        }

        if pattern.starts_with("*.") {
            let suffix = &pattern[2..];
            return domain.ends_with(suffix) && domain != suffix;
        }

        false
    }

    pub fn get_rules_count(&self) -> usize {
        self.rules.len()
    }
}

// 预设规则
pub fn create_default_rules() -> Vec<DnsRule> {
    vec![
        // 示例：阻止常见的跟踪域名
        DnsRule {
            pattern: "*.doubleclick.net".to_string(),
            action: DnsRuleAction::Block,
        },
        DnsRule {
            pattern: "*.google-analytics.com".to_string(),
            action: DnsRuleAction::Block,
        },
        // 示例：重定向到本地 8.8.8.8
        DnsRule {
            pattern: "*.ads.com".to_string(),
            action: DnsRuleAction::Redirect("127.0.0.1".parse().unwrap()),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dns_query_type_conversion() {
        assert_eq!(DnsQueryType::from_u16(1), DnsQueryType::A);
        assert_eq!(DnsQueryType::from_u16(28), DnsQueryType::AAAA);
        assert_eq!(DnsQueryType::A.to_u16(), 1);
    }

    #[test]
    fn test_pattern_matching() {
        let engine = DnsRuleEngine::new();

        // 精确匹配
        assert!(engine.pattern_match("example.com", "example.com"));
        assert!(!engine.pattern_match("example.com", "sub.example.com"));

        // 通配符匹配
        assert!(engine.pattern_match("*.example.com", "sub.example.com"));
        assert!(engine.pattern_match("*.example.com", "deep.sub.example.com"));
        assert!(!engine.pattern_match("*.example.com", "example.com"));

        // 全局匹配
        assert!(engine.pattern_match("*", "any.domain.com"));
    }

    #[test]
    fn test_rule_engine() {
        let mut engine = DnsRuleEngine::new();
        
        engine.add_rule(DnsRule {
            pattern: "*.ads.com".to_string(),
            action: DnsRuleAction::Block,
        });

        engine.add_rule(DnsRule {
            pattern: "trusted.com".to_string(),
            action: DnsRuleAction::Pass,
        });

        assert!(matches!(
            engine.match_rule("ads.example.ads.com"),
            Some(DnsRuleAction::Block)
        ));

        assert!(matches!(
            engine.match_rule("trusted.com"),
            Some(DnsRuleAction::Pass)
        ));

        assert_eq!(engine.match_rule("other.com"), None);
    }
}
