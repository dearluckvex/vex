use std::collections::HashMap;
use std::net::IpAddr;

/// A lightweight GeoIP database for country-level lookups.
/// Uses in-memory CIDR→country mappings (can be loaded from file).
pub struct GeoIpDb {
    v4_entries: Vec<GeoEntry>,
    v6_entries: Vec<GeoEntry>,
}

#[derive(Debug, Clone)]
struct GeoEntry {
    addr: u128, // stored as u128 for both v4 and v6
    prefix_len: u8,
    country: String,
}

impl GeoIpDb {
    /// Create an empty GeoIP database.
    pub fn new() -> Self {
        Self {
            v4_entries: Vec::new(),
            v6_entries: Vec::new(),
        }
    }

    /// Add a CIDR→country mapping.
    pub fn add_entry(&mut self, cidr: &str, country: &str) {
        let parts: Vec<&str> = cidr.splitn(2, '/').collect();
        if parts.len() != 2 {
            return;
        }
        let addr: IpAddr = match parts[0].parse() {
            Ok(a) => a,
            Err(_) => return,
        };
        let prefix_len: u8 = match parts[1].parse() {
            Ok(p) => p,
            Err(_) => return,
        };

        let entry = GeoEntry {
            addr: ip_to_u128(addr),
            prefix_len,
            country: country.to_uppercase(),
        };

        match addr {
            IpAddr::V4(_) => self.v4_entries.push(entry),
            IpAddr::V6(_) => self.v6_entries.push(entry),
        }
    }

    /// Sort entries for efficient lookup (longest prefix first).
    pub fn build(&mut self) {
        self.v4_entries
            .sort_by(|a, b| b.prefix_len.cmp(&a.prefix_len));
        self.v6_entries
            .sort_by(|a, b| b.prefix_len.cmp(&a.prefix_len));
    }

    /// Lookup the country code for an IP address.
    pub fn lookup(&self, ip: IpAddr) -> Option<&str> {
        let ip_bits = ip_to_u128(ip);
        let entries = match ip {
            IpAddr::V4(_) => &self.v4_entries,
            IpAddr::V6(_) => &self.v6_entries,
        };
        let total_bits: u32 = match ip {
            IpAddr::V4(_) => 32,
            IpAddr::V6(_) => 128,
        };

        for entry in entries {
            let shift = total_bits.saturating_sub(entry.prefix_len as u32);
            let mask = if shift >= 128 {
                0u128
            } else {
                u128::MAX.checked_shl(shift).unwrap_or(0)
            };

            // For IPv4, the bits are stored in the lower 32 bits via ip_to_u128
            if (ip_bits & mask) == (entry.addr & mask) {
                return Some(&entry.country);
            }
        }
        None
    }

    /// Load a simple text format: each line is "CIDR COUNTRY_CODE".
    pub fn load_from_text(text: &str) -> Self {
        let mut db = Self::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 2 {
                db.add_entry(parts[0], parts[1]);
            }
        }
        db.build();
        db
    }

    /// Create a built-in database with common private/reserved ranges.
    pub fn builtin() -> Self {
        let mut db = Self::new();

        // Private IPv4 ranges → PRIVATE
        db.add_entry("10.0.0.0/8", "PRIVATE");
        db.add_entry("172.16.0.0/12", "PRIVATE");
        db.add_entry("192.168.0.0/16", "PRIVATE");
        db.add_entry("127.0.0.0/8", "PRIVATE");

        // Link-local
        db.add_entry("169.254.0.0/16", "PRIVATE");

        // Private IPv6
        db.add_entry("::1/128", "PRIVATE");
        db.add_entry("fc00::/7", "PRIVATE");
        db.add_entry("fe80::/10", "PRIVATE");

        db.build();
        db
    }

    pub fn entry_count(&self) -> usize {
        self.v4_entries.len() + self.v6_entries.len()
    }
}

impl Default for GeoIpDb {
    fn default() -> Self {
        Self::new()
    }
}

fn ip_to_u128(ip: IpAddr) -> u128 {
    match ip {
        IpAddr::V4(v4) => {
            // Store IPv4 in the upper bits of a 32-bit space
            // but we shift to align with mask calculations
            u32::from(v4) as u128
        }
        IpAddr::V6(v6) => u128::from(v6),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_builtin_private() {
        let db = GeoIpDb::builtin();

        assert_eq!(
            db.lookup("192.168.1.1".parse().unwrap()),
            Some("PRIVATE")
        );
        assert_eq!(
            db.lookup("10.0.0.1".parse().unwrap()),
            Some("PRIVATE")
        );
        assert_eq!(
            db.lookup("172.16.5.1".parse().unwrap()),
            Some("PRIVATE")
        );
        assert_eq!(
            db.lookup("127.0.0.1".parse().unwrap()),
            Some("PRIVATE")
        );
        assert_eq!(db.lookup("::1".parse().unwrap()), Some("PRIVATE"));

        // Public IPs should not match
        assert_eq!(db.lookup("8.8.8.8".parse().unwrap()), None);
        assert_eq!(db.lookup("1.1.1.1".parse().unwrap()), None);
    }

    #[test]
    fn test_load_from_text() {
        let text = r#"
# GeoIP test data
1.0.0.0/8 CN
8.8.8.0/24 US
2001:db8::/32 TEST
"#;
        let db = GeoIpDb::load_from_text(text);
        assert_eq!(db.entry_count(), 3);

        assert_eq!(db.lookup("1.2.3.4".parse().unwrap()), Some("CN"));
        assert_eq!(db.lookup("8.8.8.8".parse().unwrap()), Some("US"));
        assert_eq!(db.lookup("8.8.9.1".parse().unwrap()), None);
        assert_eq!(
            db.lookup("2001:db8::1".parse().unwrap()),
            Some("TEST")
        );
    }

    #[test]
    fn test_longest_prefix_match() {
        let text = r#"
1.0.0.0/8 CN
1.1.1.0/24 US
"#;
        let db = GeoIpDb::load_from_text(text);

        // 1.1.1.1 matches both, but /24 is more specific
        assert_eq!(db.lookup("1.1.1.1".parse().unwrap()), Some("US"));
        // 1.2.3.4 only matches /8
        assert_eq!(db.lookup("1.2.3.4".parse().unwrap()), Some("CN"));
    }
}
