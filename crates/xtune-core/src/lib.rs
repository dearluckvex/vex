pub mod config;
pub mod proxy;
pub mod router;
pub mod dns;

pub use config::model::{
    AppConfig, Node, ProxyProtocol, TransportConfig, TransportType, Subscription, RoutingRule,
};
pub use config::clash::parse_clash_config;
pub use config::v2ray::{
    parse_v2ray_config, parse_proxy_uri, parse_vmess_uri, parse_vless_uri,
    parse_ss_uri, parse_tuic_uri, parse_trojan_uri, parse_hysteria2_uri,
};
pub use config::subscription::{fetch_subscription, parse_subscription_content, detect_format};

pub use proxy::{ProxyState, ProxyStats};
pub use proxy::connector::{Outbound, DirectOutbound, SharedOutbound, BoxProxyStream, ProxyStream};
pub use proxy::service::ProxyService;
pub use proxy::socks5::Socks5Server;
pub use proxy::http::HttpProxyServer;
pub use proxy::vless::VlessOutbound;
pub use proxy::trojan::TrojanOutbound;
pub use proxy::ss::SsOutbound;
pub use proxy::vmess::VMessOutbound;
pub use proxy::factory::create_outbound;
pub use proxy::routing::RoutingOutbound;

pub use router::{Router, RouteAction, MatchRule, RuleSet, GeoIpDb};

