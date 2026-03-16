use act_sdk::cbor::to_cbor;
use act_sdk::prelude::*;

use http::Uri;
use http::header;
use std::collections::HashMap;
use wasip3::http::types::{ErrorCode, Fields, Method, Request, RequestOptions, Response, Scheme};

// Component-specific metadata keys
const META_HTTP_STATUS: &str = "http-client:status";
const META_HTTP_HEADERS: &str = "http-client:headers";
const META_HTTP_TRAILERS: &str = "http-client:trailers";

#[serde_with::serde_as]
#[derive(Clone, Deserialize, JsonSchema)]
#[serde(untagged)]
enum Body {
    /// Raw binary request body (CBOR byte string)
    Raw {
        #[serde_as(as = "serde_with::Bytes")]
        #[schemars(with = "Vec<u8>")]
        body_raw: Vec<u8>,
    },
    /// JSON request body. Auto-serialized, auto-sets Content-Type.
    Json { body_json: serde_json::Value },
    /// Text request body (UTF-8)
    Text { body: String },
}

impl Body {
    fn into_bytes(self) -> Vec<u8> {
        match self {
            Body::Raw { body_raw } => body_raw,
            Body::Json { body_json } => serde_json::to_vec(&body_json).unwrap(),
            Body::Text { body } => body.into_bytes(),
        }
    }

    fn is_json(&self) -> bool {
        matches!(self, Body::Json { .. })
    }
}

#[derive(Deserialize, JsonSchema)]
struct FetchArgs {
    /// URL to fetch
    url: String,
    /// HTTP method (default GET)
    #[serde(default = "default_method", with = "http_serde::method")]
    #[schemars(with = "String")]
    method: http::Method,
    /// Request headers as key-value pairs
    #[serde(default)]
    headers: HashMap<String, String>,
    /// Request body (provide one of body_raw, body_json, or body)
    #[serde(flatten)]
    body: Option<Body>,
    /// Request timeout in milliseconds
    timeout_ms: Option<u64>,
    /// Whether to follow redirects (default true)
    #[serde(default = "default_true")]
    follow_redirects: bool,
}

fn default_method() -> http::Method {
    http::Method::GET
}

fn default_true() -> bool {
    true
}

const MAX_REDIRECTS: u8 = 10;

/// Convert an http::Method to a WASI HTTP Method.
fn to_wasi_method(m: &http::Method) -> Method {
    match *m {
        http::Method::GET => Method::Get,
        http::Method::POST => Method::Post,
        http::Method::PUT => Method::Put,
        http::Method::DELETE => Method::Delete,
        http::Method::PATCH => Method::Patch,
        http::Method::HEAD => Method::Head,
        http::Method::OPTIONS => Method::Options,
        _ => Method::Other(m.to_string()),
    }
}

fn build_request_options(timeout_ms: Option<u64>) -> Option<RequestOptions> {
    timeout_ms.map(|ms| {
        let ns = ms * 1_000_000;
        let opts = RequestOptions::new();
        let _ = opts.set_connect_timeout(Some(ns));
        let _ = opts.set_first_byte_timeout(Some(ns));
        opts
    })
}

fn build_request(
    method: &Method,
    uri: &Uri,
    headers: &Fields,
    body: Option<Vec<u8>>,
    options: Option<RequestOptions>,
) -> ActResult<Request> {
    let scheme = match uri.scheme_str() {
        Some("https") => Scheme::Https,
        Some("http") => Scheme::Http,
        Some(other) => {
            return Err(ActError::invalid_args(format!(
                "Unsupported scheme: {other}"
            )));
        }
        None => return Err(ActError::invalid_args("Missing scheme")),
    };

    let body_contents = if let Some(body_bytes) = body {
        let (mut body_writer, body_reader) = wasip3::wit_stream::new::<u8>();
        wit_bindgen::spawn(async move {
            body_writer.write_all(body_bytes).await;
        });
        Some(body_reader)
    } else {
        None
    };

    let (_, trailers_reader) =
        wasip3::wit_future::new::<Result<Option<Fields>, ErrorCode>>(|| Ok(None));

    let (request, _) = Request::new(headers.clone(), body_contents, trailers_reader, options);
    let _ = request.set_method(method);
    let _ = request.set_scheme(Some(&scheme));

    if let Some(authority) = uri.authority() {
        let _ = request.set_authority(Some(authority.as_str()));
    }

    let _ = request.set_path_with_query(uri.path_and_query().map(|pq| pq.as_str()));

    Ok(request)
}

fn fields_to_header_map(fields: &Fields) -> http::HeaderMap {
    let mut map = http::HeaderMap::new();
    for (name, value) in fields.copy_all() {
        if let (Ok(name), Ok(value)) = (
            http::HeaderName::from_bytes(name.as_bytes()),
            http::HeaderValue::from_bytes(&value),
        ) {
            map.append(name, value);
        }
    }
    map
}

/// Serialize a HeaderMap to CBOR via http_serde.
fn header_map_to_cbor(map: &http::HeaderMap) -> Vec<u8> {
    #[derive(serde::Serialize)]
    struct Wrapper<'a>(#[serde(with = "http_serde::header_map")] &'a http::HeaderMap);
    to_cbor(&Wrapper(map))
}

