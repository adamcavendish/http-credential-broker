use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::net::SocketAddr;
use std::path::Path;
use std::time::Duration;

use base64::Engine as _;
use http::header::{AUTHORIZATION, HeaderName, HeaderValue, PROXY_AUTHORIZATION};
use http::{HeaderMap, Method, Uri};
use serde::Deserialize;

use crate::error::{BrokerError, config_error};
use crate::routing::validate_service_name;

/// Runtime broker configuration.
#[derive(Clone, Debug)]
pub struct BrokerConfig {
    /// Socket address to listen on.
    pub listen: SocketAddr,
    /// URL prefix used for proxied services.
    pub proxy_prefix: String,
    /// Named upstream services.
    pub services: BTreeMap<String, ServiceConfig>,
}

impl BrokerConfig {
    /// Load and validate a TOML configuration file.
    pub fn from_path(path: impl AsRef<Path>) -> Result<Self, BrokerError> {
        let source = std::fs::read_to_string(path)?;
        Self::from_toml_str(&source)
    }

    /// Parse and validate TOML configuration content.
    pub fn from_toml_str(source: &str) -> Result<Self, BrokerError> {
        let raw: RawBrokerConfig = toml::from_str(source)?;
        raw.resolve()
    }
}

/// Runtime configuration for a single named upstream service.
#[derive(Clone, Debug)]
pub struct ServiceConfig {
    pub(crate) base_url: Uri,
    pub(crate) allowed_methods: BTreeSet<Method>,
    pub(crate) allowed_path_prefixes: Vec<String>,
    pub(crate) inject_headers: HeaderMap,
    pub(crate) strip_request_headers: Vec<HeaderName>,
    pub(crate) preserve_host: bool,
    pub(crate) timeout: Option<Duration>,
}

