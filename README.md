# HTTP Credential Broker

Small local HTTP proxy for trusted clients that should call named cloud APIs
without seeing the upstream credentials.

The broker is intentionally simple:

- requests go to `/proxy/{service}/...`
- `{service}` must exist in the TOML config
- method and path prefix must be allowed
- caller `Authorization` and `Proxy-Authorization` are stripped by default
- configured auth/header values are injected inside the broker
- arbitrary target URLs are not accepted

## Run

```sh
export GITHUB_TOKEN=...
cargo run -- --config broker.example.toml
```

Then call a configured service:

```sh
curl http://127.0.0.1:8787/proxy/github/repos/adamcavendish/example/issues
```

The upstream receives:

```text
GET https://api.github.com/repos/adamcavendish/example/issues
Authorization: Bearer $GITHUB_TOKEN
Accept: application/vnd.github+json
```

## Config

```toml
listen = "127.0.0.1:8787"
proxy_prefix = "/proxy"

[services.github]
base_url = "https://api.github.com"
allowed_methods = ["GET", "POST"]
allowed_path_prefixes = ["/repos/adamcavendish/"]

[services.github.auth]
type = "bearer"
token_env = "GITHUB_TOKEN"
```

Supported auth types:

```toml
[services.example.auth]
type = "bearer"
token_env = "API_TOKEN"
```

```toml
[services.example.auth]
type = "basic"
username = "user"
password_env = "API_PASSWORD"
```

```toml
[services.example.auth]
type = "header"
name = "x-api-key"
value_env = "API_KEY"
```

Extra headers:

```toml
[[services.example.headers]]
name = "Accept"
value = "application/json"
```

The broker fails startup when a referenced env var is missing or empty.

## Container

```sh
docker build -t http-credential-broker:local .
docker run --rm \
  -p 127.0.0.1:8787:8787 \
  -v "$PWD/broker.example.toml:/etc/http-credential-broker/broker.toml:ro" \
  -e GITHUB_TOKEN \
  http-credential-broker:local
```

Tagged releases publish container images to GitHub Container Registry through
the release artifacts workflow.

## Local Checks

```sh
just ci
```

## Release Checks

```sh
just dist-plan
```
