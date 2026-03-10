use act_sdk::prelude::*;

use std::collections::HashMap;
use wasip3::http::types::{ErrorCode, Fields, Method, Request, Response, Scheme};

#[derive(Deserialize, JsonSchema)]
struct FetchArgs {
    /// URL to fetch
    url: String,
    /// HTTP method (default GET)
    #[serde(default = "default_method")]
    method: String,
    /// Request headers as key-value pairs
    #[serde(default)]
    headers: HashMap<String, String>,
    /// Request body (for POST/PUT/PATCH)
    body: Option<String>,
}

fn default_method() -> String {
    "GET".to_string()
}

#[act_component(
    name = "http-client",
    version = "0.1.0",
    description = "HTTP client ACT component",
)]
mod component {
    use super::*;

    #[act_tool(description = "Make an HTTP request")]
    async fn fetch(args: FetchArgs) -> ActResult<serde_json::Value> {
        let method = match args.method.as_str() {
            "GET" => Method::Get,
            "POST" => Method::Post,
            "PUT" => Method::Put,
            "DELETE" => Method::Delete,
            "PATCH" => Method::Patch,
            "HEAD" => Method::Head,
            "OPTIONS" => Method::Options,
            "QUERY" => Method::Other("QUERY".to_string()),
            other => return Err(ActError::invalid_args(format!("Unsupported method: {other}"))),
        };

        let parsed = url::Url::parse(&args.url)
            .map_err(|e| ActError::invalid_args(format!("Invalid URL: {e}")))?;

        let scheme = match parsed.scheme() {
            "https" => Scheme::Https,
            "http" => Scheme::Http,
            other => return Err(ActError::invalid_args(format!("Unsupported scheme: {other}"))),
        };

        // Build headers
        let header_list: Vec<(String, Vec<u8>)> = args
            .headers
            .iter()
            .map(|(k, v)| (k.clone(), v.as_bytes().to_vec()))
            .collect();
        let headers = Fields::from_list(&header_list).unwrap();

        // Build body stream
        let body_contents = if let Some(body_str) = &args.body {
            let (mut body_writer, body_reader) = wasip3::wit_stream::new::<u8>();
            let body_bytes = body_str.as_bytes().to_vec();
            wit_bindgen::spawn(async move {
                body_writer.write_all(body_bytes).await;
            });
            Some(body_reader)
        } else {
            None
        };

        // Trailers: none
        let (trailers_writer, trailers_reader) =
            wasip3::wit_future::new::<Result<Option<Fields>, ErrorCode>>(|| Ok(None));
        drop(trailers_writer);

        let (request, _completion) = Request::new(headers, body_contents, trailers_reader, None);
        let _ = request.set_method(&method);
        let _ = request.set_scheme(Some(&scheme));

        let authority = match parsed.port() {
            Some(port) => format!("{}:{}", parsed.host_str().unwrap_or(""), port),
            None => parsed.host_str().unwrap_or("").to_string(),
        };
        let _ = request.set_authority(Some(&authority));

        let path_with_query = match parsed.query() {
            Some(q) => format!("{}?{q}", parsed.path()),
            None => parsed.path().to_string(),
        };
        let _ = request.set_path_with_query(Some(&path_with_query));

        // Send request
        let response = wasip3::http::client::send(request)
            .await
            .map_err(|e| ActError::internal(format!("HTTP error: {e:?}")))?;

        let status = response.get_status_code();

        // Consume response body
        let (result_writer, result_reader) =
            wasip3::wit_future::new::<Result<(), ErrorCode>>(|| Ok(()));
        drop(result_writer);

        let (mut body_stream, _trailers) = Response::consume_body(response, result_reader);

        let mut body_bytes = Vec::new();
        loop {
            let (result, chunk) = body_stream.read(Vec::with_capacity(16384)).await;
            match result {
                wasip3::wit_bindgen::StreamResult::Complete(_) => {
                    body_bytes.extend_from_slice(&chunk);
                }
                wasip3::wit_bindgen::StreamResult::Dropped
                | wasip3::wit_bindgen::StreamResult::Cancelled => break,
            }
        }

        let body_text = String::from_utf8_lossy(&body_bytes);
        Ok(serde_json::json!({
            "status": status,
            "body": body_text,
        }))
    }
}
