use anyhow::{Context, Result};
use base64::Engine;
use base64::engine::general_purpose;

use super::clash;
use super::model::*;
use super::singbox;
use super::v2ray;

/// Subscription content format (auto-detected)
#[derive(Debug, PartialEq)]
pub enum SubFormat {
    Clash,
    SingBox,
    V2Ray,
    Base64Lines,
    PlainLines,
}

/// Fetch a subscription URL and parse it into nodes
pub async fn fetch_subscription(sub: &Subscription) -> Result<Vec<Node>> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .user_agent("Vex/0.1")
        .danger_accept_invalid_certs(true)
        .build()
        .context("Failed to create HTTP client")?;

    let response = client
        .get(&sub.url)
        .send()
        .await
        .context("Failed to fetch subscription")?;

    let status = response.status();
    if !status.is_success() {
        anyhow::bail!("Subscription returned HTTP {}", status);
    }

    let content = response
        .text()
        .await
        .context("Failed to read subscription body")?;

    parse_subscription_content(&content, &sub.format)
}

/// Parse subscription content with format detection
pub fn parse_subscription_content(content: &str, format_hint: &str) -> Result<Vec<Node>> {
    let content = content.trim();

    match format_hint {
        "clash" => return clash::parse_clash_config(content),
        "singbox" | "sing-box" => return singbox::parse_singbox_config(content),
        "v2ray" => return v2ray::parse_v2ray_config(content),
        "base64" => return parse_base64_lines(content),
        _ => {} // "auto" or unknown → auto-detect
    }

    // Auto-detect format
    let format = detect_format(content);
    tracing::info!("Auto-detected subscription format: {:?}", format);

    match format {
        SubFormat::Clash => clash::parse_clash_config(content),
        SubFormat::SingBox => singbox::parse_singbox_config(content),
        SubFormat::V2Ray => v2ray::parse_v2ray_config(content),
        SubFormat::Base64Lines => parse_base64_lines(content),
        SubFormat::PlainLines => parse_plain_lines(content),
    }
}

/// Detect the format of subscription content
pub fn detect_format(content: &str) -> SubFormat {
    let trimmed = content.trim();

    // Check for Clash YAML
    if clash::is_clash_config(trimmed) {
        return SubFormat::Clash;
    }

    // Check for sing-box JSON (before generic V2Ray check)
    if trimmed.starts_with('{') && singbox::is_singbox_config(trimmed) {
        return SubFormat::SingBox;
    }

    // Check for V2Ray JSON
    if trimmed.starts_with('{') && v2ray::is_v2ray_config(trimmed) {
        return SubFormat::V2Ray;
    }

    // Check if content has proxy URI lines
    let first_line = trimmed.lines().next().unwrap_or("");
    if first_line.contains("://") {
        return SubFormat::PlainLines;
    }

    // Try Base64 decode
    if looks_like_base64(trimmed) {
        return SubFormat::Base64Lines;
    }

    // Default to plain lines
    SubFormat::PlainLines
}

fn looks_like_base64(content: &str) -> bool {
    let clean: String = content.chars().filter(|c| !c.is_whitespace()).collect();

    if clean.is_empty() {
        return false;
    }

    // Base64 characters only
    clean
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '/' || c == '=')
}

/// Decode Base64 content and parse each line as a proxy URI
fn parse_base64_lines(content: &str) -> Result<Vec<Node>> {
    let clean: String = content
        .chars()
        .filter(|c| !c.is_whitespace() || *c == '\n')
        .collect();

    // Try standard Base64 first, then URL-safe
    let decoded = general_purpose::STANDARD
        .decode(clean.trim())
        .or_else(|_| general_purpose::STANDARD_NO_PAD.decode(clean.trim()))
        .or_else(|_| general_purpose::URL_SAFE.decode(clean.trim()))
        .or_else(|_| general_purpose::URL_SAFE_NO_PAD.decode(clean.trim()))
        .context("Failed to Base64 decode subscription content")?;

    let text = String::from_utf8(decoded).context("Invalid UTF-8 after Base64 decode")?;

    // The decoded content might be Clash YAML, sing-box JSON, or plain URI lines
    if clash::is_clash_config(&text) {
        return clash::parse_clash_config(&text);
    }
    if singbox::is_singbox_config(&text) {
        return singbox::parse_singbox_config(&text);
    }

    parse_plain_lines(&text)
}

/// Parse plain text where each line is a proxy URI
fn parse_plain_lines(content: &str) -> Result<Vec<Node>> {
    let mut nodes = Vec::new();

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        match v2ray::parse_proxy_uri(line) {
            Ok(node) => nodes.push(node),
            Err(e) => {
                tracing::debug!("Skipping line: {} ({})", line, e);
            }
        }
    }

    if nodes.is_empty() {
        anyhow::bail!("No valid proxy nodes found in subscription");
    }

    Ok(nodes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_clash_format() {
        let content = "proxies:\n  - name: test\n    type: ss";
        assert_eq!(detect_format(content), SubFormat::Clash);
    }

    #[test]
    fn test_detect_v2ray_format() {
        let content = r#"{"outbounds": []}"#;
        assert_eq!(detect_format(content), SubFormat::V2Ray);
    }

    #[test]
    fn test_detect_plain_lines() {
        let content = "ss://abc@1.2.3.4:8388#test\nvmess://xyz";
        assert_eq!(detect_format(content), SubFormat::PlainLines);
    }

    #[test]
    fn test_detect_base64() {
        let plain = "ss://YWVzLTI1Ni1nY206cGFzcw==@1.2.3.4:8388#test\n";
        let encoded = general_purpose::STANDARD.encode(plain);
        assert_eq!(detect_format(&encoded), SubFormat::Base64Lines);
    }

    #[test]
    fn test_parse_base64_subscription() {
        use base64::engine::general_purpose;

        let lines = "ss://YWVzLTI1Ni1nY206cGFzc3dvcmQ=@1.2.3.4:8388#SS-1\nss://YWVzLTI1Ni1nY206cGFzc3dvcmQ=@5.6.7.8:8388#SS-2\n";
        let encoded = general_purpose::STANDARD.encode(lines);

        let nodes = parse_subscription_content(&encoded, "auto").unwrap();
        assert_eq!(nodes.len(), 2);
        assert_eq!(nodes[0].name, "SS-1");
        assert_eq!(nodes[1].name, "SS-2");
    }

    #[test]
    fn test_parse_plain_lines() {
        let content = "ss://YWVzLTI1Ni1nY206cGFzc3dvcmQ=@1.2.3.4:8388#SS-1\n# comment\n\nss://YWVzLTI1Ni1nY206cGFzc3dvcmQ=@5.6.7.8:8388#SS-2\n";
        let nodes = parse_subscription_content(content, "auto").unwrap();
        assert_eq!(nodes.len(), 2);
    }
}
