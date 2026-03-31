use act_sdk::cbor::to_cbor;
use act_sdk::prelude::*;

use std::collections::HashMap;

act_sdk::embed_skill!("skill/");

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

#[act_component]
mod component {
    use super::*;

    #[act_tool(description = "Make an HTTP request")]
    async fn fetch(#[args] args: FetchArgs, ctx: &mut ActContext) -> ActResult<()> {
        let redirect_limit = if args.follow_redirects { 10 } else { 0 };

        let mut builder = wasi_fetch::Client::new()
            .request(args.method.clone(), &args.url)
            .redirect_limit(redirect_limit);

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
            builder = builder.body(body.into_bytes());
        }

        // Set timeout
        if let Some(ms) = args.timeout_ms {
            builder = builder.timeout(std::time::Duration::from_millis(ms));
        }

        let response = builder.send().await.map_err(|e| match e {
            wasi_fetch::Error::Url(msg) => ActError::invalid_args(msg),
            other => ActError::internal(format!("HTTP error: {other}")),
        })?;

        let status = response.status().as_u16();
        let resp_headers = response.headers().clone();
        let content_type = resp_headers
            .get(http::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());

        // Stream response body chunks
        let mut body = response.into_body();
        let mut first_chunk = true;

        while let Some(chunk) = body.chunk().await {
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
