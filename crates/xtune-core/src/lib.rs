pub mod config;
pub mod dns;
pub mod proxy;
pub mod router;
pub mod system_proxy;

pub use config::clash::parse_clash_config;
pub use config::model::{
    AppConfig, Node, ProxyMode, ProxyProtocol, RoutingRule, Subscription, TransportConfig,
    TransportType, decode_display_name, normalize_node_names,
};
pub use config::singbox::{parse_singbox_config, parse_singbox_outbound};
pub use config::subscription::{detect_format, fetch_subscription, parse_subscription_content};
pub use config::v2ray::{
    parse_hysteria2_uri, parse_proxy_uri, parse_ss_uri, parse_trojan_uri, parse_tuic_uri,
    parse_v2ray_config, parse_vless_uri, parse_vmess_uri,
};

pub use proxy::connector::{BoxProxyStream, DirectOutbound, Outbound, ProxyStream, SharedOutbound};
pub use proxy::factory::create_outbound;
pub use proxy::http::HttpProxyServer;
pub use proxy::hysteria2::Hysteria2Outbound;
pub use proxy::relay::relay_bidirectional;
pub use proxy::routing::RoutingOutbound;
pub use proxy::service::ProxyService;
pub use proxy::socks5::Socks5Server;
pub use proxy::speedtest::{SpeedTestResult, http_latency_test, latency_test_node, speed_test_node, tcp_latency_test};
pub use proxy::ss::SsOutbound;
pub use proxy::trojan::TrojanOutbound;
pub use proxy::tuic::TuicOutbound;
pub use proxy::tun::{
    TunProxy, TunRouteGuard, TunRouteInfo, ensure_wintun_dll, resolve_to_ipv4, setup_tun_routes,
    tun_requirements, tun_supported, wintun_dll_available,
};
pub use proxy::vless::VlessOutbound;
pub use proxy::vmess::VMessOutbound;
pub use proxy::{ProxyState, ProxyStats};

pub use dns::{
    DnsConfig, DnsGroup, DnsResolver, DnsServer, build_dns_error_response, build_dns_response,
    china_domain_suffixes, china_split_dns_config, parse_dns_query,
};

pub use router::{GeoIpDb, MatchRule, RouteAction, Router, RuleSet, china_direct_ruleset};
pub use system_proxy::{
    DEFAULT_BYPASS, SystemProxyConfig, clear_system_proxy, get_system_proxy, set_system_proxy,
    set_system_proxy_with_bypass, system_proxy_supported,
};
