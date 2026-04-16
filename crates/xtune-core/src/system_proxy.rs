use anyhow::{Context, Result, bail};

pub const DEFAULT_BYPASS: &str = "localhost,127.0.0.1,::1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemProxyConfig {
    pub enabled: bool,
    pub host: String,
    pub port: u16,
    pub bypass: String,
}

impl From<sysproxy::Sysproxy> for SystemProxyConfig {
    fn from(value: sysproxy::Sysproxy) -> Self {
        Self {
            enabled: value.enable,
            host: value.host,
            port: value.port,
            bypass: value.bypass,
        }
    }
}

pub fn system_proxy_supported() -> bool {
    sysproxy::Sysproxy::is_support()
}

pub fn get_system_proxy() -> Result<SystemProxyConfig> {
    ensure_supported()?;
    sysproxy::Sysproxy::get_system_proxy()
        .map(Into::into)
        .map_err(|e| {
            anyhow::anyhow!(
                "failed to read system proxy: {}. On Windows, try running as Administrator.",
                e
            )
        })
}

pub fn set_system_proxy(host: &str, port: u16) -> Result<()> {
    set_system_proxy_with_bypass(host, port, DEFAULT_BYPASS)
}

pub fn set_system_proxy_with_bypass(host: &str, port: u16, bypass: &str) -> Result<()> {
    ensure_supported()?;

    let host = host.trim();
    if host.is_empty() {
        bail!("system proxy host cannot be empty");
    }

    sysproxy::Sysproxy {
        enable: true,
        host: host.to_string(),
        port,
        bypass: bypass.to_string(),
    }
    .set_system_proxy()
    .map_err(|e| {
        let msg = format!("{}", e);
        if msg.contains("denied") || msg.contains("permission") || msg.contains("access") {
            anyhow::anyhow!(
                "failed to set system proxy ({}:{}) — {}\nTry running as Administrator.",
                host, port, e
            )
        } else {
            anyhow::anyhow!(
                "failed to set system proxy ({}:{}) — {}",
                host, port, e
            )
        }
    })
}

pub fn clear_system_proxy() -> Result<()> {
    ensure_supported()?;

    sysproxy::Sysproxy {
        enable: false,
        host: String::new(),
        port: 0,
        bypass: String::new(),
    }
    .set_system_proxy()
    .context("failed to clear system proxy")
}

fn ensure_supported() -> Result<()> {
    if system_proxy_supported() {
        Ok(())
    } else {
        bail!("system proxy is not supported on this platform")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_bypass_has_local_targets() {
        assert!(DEFAULT_BYPASS.contains("localhost"));
        assert!(DEFAULT_BYPASS.contains("127.0.0.1"));
        assert!(DEFAULT_BYPASS.contains("::1"));
    }

    #[test]
    fn sysproxy_conversion_preserves_fields() {
        let raw = sysproxy::Sysproxy {
            enable: true,
            host: "127.0.0.1".into(),
            port: 1080,
            bypass: "localhost".into(),
        };

        let proxy: SystemProxyConfig = raw.into();
        assert!(proxy.enabled);
        assert_eq!(proxy.host, "127.0.0.1");
        assert_eq!(proxy.port, 1080);
        assert_eq!(proxy.bypass, "localhost");
    }
}
