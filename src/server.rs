use std::convert::Infallible;
use std::future::Future;
use std::sync::Arc;

use aioduct::TokioClient;
use http::{Response, StatusCode};
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use tokio::net::TcpListener;

use crate::config::BrokerConfig;
use crate::error::BrokerError;
use crate::response::{ResponseBody, text_response, upstream_response};
use crate::routing::{route_request_path, validate_forward_path, validate_service_name};

/// Serve requests on an already-bound listener until `shutdown` resolves.
pub async fn serve_with_shutdown(
    listener: TcpListener,
    config: BrokerConfig,
    shutdown: impl Future<Output = ()>,
) -> Result<(), BrokerError> {
    let state = Arc::new(BrokerState {
        config,
        client: TokioClient::builder().build()?,
    });

    tokio::pin!(shutdown);

    loop {
        tokio::select! {
            () = &mut shutdown => {
                tracing::info!("http credential broker shutting down");
                return Ok(());
            }
            accepted = listener.accept() => {
                let (stream, peer_addr) = accepted?;
                let state = state.clone();
                tokio::spawn(async move {
                    let io = aioduct::runtime::tokio_rt::TokioIo::new(stream);
                    let service = service_fn(move |req| {
                        let state = state.clone();
                        async move { Ok::<_, Infallible>(handle_request(state, req).await) }
                    });

                    if let Err(err) = http1::Builder::new().serve_connection(io, service).await {
                        tracing::debug!(%peer_addr, error = %err, "connection failed");
                    }
                });
            }
        }
    }
}

#[derive(Clone)]
struct BrokerState {
    config: BrokerConfig,
    client: TokioClient,
}

async fn handle_request(
    state: Arc<BrokerState>,
    req: hyper::Request<Incoming>,
) -> Response<ResponseBody> {
    if req.uri().path() == "/healthz" {
        return text_response(StatusCode::OK, "ok\n");
    }

    let Some(route) = route_request_path(&state.config.proxy_prefix, req.uri().path()) else {
        return text_response(StatusCode::NOT_FOUND, "not found\n");
    };

    if let Err(msg) = validate_service_name(&route.service_name) {
        return text_response(
            StatusCode::BAD_REQUEST,
            format!("invalid service name: {msg}\n"),
        );
    }

    let Some(service) = state.config.services.get(&route.service_name).cloned() else {
        return text_response(StatusCode::NOT_FOUND, "unknown service\n");
    };

    if !service.allowed_methods.contains(req.method()) {
        return text_response(StatusCode::METHOD_NOT_ALLOWED, "method not allowed\n");
    }

    if let Err(msg) = validate_forward_path(&route.forward_path) {
        return text_response(StatusCode::BAD_REQUEST, format!("invalid path: {msg}\n"));
    }

    if !service.path_allowed(&route.forward_path) {
        return text_response(StatusCode::FORBIDDEN, "path not allowed\n");
    }

    let mut forward = state
        .client
        .forward(req)
        .upstream(service.base_url.clone())
        .strip_prefix(route.strip_prefix);

    if service.preserve_host {
        forward = forward.preserve_host();
    }

    if let Some(timeout) = service.timeout {
        forward = forward.timeout(timeout);
    }

    let headers = service.inject_headers.clone();
    let strip_headers = service.strip_request_headers.clone();
    let service_name = route.service_name;
    let forward = forward.on_request(move |parts| {
        for name in &strip_headers {
            parts.headers.remove(name);
        }
        for (name, value) in &headers {
            parts.headers.insert(name.clone(), value.clone());
        }
    });

    match forward.send().await {
        Ok(resp) => {
            tracing::info!(service = %service_name, status = %resp.status(), "proxied request");
            upstream_response(resp)
        }
        Err(err) => {
            tracing::warn!(service = %service_name, error = %err, "upstream request failed");
            text_response(StatusCode::BAD_GATEWAY, "upstream request failed\n")
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use http::StatusCode;
    use http::header::{AUTHORIZATION, HeaderValue};
    use hyper::Request;
    use tokio::sync::oneshot;

    use super::*;
    use crate::BrokerConfig;

    #[tokio::test]
    async fn proxy_injects_configured_auth_and_strips_caller_auth() {
        let upstream_seen = Arc::new(Mutex::new(Vec::new()));
        let upstream = start_echo_upstream(upstream_seen.clone()).await;
        let broker_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let broker_addr = broker_listener.local_addr().unwrap();

        let config = BrokerConfig::from_toml_str(&format!(
            r#"
listen = "{broker_addr}"

[services.test]
base_url = "http://{upstream}"
allowed_methods = ["GET"]
allowed_path_prefixes = ["/allowed"]

[services.test.auth]
type = "bearer"
token = "broker-token"
"#
        ))
        .unwrap();

        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let broker = tokio::spawn(async move {
            serve_with_shutdown(broker_listener, config, async {
                let _ = shutdown_rx.await;
            })
            .await
        });

        let client = TokioClient::new();
        let url = format!("http://{broker_addr}/proxy/test/allowed/resource");
        let resp = client
            .get(&url)
            .unwrap()
            .header(
                AUTHORIZATION,
                HeaderValue::from_static("Bearer caller-token"),
            )
            .send()
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(resp.text().await.unwrap(), "ok");

        shutdown_tx.send(()).unwrap();
        broker.await.unwrap().unwrap();

        let seen = upstream_seen.lock().unwrap();
        assert_eq!(
            seen.as_slice(),
            &[(
                "/allowed/resource".to_owned(),
                "Bearer broker-token".to_owned()
            )]
        );
    }

    #[tokio::test]
    async fn proxy_rejects_disallowed_path_before_upstream() {
        let upstream_seen = Arc::new(Mutex::new(Vec::new()));
        let upstream = start_echo_upstream(upstream_seen.clone()).await;
        let broker_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let broker_addr = broker_listener.local_addr().unwrap();

        let config = BrokerConfig::from_toml_str(&format!(
            r#"
listen = "{broker_addr}"

[services.test]
base_url = "http://{upstream}"
allowed_methods = ["GET"]
allowed_path_prefixes = ["/allowed"]
"#
        ))
        .unwrap();

        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let broker = tokio::spawn(async move {
            serve_with_shutdown(broker_listener, config, async {
                let _ = shutdown_rx.await;
            })
            .await
        });

        let client = TokioClient::new();
        let url = format!("http://{broker_addr}/proxy/test/denied");
        let resp = client.get(&url).unwrap().send().await.unwrap();

        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        assert!(upstream_seen.lock().unwrap().is_empty());

        shutdown_tx.send(()).unwrap();
        broker.await.unwrap().unwrap();
    }

    async fn start_echo_upstream(seen: Arc<Mutex<Vec<(String, String)>>>) -> std::net::SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                let seen = seen.clone();
                tokio::spawn(async move {
                    let io = aioduct::runtime::tokio_rt::TokioIo::new(stream);
                    let service = service_fn(move |req: Request<Incoming>| {
                        let seen = seen.clone();
                        async move {
                            let path = req.uri().path().to_owned();
                            let auth = req
                                .headers()
                                .get(AUTHORIZATION)
                                .and_then(|value| value.to_str().ok())
                                .unwrap_or("<missing>")
                                .to_owned();
                            seen.lock().unwrap().push((path, auth));
                            Ok::<_, Infallible>(text_response(StatusCode::OK, "ok"))
                        }
                    });
                    let _ = http1::Builder::new().serve_connection(io, service).await;
                });
            }
        });
        addr
    }
}
