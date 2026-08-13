use act_sdk::cbor::to_cbor;
use act_sdk::prelude::*;

use std::collections::HashMap;
use std::time::Duration;

// Component-specific metadata keys
const META_HTTP_STATUS: &str = "http-client:status";
const META_HTTP_HEADERS: &str = "http-client:headers";

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

/// A URL that cannot serve as a request target — a parse failure, a
/// scheme-less relative form — is the caller's argument being wrong, not
/// a transport failure: `std:invalid-args`. hclient 0.1.0-alpha.18 files
/// exactly that class under `ErrorKind::Uri` (before that kind existed
/// this went through `source()`-downcasting of `UriError`, and before
/// that through `wasi_fetch::Error::Url`, which a pre-send
/// `http::Uri::try_from` check used to imitate — that check also blocked
/// IDN hosts, which `idn` punycodes, so it is gone). One drift against
/// the wasi-fetch behaviour stays: a *resolution* failure (bad host)
/// lands in `invalid_args` too, because hclient folds resolve errors
/// into the same kind.
fn classify_send_error(e: hclient::Error) -> ActError {
    if matches!(e.kind(), hclient::ErrorKind::Uri) {
        ActError::invalid_args(e.to_string())
    } else {
        ActError::internal(format!("HTTP error: {e}"))
    }
}

#[act_component]
mod component {
    use super::*;

    #[act_tool(description = "Make an HTTP request")]
    async fn fetch(#[args] args: FetchArgs, ctx: &mut ActContext) -> ActResult<()> {
        let client = hclient::Client::builder(hclient_wasi::WasiHttp::new())
            .build()
            .map_err(|e| ActError::internal(format!("Cannot build HTTP client: {e}")))?;

        // Redirects: follow up to 10 hops — `Limit`'s default, and the
        // limit wasi-fetch was asked for here — or, with
        // `follow_redirects = false`, hand the 3xx response back to the
        // caller. That is `Forbid`, not `Limit::new(0)`: the latter turns
        // the first redirect into an *error*, the former is "here is the
        // redirect response", which is what wasi-fetch's
        // `redirect_limit(0)` did. Two branches, not one `if` over the
        // policy value: `Limit` and `Forbid` are distinct concrete types
        // and `.redirect()` takes one `P: RedirectPolicy`.
        let mut builder = client.request(args.method.clone(), &args.url);
        builder = if args.follow_redirects {
            builder.redirect(hclient::redirect::Limit::default())
        } else {
            builder.redirect(hclient::redirect::Forbid)
        };

        // Set headers
        for (k, v) in &args.headers {
            builder = builder.header(k.as_str(), v.as_str());
        }

        // Set body
        if let Some(body) = args.body {
            // Auto-set Content-Type for JSON if not already set
            if body.is_json()
                && !args
                    .headers
                    .keys()
                    .any(|k| k.eq_ignore_ascii_case("content-type"))
            {
                builder = builder.header("content-type", "application/json");
            }
            builder = builder.body(hclient::RequestBody::Full(bytes::Bytes::from(
                body.into_bytes(),
            )));
        }

        // Set timeout. `wasi_fetch::RequestBuilder::timeout` put one
        // `Duration` into the wasip3 `connect` and `first_byte` options
        // together; `hclient::Timeouts` keeps them as two fields, so both
        // get the same value here or the connect timeout would be
        // silently dropped. `Timeouts` is `#[non_exhaustive]` — start
        // from the default and set what we mean.
        if let Some(ms) = args.timeout_ms {
            let d = Duration::from_millis(ms);
            builder = builder.timeouts({
                let mut timeouts = hclient::Timeouts::default();
                timeouts.connect = Some(d);
                timeouts.first_byte = Some(d);
                timeouts
            });
        }

        let mut response = builder.send().await.map_err(classify_send_error)?;

        let status = response.status().as_u16();
        let resp_headers = response.headers().clone();
        let content_type = resp_headers
            .get(http::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());

        // Stream response body chunks — read straight off the response,
        // no `into_body` step like wasi-fetch needed.
        let mut first_chunk = true;

        while let Some(chunk) = response.chunk().await {
            let chunk = chunk.map_err(|e| ActError::internal(format!("HTTP error: {e}")))?;
            let metadata = if first_chunk {
                first_chunk = false;
                status_headers_metadata(status, &resp_headers)
            } else {
                vec![]
            };
            ctx.send_content(chunk.to_vec(), content_type.clone(), metadata);
        }

        // If no body was received, still send status/headers
        if first_chunk {
            ctx.send_content(
                vec![],
                content_type.clone(),
                status_headers_metadata(status, &resp_headers),
            );
        }

        Ok(())
    }
}
