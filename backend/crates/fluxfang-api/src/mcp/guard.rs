use std::net::{IpAddr, SocketAddr};

use axum::extract::ConnectInfo;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{extract::Request, middleware::Next};

/// True for any loopback address (127.0.0.0/8, ::1). Fail-closed: anything
/// else is rejected by `mcp_guard`.
pub fn is_loopback(ip: IpAddr) -> bool {
    ip.is_loopback()
}

/// Reject any non-loopback peer with 403. The backend binds 0.0.0.0:8080 with
/// host networking, so "localhost only" must be enforced here, not assumed.
/// If the peer address is unavailable (e.g. served without connect info), fail
/// closed with 403.
pub async fn mcp_guard(req: Request, next: Next) -> Response {
    let allowed = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ci| is_loopback(ci.0.ip()))
        .unwrap_or(false);

    if allowed {
        next.run(req).await
    } else {
        (StatusCode::FORBIDDEN, "mcp endpoint is loopback-only").into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::is_loopback;
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    #[test]
    fn loopback_v4_and_v6_allowed_others_denied() {
        assert!(is_loopback(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))));
        assert!(is_loopback(IpAddr::V4(Ipv4Addr::new(127, 5, 5, 5))));
        assert!(is_loopback(IpAddr::V6(Ipv6Addr::LOCALHOST)));
        assert!(!is_loopback(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 4))));
        assert!(!is_loopback(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1))));
    }
}
