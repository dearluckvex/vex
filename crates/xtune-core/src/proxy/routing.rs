use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use anyhow::Result;

use super::connector::{BoxProxyStream, DirectOutbound, Outbound, SharedOutbound};
use crate::router::{RouteAction, Router};

/// An outbound that routes connections through a Router, dispatching to
/// either a proxy outbound, direct outbound, or rejecting the connection.
pub struct RoutingOutbound {
    router: Arc<Router>,
    proxy_outbound: SharedOutbound,
    direct_outbound: SharedOutbound,
}

impl RoutingOutbound {
    pub fn new(router: Arc<Router>, proxy_outbound: SharedOutbound) -> Self {
        Self {
            router,
            proxy_outbound,
            direct_outbound: SharedOutbound::direct(),
        }
    }
}

impl Outbound for RoutingOutbound {
    fn connect(
        &self,
        host: &str,
        port: u16,
    ) -> Pin<Box<dyn Future<Output = Result<BoxProxyStream>> + Send + '_>> {
        let action = self.router.route(host, port);
        tracing::debug!("Route: {}:{} -> {:?}", host, port, action);

        match action {
            RouteAction::Proxy => self.proxy_outbound.0.connect(host, port),
            RouteAction::Direct => self.direct_outbound.0.connect(host, port),
            RouteAction::Reject => {
                let msg = format!("Connection rejected by routing rule: {}:{}", host, port);
                Box::pin(async move { anyhow::bail!("{}", msg) })
            }
        }
    }

    fn name(&self) -> &str {
        "routing"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::router::{MatchRule, RuleSet};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    #[tokio::test]
    async fn test_routing_outbound_direct() {
        // Echo server
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let _echo = tokio::spawn(async move {
            let (mut s, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 5];
            s.read_exact(&mut buf).await.unwrap();
            s.write_all(&buf).await.unwrap();
        });

        // Route everything direct
        let mut rules = RuleSet::new();
        rules.set_default(RouteAction::Direct);
        let router = Arc::new(Router::new(rules));

        let outbound = RoutingOutbound::new(router, SharedOutbound::direct());
        let mut stream = outbound
            .connect(&addr.ip().to_string(), addr.port())
            .await
            .unwrap();

        stream.write_all(b"hello").await.unwrap();
        let mut buf = [0u8; 5];
        stream.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"hello");
    }

    #[tokio::test]
    async fn test_routing_outbound_reject() {
        let mut rules = RuleSet::new();
        rules.add_rule(MatchRule::DomainKeyword("ads".into()), RouteAction::Reject);
        let router = Arc::new(Router::new(rules));

        let outbound = RoutingOutbound::new(router, SharedOutbound::direct());
        let result = outbound.connect("ads.example.com", 80).await;
        assert!(result.is_err());
        let err = result.err().unwrap();
        assert!(err.to_string().contains("rejected"));
    }
}
