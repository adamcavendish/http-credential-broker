use bytes::Bytes;
use http::header::HeaderValue;
use http::{Response, StatusCode};
use http_body_util::{BodyExt, Full};

pub(crate) type BoxError = Box<dyn std::error::Error + Send + Sync>;
pub(crate) type ResponseBody = http_body_util::combinators::UnsyncBoxBody<Bytes, BoxError>;

pub(crate) fn upstream_response(resp: aioduct::response::Response) -> Response<ResponseBody> {
    let status = resp.status();
    let version = resp.version();
    let headers = resp.headers().clone();
    let body = resp
        .into_body()
        .map_err(|err| -> BoxError { Box::new(err) })
        .boxed_unsync();
    let mut response = Response::new(body);
    *response.status_mut() = status;
    *response.version_mut() = version;
    *response.headers_mut() = headers;
    response
}

pub(crate) fn text_response(status: StatusCode, body: impl Into<Bytes>) -> Response<ResponseBody> {
    let body = Full::new(body.into())
        .map_err(|never| match never {})
        .map_err(|err| -> BoxError { err })
        .boxed_unsync();
    let mut response = Response::new(body);
    *response.status_mut() = status;
    response.headers_mut().insert(
        http::header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    response
}
