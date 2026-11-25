//! Captured exchanges and their body size limits.

use serde::Serialize;

pub const MAX_BODY_BYTES: usize = 5_000_000;
pub const MAX_TOTAL_BODY_BYTES: usize = 25_000_000;
pub const MAX_ENTRIES: usize = 1_000;
#[derive(Debug, Clone, Serialize)]
pub struct Header {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CapturedBody {
    /// Decoded size in bytes, when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<usize>,
    pub mime_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// The body would have been returned but a size limit or a failure kept it out.
    pub truncated: bool,
    /// Why there is no text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub omitted: Option<String>,
}

pub const NOT_RETURNED: &str =
    "not returned: only JSON, form and multipart bodies are captured as text";
pub const TOO_LARGE: &str = "larger than the 5 MB per-body limit";
pub const BUDGET_USED: &str = "the capture's 25 MB body budget was used up";
pub const UNAVAILABLE: &str = "Chrome no longer had the body";
pub const UNSUPPORTED_ENCODING: &str = "its content-encoding can't be decoded";

/// Body types eligible to be captured as text.
pub fn is_returnable_mime_type(mime: Option<&str>) -> bool {
    let Some(mime) = mime else { return false };
    let mime = mime.to_ascii_lowercase();
    mime.contains("json") || mime.contains("application/x-www-form-urlencoded") || mime.contains("multipart/form-data")
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CapturedRequest {
    pub method: String,
    pub url: String,
    pub headers: Vec<Header>,
    pub resource_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<CapturedBody>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub redirected_from: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CapturedResponse {
    pub status: i64,
    pub status_text: String,
    pub headers: Vec<Header>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<CapturedBody>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CapturedEntry {
    pub id: String,
    pub started_date_time: String,
    pub duration_ms: u64,
    pub request: CapturedRequest,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response: Option<CapturedResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure: Option<String>,
}

pub fn header<'a>(headers: &'a [Header], name: &str) -> Option<&'a str> {
    headers.iter().find(|header| header.name.eq_ignore_ascii_case(name)).map(|header| header.value.as_str())
}

pub fn content_type(headers: &[Header]) -> Option<String> {
    header(headers, "content-type").map(|value| value.trim().to_string())
}

pub fn declared_body_size(headers: &[Header]) -> Option<usize> {
    header(headers, "content-length").and_then(|value| value.trim().parse().ok())
}

/// Whether the encoding can be decoded and a body of `size` bytes (declared or measured) fits in `limit`; `Err` says
/// why not.
pub fn body_can_fit(headers: &[Header], size: Option<usize>, limit: usize) -> Result<(), (&'static str, bool)> {
    let encoding = header(headers, "content-encoding").map(|value| value.trim().to_ascii_lowercase());
    if !matches!(encoding.as_deref(), None | Some("identity" | "gzip" | "br" | "deflate" | "zstd")) {
        return Err((UNSUPPORTED_ENCODING, true));
    }
    match size.or_else(|| declared_body_size(headers)) {
        Some(size) if size > MAX_BODY_BYTES => Err((TOO_LARGE, true)),
        Some(size) if size > limit => Err((BUDGET_USED, true)),
        _ if limit == 0 => Err((BUDGET_USED, true)),
        _ => Ok(()),
    }
}

/// Body budget shared by one capture.
#[derive(Default)]
pub struct Budget {
    pub captured_body_bytes: usize,
    pub truncated_body_count: usize,
}

impl Budget {
    pub fn remaining(&self) -> usize {
        MAX_TOTAL_BODY_BYTES.saturating_sub(self.captured_body_bytes)
    }

    /// Records a fetched returnable body, cut off when it is over the per-body limit or the remaining budget.
    pub fn capture(&mut self, bytes: &[u8], mime: Option<&str>) -> CapturedBody {
        let mime_type = mime.unwrap_or("application/octet-stream").to_string();
        if bytes.len() > MAX_BODY_BYTES {
            return self.omitted(mime, Some(bytes.len()), TOO_LARGE, true);
        }
        if bytes.len() > self.remaining() {
            return self.omitted(mime, Some(bytes.len()), BUDGET_USED, true);
        }
        self.captured_body_bytes += bytes.len();
        CapturedBody {
            size: Some(bytes.len()),
            mime_type,
            text: Some(String::from_utf8_lossy(bytes).into_owned()),
            truncated: false,
            omitted: None,
        }
    }

    /// A body without text; `truncated` ones (kept out by a limit or a failure) are counted.
    pub fn omitted(&mut self, mime: Option<&str>, size: Option<usize>, reason: &str, truncated: bool) -> CapturedBody {
        if truncated {
            self.truncated_body_count += 1;
        }
        CapturedBody {
            size,
            mime_type: mime.unwrap_or("application/octet-stream").to_string(),
            text: None,
            truncated,
            omitted: Some(reason.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bodies_respect_limits() {
        let mut budget = Budget::default();
        let body = budget.capture(b"{}", Some("application/json"));
        assert_eq!(body.text.as_deref(), Some("{}"));
        assert_eq!(budget.captured_body_bytes, 2);
        let mut max_body = vec![b' '; 5_000_000];
        max_body[0] = b'[';
        *max_body.last_mut().unwrap() = b']';
        let body = budget.capture(&max_body, Some("application/json"));
        assert_eq!(body.text.as_ref().map(String::len), Some(5_000_000));
        assert!(!body.truncated);
        let big = budget.capture(&vec![b' '; MAX_BODY_BYTES + 1], Some("application/json"));
        assert!(big.text.is_none() && big.truncated && big.omitted.as_deref() == Some(TOO_LARGE));
        assert_eq!(budget.captured_body_bytes, 5_000_002, "bodies that aren't returned don't use the budget");
        assert!(is_returnable_mime_type(Some("application/problem+json")));
        assert!(!is_returnable_mime_type(Some("text/html")) && !is_returnable_mime_type(None));
        let length = |n: &str| vec![Header { name: "content-length".into(), value: n.into() }];
        assert_eq!(body_can_fit(&length("5000000"), None, MAX_TOTAL_BODY_BYTES), Ok(()));
        assert_eq!(body_can_fit(&length("5000001"), None, MAX_TOTAL_BODY_BYTES), Err((TOO_LARGE, true)));
        assert_eq!(body_can_fit(&length("20"), None, 10), Err((BUDGET_USED, true)));
        assert_eq!(body_can_fit(&length("5"), None, 10), Ok(()));
        let compress = vec![Header { name: "content-encoding".into(), value: "compress".into() }];
        assert_eq!(body_can_fit(&compress, Some(5), 10), Err((UNSUPPORTED_ENCODING, true)));
    }
}