impl ServiceConfig {
    pub(crate) fn path_allowed(&self, path: &str) -> bool {
        self.allowed_path_prefixes
            .iter()
            .any(|prefix| crate::routing::path_matches_prefix(path, prefix))
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawBrokerConfig {
    listen: SocketAddr,
    #[serde(default = "default_proxy_prefix")]
    proxy_prefix: String,
    services: BTreeMap<String, RawServiceConfig>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawServiceConfig {
    base_url: String,
    allowed_methods: Vec<String>,
    allowed_path_prefixes: Vec<String>,
    #[serde(default)]
    auth: Option<RawAuthConfig>,
    #[serde(default)]
    headers: Vec<RawHeaderConfig>,
    #[serde(default = "default_strip_request_headers")]
    strip_request_headers: Vec<String>,
    #[serde(default)]
    preserve_host: bool,
    #[serde(default)]
    timeout_ms: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawHeaderConfig {
    name: String,
    value: Option<String>,
    value_env: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum RawAuthConfig {
    Bearer {
        token: Option<String>,
        token_env: Option<String>,
    },
    Basic {
        username: Option<String>,
        username_env: Option<String>,
        password: Option<String>,
        password_env: Option<String>,
    },
    Header {
        name: String,
        value: Option<String>,
        value_env: Option<String>,
    },
}

impl RawBrokerConfig {
    fn resolve(self) -> Result<BrokerConfig, BrokerError> {
        let proxy_prefix = validate_proxy_prefix(&self.proxy_prefix)?;
        if self.services.is_empty() {
            return Err(config_error("at least one service must be configured"));
        }

        let mut services = BTreeMap::new();
        for (name, raw) in self.services {
            validate_service_name(&name)
                .map_err(|msg| config_error(format!("service `{name}`: {msg}")))?;
            let service = raw
                .resolve(&name)
                .map_err(|err| config_error(format!("service `{name}`: {err}")))?;
            services.insert(name, service);
        }

        Ok(BrokerConfig {
            listen: self.listen,
            proxy_prefix,
            services,
        })
    }
}

impl RawServiceConfig {
    fn resolve(self, service_name: &str) -> Result<ServiceConfig, String> {
        let base_url = parse_base_url(&self.base_url)?;
        let allowed_methods = parse_methods(&self.allowed_methods)?;
        let allowed_path_prefixes = parse_path_prefixes(&self.allowed_path_prefixes)?;
        let mut inject_headers = HeaderMap::new();

        if let Some(auth) = self.auth {
            let (name, value) = auth.resolve()?;
            inject_headers.insert(name, value);
        }

        for header in self.headers {
            let (name, value) = header.resolve(service_name)?;
            inject_headers.insert(name, value);
        }

        let strip_request_headers =
            parse_strip_request_headers(self.strip_request_headers, service_name)?;

        Ok(ServiceConfig {
            base_url,
            allowed_methods,
            allowed_path_prefixes,
            inject_headers,
            strip_request_headers,
            preserve_host: self.preserve_host,
            timeout: self.timeout_ms.map(Duration::from_millis),
        })
    }
}

impl RawHeaderConfig {
    fn resolve(self, service_name: &str) -> Result<(HeaderName, HeaderValue), String> {
        let name = parse_header_name(&self.name)?;
        ensure_config_injectable_header(&name)?;
        let value = resolve_secret_source(
            &format!("service `{service_name}` header `{}`", self.name),
            self.value,
            self.value_env,
        )?;
        let value = HeaderValue::from_str(&value)
            .map_err(|err| format!("invalid header `{}` value: {err}", self.name))?;
        Ok((name, value))
    }
}

impl RawAuthConfig {
    fn resolve(self) -> Result<(HeaderName, HeaderValue), String> {
        match self {
            RawAuthConfig::Bearer { token, token_env } => {
                let token = resolve_secret_source("bearer token", token, token_env)?;
                let value = HeaderValue::from_str(&format!("Bearer {token}"))
                    .map_err(|err| format!("invalid bearer token header value: {err}"))?;
                Ok((AUTHORIZATION, value))
            }
            RawAuthConfig::Basic {
                username,
                username_env,
                password,
                password_env,
            } => {
                let username =
                    resolve_secret_source("basic auth username", username, username_env)?;
                let password =
                    resolve_secret_source("basic auth password", password, password_env)?;
                let encoded = base64::engine::general_purpose::STANDARD
                    .encode(format!("{username}:{password}"));
                let value = HeaderValue::from_str(&format!("Basic {encoded}"))
                    .map_err(|err| format!("invalid basic auth header value: {err}"))?;
                Ok((AUTHORIZATION, value))
            }
            RawAuthConfig::Header {
                name,
                value,
                value_env,
            } => {
                let name = parse_header_name(&name)?;
                ensure_config_injectable_header(&name)?;
                let value = resolve_secret_source("header auth value", value, value_env)?;
                let value = HeaderValue::from_str(&value)
                    .map_err(|err| format!("invalid header auth value: {err}"))?;
                Ok((name, value))
            }
        }
    }
}

fn validate_proxy_prefix(prefix: &str) -> Result<String, BrokerError> {
    let prefix = prefix.trim_end_matches('/');
    if prefix.is_empty() || !prefix.starts_with('/') {
        return Err(config_error("proxy_prefix must start with `/`"));
    }
    if prefix.contains('?') || prefix.contains('#') {
        return Err(config_error("proxy_prefix must be a path, not a URL"));
    }
    Ok(prefix.to_owned())
}

fn parse_base_url(raw: &str) -> Result<Uri, String> {
    if raw.contains('#') {
        return Err("base_url must not include fragment".to_owned());
    }
    let uri = raw
        .parse::<Uri>()
        .map_err(|err| format!("invalid base_url `{raw}`: {err}"))?;
    if uri.scheme().is_none() || uri.authority().is_none() {
        return Err("base_url must include scheme and authority".to_owned());
    }
    if uri.query().is_some() {
        return Err("base_url must not include query".to_owned());
    }
    Ok(uri)
}

fn parse_methods(raw: &[String]) -> Result<BTreeSet<Method>, String> {
    if raw.is_empty() {
        return Err("allowed_methods must not be empty".to_owned());
    }

    let mut methods = BTreeSet::new();
    for value in raw {
        let method = Method::from_bytes(value.as_bytes())
            .map_err(|err| format!("invalid HTTP method `{value}`: {err}"))?;
        if method == Method::CONNECT {
            return Err("CONNECT is not supported by this broker".to_owned());
        }
        methods.insert(method);
    }
    Ok(methods)
}

fn parse_path_prefixes(raw: &[String]) -> Result<Vec<String>, String> {
    if raw.is_empty() {
        return Err("allowed_path_prefixes must not be empty".to_owned());
    }

    raw.iter()
        .map(|prefix| {
            if !prefix.starts_with('/') {
                return Err(format!("path prefix `{prefix}` must start with `/`"));
            }
            if prefix.contains('?') || prefix.contains('#') {
                return Err(format!(
                    "path prefix `{prefix}` must not contain query or fragment"
                ));
            }
            Ok(prefix.clone())
        })
        .collect()
}

fn parse_strip_request_headers(
    raw: Vec<String>,
    service_name: &str,
) -> Result<Vec<HeaderName>, String> {
    raw.into_iter()
        .map(|name| {
            parse_header_name(&name).map_err(|err| {
                format!("invalid strip_request_headers entry for `{service_name}`: {err}")
            })
        })
        .collect()
}

fn parse_header_name(raw: &str) -> Result<HeaderName, String> {
    HeaderName::from_bytes(raw.as_bytes())
        .map_err(|err| format!("invalid header name `{raw}`: {err}"))
}

fn ensure_config_injectable_header(name: &HeaderName) -> Result<(), String> {
    const BLOCKED: &[&str] = &[
        "connection",
        "content-length",
        "host",
        "proxy-authorization",
        "te",
        "trailer",
        "transfer-encoding",
        "upgrade",
    ];
    if BLOCKED
        .iter()
        .any(|blocked| name.as_str().eq_ignore_ascii_case(blocked))
    {
        return Err(format!(
            "header `{name}` is managed by the broker or HTTP stack"
        ));
    }
    Ok(())
}

fn resolve_secret_source(
    label: &str,
    value: Option<String>,
    env_name: Option<String>,
) -> Result<String, String> {
    match (value, env_name) {
        (Some(_), Some(_)) => Err(format!("{label} must use either value or env, not both")),
        (Some(value), None) => {
            if value.is_empty() {
                Err(format!("{label} value must not be empty"))
            } else {
                Ok(value)
            }
        }
        (None, Some(env_name)) => {
            if env_name.is_empty() {
                return Err(format!("{label} env name must not be empty"));
            }
            let value = env::var(&env_name)
                .map_err(|_| format!("{label} env var `{env_name}` is missing"))?;
            if value.is_empty() {
                Err(format!("{label} env var `{env_name}` must not be empty"))
            } else {
                Ok(value)
            }
        }
        (None, None) => Err(format!("{label} must provide value or env")),
    }
}

fn default_proxy_prefix() -> String {
    "/proxy".to_owned()
}

fn default_strip_request_headers() -> Vec<String> {
    vec![
        AUTHORIZATION.as_str().to_owned(),
        PROXY_AUTHORIZATION.as_str().to_owned(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_requires_referenced_env_values() {
        let source = r#"
listen = "127.0.0.1:0"

[services.github]
base_url = "https://api.github.com"
allowed_methods = ["GET"]
allowed_path_prefixes = ["/repos/acme/"]

[services.github.auth]
type = "bearer"
token_env = "HTTP_CREDENTIAL_BROKER_TEST_MISSING_TOKEN"
"#;
        let err = BrokerConfig::from_toml_str(source).unwrap_err();
        assert!(
            err.to_string()
                .contains("HTTP_CREDENTIAL_BROKER_TEST_MISSING_TOKEN")
        );
    }

    #[test]
    fn config_rejects_arbitrary_or_unsafe_routes() {
        let source = r#"
listen = "127.0.0.1:0"

[services.bad]
base_url = "https://example.com/path?x=1"
allowed_methods = ["GET"]
allowed_path_prefixes = ["/"]
"#;
        let err = BrokerConfig::from_toml_str(source).unwrap_err();
        assert!(err.to_string().contains("base_url must not include query"));
    }
}
