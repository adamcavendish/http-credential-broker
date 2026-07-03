use percent_encoding::percent_decode_str;

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct RouteMatch {
    pub(crate) service_name: String,
    pub(crate) forward_path: String,
    pub(crate) strip_prefix: String,
}

pub(crate) fn route_request_path(proxy_prefix: &str, path: &str) -> Option<RouteMatch> {
    let prefix = proxy_prefix.trim_end_matches('/');
    let rest = if path == prefix {
        return None;
    } else {
        path.strip_prefix(&format!("{prefix}/"))?
    };

    let (service_name, tail) = rest.split_once('/').unwrap_or((rest, ""));
    if service_name.is_empty() {
        return None;
    }

    let forward_path = if tail.is_empty() {
        "/".to_owned()
    } else {
        format!("/{tail}")
    };

    Some(RouteMatch {
        service_name: service_name.to_owned(),
        forward_path,
        strip_prefix: format!("{prefix}/{service_name}"),
    })
}

pub(crate) fn validate_service_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("must not be empty".to_owned());
    }
    if !name
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err("may contain only ASCII letters, digits, `_`, or `-`".to_owned());
    }
    Ok(())
}

pub(crate) fn validate_forward_path(path: &str) -> Result<(), String> {
    if !path.starts_with('/') {
        return Err("must start with `/`".to_owned());
    }
    for segment in path.split('/') {
        let decoded = percent_decode_str(segment)
            .decode_utf8()
            .map_err(|_| "path segments must be valid UTF-8 after percent decoding".to_owned())?;
        if decoded == "." || decoded == ".." {
            return Err("dot segments are not allowed".to_owned());
        }
        if decoded.contains('/') || decoded.contains('\\') {
            return Err("encoded path separators are not allowed".to_owned());
        }
    }
    Ok(())
}

pub(crate) fn path_matches_prefix(path: &str, prefix: &str) -> bool {
    if prefix == "/" || path == prefix {
        return true;
    }
    if prefix.ends_with('/') {
        path.starts_with(prefix)
    } else {
        path.strip_prefix(prefix)
            .is_some_and(|suffix| suffix.starts_with('/'))
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn route_extracts_service_and_forward_path() {
        assert_eq!(
            route_request_path("/proxy", "/proxy/github/repos/acme/app/issues"),
            Some(RouteMatch {
                service_name: "github".to_owned(),
                forward_path: "/repos/acme/app/issues".to_owned(),
                strip_prefix: "/proxy/github".to_owned(),
            })
        );
        assert_eq!(
            route_request_path("/proxy", "/proxy/github"),
            Some(RouteMatch {
                service_name: "github".to_owned(),
                forward_path: "/".to_owned(),
                strip_prefix: "/proxy/github".to_owned(),
            })
        );
        assert_eq!(route_request_path("/proxy", "/healthz"), None);
    }

    #[test]
    fn path_prefix_matching_is_segment_aware() {
        assert!(path_matches_prefix("/repos/acme", "/repos/acme"));
        assert!(path_matches_prefix("/repos/acme/issues", "/repos/acme"));
        assert!(!path_matches_prefix(
            "/repos/acme-inc/issues",
            "/repos/acme"
        ));
        assert!(path_matches_prefix("/anything", "/"));
    }

    #[test]
    fn encoded_path_separators_are_rejected() {
        assert!(validate_forward_path("/repos/acme/app").is_ok());
        assert!(validate_forward_path("/repos/acme%2Fapp").is_err());
        assert!(validate_forward_path("/repos/../secrets").is_err());
    }
}
