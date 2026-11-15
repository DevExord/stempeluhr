// SPDX-License-Identifier: GPL-3.0-or-later
use url::Url;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UrlError {
    Empty,
    Invalid,
    UnsupportedScheme,
    HasCredentials,
}

/// Turns whatever the user typed into the canonical base URL of the Kimai
/// installation: scheme present (default `https`), no trailing slash, no
/// query/fragment, and no accidentally appended `/api` or `/api/doc`.
/// Installations in a sub-directory (`https://host/kimai`) are preserved.
pub fn normalize_base_url(input: &str) -> Result<String, UrlError> {
    let input = input.trim();
    if input.is_empty() {
        return Err(UrlError::Empty);
    }
    let with_scheme = if input.contains("://") {
        input.to_string()
    } else {
        format!("https://{input}")
    };
    let mut url = Url::parse(&with_scheme).map_err(|_| UrlError::Invalid)?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(UrlError::UnsupportedScheme);
    }
    if url.host_str().is_none_or(str::is_empty) {
        return Err(UrlError::Invalid);
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(UrlError::HasCredentials);
    }
    url.set_query(None);
    url.set_fragment(None);

    let mut segments: Vec<String> = url
        .path_segments()
        .map(|s| s.filter(|p| !p.is_empty()).map(str::to_owned).collect())
        .unwrap_or_default();
    // Strip an appended API suffix: ".../api", ".../api/doc", ".../api/doc.json"
    if let Some(pos) = segments.iter().rposition(|s| s.eq_ignore_ascii_case("api")) {
        let tail = &segments[pos + 1..];
        if tail.len() <= 1 {
            segments.truncate(pos);
        }
    }
    let path = if segments.is_empty() {
        String::new()
    } else {
        format!("/{}", segments.join("/"))
    };
    let mut out = format!("{}://{}", url.scheme(), url.host_str().unwrap_or_default());
    if let Some(port) = url.port() {
        out.push_str(&format!(":{port}"));
    }
    out.push_str(&path);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adds_scheme_and_strips_slash() {
        assert_eq!(
            normalize_base_url("kimai.example.com/").unwrap(),
            "https://kimai.example.com"
        );
    }

    #[test]
    fn strips_api_suffixes() {
        for s in [
            "https://k.example.com/api",
            "https://k.example.com/api/",
            "https://k.example.com/api/doc",
            "https://k.example.com/api/doc.json",
            "  https://k.example.com/?x=1#y ",
        ] {
            assert_eq!(
                normalize_base_url(s).unwrap(),
                "https://k.example.com",
                "{s}"
            );
        }
    }

    #[test]
    fn keeps_subdirectory_and_port() {
        assert_eq!(
            normalize_base_url("http://localhost:8001/kimai/api/").unwrap(),
            "http://localhost:8001/kimai"
        );
    }

    #[test]
    fn rejects_garbage() {
        assert_eq!(normalize_base_url(""), Err(UrlError::Empty));
        assert_eq!(
            normalize_base_url("ftp://x.org"),
            Err(UrlError::UnsupportedScheme)
        );
        assert_eq!(
            normalize_base_url("https://u:p@x.org"),
            Err(UrlError::HasCredentials)
        );
        assert_eq!(normalize_base_url("https://"), Err(UrlError::Invalid));
    }
}