fn status_headers_metadata(status: u16, headers: &http::HeaderMap) -> Vec<(String, Vec<u8>)> {
    vec![
        (META_HTTP_STATUS.to_string(), to_cbor(&status)),
        (META_HTTP_HEADERS.to_string(), header_map_to_cbor(headers)),
    ]
}

/// Resolve a redirect Location against the current URI.
fn resolve_redirect(base: &Uri, location: &str) -> ActResult<Uri> {
    let parse = |s: &str| -> ActResult<Uri> {
        s.parse()
            .map_err(|e| ActError::internal(format!("Invalid redirect URL: {e}")))
    };

    // Absolute URI
    if location.contains("://") {
        return parse(location);
    }

    // Protocol-relative (//host/path)
    if let Some(rest) = location.strip_prefix("//") {
        let scheme = base.scheme_str().unwrap_or("https");
        return parse(&format!("{scheme}://{rest}"));
    }

    // Absolute or relative path — keep scheme + authority from base
    let path = if location.starts_with('/') {
        location.to_string()
    } else {
        let base_path = base.path();
        let parent = base_path.rfind('/').map_or("", |i| &base_path[..=i]);
        format!("{parent}{location}")
    };

    let mut parts = base.clone().into_parts();
    parts.path_and_query = Some(
        path.parse()
            .map_err(|e| ActError::internal(format!("Invalid redirect path: {e}")))?,
    );
    Uri::from_parts(parts).map_err(|e| ActError::internal(format!("Invalid redirect URL: {e}")))
}

#[act_component(
    name = "http-client",
    version = "0.1.0",
    description = "HTTP client ACT component"
)]
mod component {
    use super::*;

    #[act_tool(description = "Make an HTTP request")]
    async fn fetch(args: FetchArgs, ctx: &mut ActContext) -> ActResult<()> {
        let mut method = to_wasi_method(&args.method);
        let mut current_uri: Uri = args
            .url
            .parse()
            .map_err(|e| ActError::invalid_args(format!("Invalid URL: {e}")))?;
        let mut redirects = 0u8;

        let response = loop {
            let mut header_list: Vec<(String, Vec<u8>)> = args
                .headers
                .iter()
                .map(|(k, v)| (k.clone(), v.as_bytes().to_vec()))
                .collect();
            if args.body.as_ref().is_some_and(|b| b.is_json())
                && !args
                    .headers
                    .keys()
                    .any(|k| k.eq_ignore_ascii_case(header::CONTENT_TYPE.as_str()))
            {
                header_list.push((
                    header::CONTENT_TYPE.as_str().to_string(),
                    b"application/json".to_vec(),
                ));
            }
            let headers = Fields::from_list(&header_list).unwrap();

            let body_bytes = args.body.clone().map(Body::into_bytes);

            let request = build_request(
                &method,
                &current_uri,
                &headers,
                body_bytes,
                build_request_options(args.timeout_ms),
            )?;

            let resp = wasip3::http::client::send(request)
                .await
                .map_err(|e| ActError::internal(format!("HTTP error: {e:?}")))?;

            let status = http::StatusCode::from_u16(resp.get_status_code())
                .unwrap_or(http::StatusCode::INTERNAL_SERVER_ERROR);

            if args.follow_redirects && status.is_redirection() {
                redirects += 1;
                if redirects > MAX_REDIRECTS {
                    return Err(ActError::internal("Too many redirects"));
                }
                let resp_headers = fields_to_header_map(&resp.get_headers());
                if let Some(location) = resp_headers.get(header::LOCATION) {
                    let location_str = location
                        .to_str()
                        .map_err(|e| ActError::internal(format!("Invalid Location header: {e}")))?;
                    current_uri = resolve_redirect(&current_uri, location_str)?;
                    if status == http::StatusCode::SEE_OTHER {
                        method = Method::Get;
                    }
                    continue;
                }
            }

            break resp;
        };

        let status = response.get_status_code();
        let resp_headers = fields_to_header_map(&response.get_headers());
        let content_type = resp_headers
            .get(header::CONTENT_TYPE)
            .map(|v| v.to_str().unwrap_or("").to_string());

        let (_, result_reader) = wasip3::wit_future::new::<Result<(), ErrorCode>>(|| Ok(()));

        let (mut body_stream, trailers_future) = Response::consume_body(response, result_reader);

        let mut first_chunk = true;
        let mut read_buf = Vec::with_capacity(16384);
        loop {
            let (result, chunk) = body_stream.read(read_buf).await;
            match result {
                wasip3::wit_bindgen::StreamResult::Complete(_) => {
                    let metadata = if first_chunk {
                        first_chunk = false;
                        status_headers_metadata(status, &resp_headers)
                    } else {
                        vec![]
                    };
                    ctx.send_content(chunk, content_type.clone(), metadata);
                    read_buf = Vec::with_capacity(16384);
                }
                wasip3::wit_bindgen::StreamResult::Dropped
                | wasip3::wit_bindgen::StreamResult::Cancelled => break,
            }
        }

        if first_chunk {
            ctx.send_content(
                vec![],
                content_type.clone(),
                status_headers_metadata(status, &resp_headers),
            );
        }

        if let Ok(Some(trailers)) = trailers_future.await {
            let trailers = fields_to_header_map(&trailers);
            let metadata = vec![(
                META_HTTP_TRAILERS.to_string(),
                header_map_to_cbor(&trailers),
            )];
            ctx.send_content(vec![], None, metadata);
        }

        Ok(())
    }
}
