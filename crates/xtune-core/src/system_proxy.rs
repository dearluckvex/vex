use anyhow::{Context, Result, bail};

pub const DEFAULT_BYPASS: &str = "localhost,127.0.0.1,::1";

/// Platform-appropriate bypass list for system proxy settings.
/// Windows uses semicolons and `<local>` keyword.
#[cfg(target_os = "windows")]
pub const PLATFORM_BYPASS: &str = "localhost;127.0.0.1;::1;<local>";
#[cfg(not(target_os = "windows"))]
pub const PLATFORM_BYPASS: &str = "localhost,127.0.0.1,::1";

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
    match sysproxy::Sysproxy::get_system_proxy() {
        Ok(proxy) => Ok(proxy.into()),
        Err(e) => {
            let msg = format!("{}", e);
            if msg.contains("parse") {
                // The sysproxy crate can't parse per-protocol proxy formats
                // (e.g., "http=host:port;https=host:port"). Treat as disabled.
                Ok(SystemProxyConfig {
                    enabled: false,
                    host: String::new(),
                    port: 0,
                    bypass: String::new(),
                })
            } else {
                Err(anyhow::anyhow!("failed to read system proxy: {}", e))
            }
        }
    }
}

pub fn set_system_proxy(host: &str, port: u16) -> Result<()> {
    set_system_proxy_with_bypass(host, port, PLATFORM_BYPASS)
}

pub fn set_system_proxy_with_bypass(host: &str, port: u16, bypass: &str) -> Result<()> {
    ensure_supported()?;

    let host = host.trim();
    if host.is_empty() {
        bail!("system proxy host cannot be empty");
    }

    let proxy = sysproxy::Sysproxy {
        enable: true,
        host: host.to_string(),
        port,
        bypass: bypass.to_string(),
    };

    match proxy.set_system_proxy() {
        Ok(()) => return Ok(()),
        Err(e) => {
            let msg = format!("{}", e);
            // On Windows the registry may hold a per-protocol value like
            // "http=host:port;https=host:port" that the sysproxy crate cannot
            // re-parse before writing.  Clear first and retry once.
            if msg.contains("parse") || msg.contains("string") {
                tracing::warn!(
                    "set_system_proxy: parse error on first attempt ({}), clearing then retrying",
                    msg
                );
                // Best-effort clear; ignore any error.
                let _ = sysproxy::Sysproxy {
                    enable: false,
                    host: String::new(),
                    port: 0,
                    bypass: String::new(),
                }
                .set_system_proxy();

                proxy.set_system_proxy().map_err(|e2| {
                    anyhow::anyhow!("failed to set system proxy ({}:{}) — {}", host, port, e2)
                })?;
                return Ok(());
            }

            if msg.contains("denied") || msg.contains("permission") || msg.contains("access") {
                return Err(anyhow::anyhow!(
                    "failed to set system proxy ({}:{}) — {}\nTry running as Administrator.",
                    host,
                    port,
                    e
                ));
            }
            return Err(anyhow::anyhow!(
                "failed to set system proxy ({}:{}) — {}",
                host,
                port,
                e
            ));
        }
    }
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
