//! Library of Congress authorities via id.loc.gov: subject headings (LCSH)
//! and the name authority file (LCNAF).
//!
//! id.loc.gov answers both directions with a HEAD request and headers only:
//! - `/authorities/<scheme>/label/<heading>` redirects for an exact match and
//!   sends `X-Uri` (404 when there's none)
//! - `/authorities/<scheme>/<id>` sends `X-PrefLabel`

use super::{Authority, Resolved};
use anyhow::{Context, Result};
use reqwest::blocking::Client;
use reqwest::redirect::Policy;
use std::thread::sleep;
use std::time::{Duration, Instant};
use std::cell::Cell;

const BASE: &str = "https://id.loc.gov/authorities";
/// Be polite: id.loc.gov is a shared public service.
const MIN_GAP: Duration = Duration::from_millis(150);

pub struct Lc {
    scheme: &'static str,
    source: &'static str,
    prefixes: &'static [&'static str],
    client: Client,
    last: Cell<Option<Instant>>,
}

impl Lc {
    fn new(scheme: &'static str, source: &'static str, prefixes: &'static [&'static str]) -> Self {
        let client = Client::builder()
            .user_agent(concat!("sacda-organise/", env!("CARGO_PKG_VERSION")))
            .redirect(Policy::none())
            .timeout(Duration::from_secs(20))
            .build()
            .expect("http client");
        Self {
            scheme,
            source,
            prefixes,
            client,
            last: Cell::new(None),
        }
    }

    pub fn subjects() -> Self {
        // sh = topical, sj = children's, gf = genre/form; all live under subjects.
        Self::new("subjects", "lcsh", &["sh", "sj"])
    }

    pub fn names() -> Self {
        Self::new("names", "naf", &["nb", "no", "nr", "n"])
    }

    fn pace(&self) {
        if let Some(t) = self.last.get() {
            let since = t.elapsed();
            if since < MIN_GAP {
                sleep(MIN_GAP - since);
            }
        }
        self.last.set(Some(Instant::now()));
    }

    /// The bare identifier from `sh85005357`, an id.loc.gov URL, or `info:lc/...`.
    fn identifier_of<'a>(&self, value: &'a str) -> Option<&'a str> {
        let v = value.trim().trim_end_matches(".html").trim_end_matches(".json");
        let marker = format!("/authorities/{}/", self.scheme);
        let id = match v.find(&marker) {
            Some(i) => &v[i + marker.len()..],
            None => v,
        };
        let id = id.trim_end_matches('/');
        let prefix = self.prefixes.iter().find(|p| id.starts_with(*p))?;
        let digits = &id[prefix.len()..];
        (digits.len() >= 6 && digits.bytes().all(|b| b.is_ascii_digit())).then_some(id)
    }

    fn header(resp: &reqwest::blocking::Response, name: &str) -> Option<String> {
        resp.headers()
            .get(name)
            .and_then(|h| h.to_str().ok())
            .map(percent_decode)
    }
}

/// X-PrefLabel can arrive percent-encoded for non-ASCII headings.
fn percent_decode(s: &str) -> String {
    url::form_urlencoded::parse(format!("x={}", s.replace('+', "%2B")).as_bytes())
        .next()
        .map(|(_, v)| v.into_owned())
        .unwrap_or_else(|| s.to_string())
}

impl Authority for Lc {
    fn name(&self) -> &str {
        if self.scheme == "subjects" {
            "lcsh"
        } else {
            "lcnaf"
        }
    }

    fn is_identifier(&self, value: &str) -> bool {
        self.identifier_of(value).is_some()
    }

    fn resolve(&self, value: &str) -> Result<Option<Resolved>> {
        if let Some(id) = self.identifier_of(value) {
            self.pace();
            let url = format!("{BASE}/{}/{id}", self.scheme);
            let resp = self.client.head(&url).send().with_context(|| format!("HEAD {url}"))?;
            if resp.status().as_u16() == 404 {
                return Ok(None);
            }
            let label = Self::header(&resp, "x-preflabel");
            return Ok(label.map(|label| Resolved {
                label,
                uri: format!("http://id.loc.gov/authorities/{}/{id}", self.scheme),
                source: self.source.into(),
                from_identifier: true,
            }));
        }

        let heading = value.trim();
        if heading.is_empty() {
            return Ok(None);
        }
        self.pace();
        let url = format!(
            "{BASE}/{}/label/{}",
            self.scheme,
            urlencoding_path(heading)
        );
        let resp = self.client.head(&url).send().with_context(|| format!("HEAD {url}"))?;
        if !resp.status().is_redirection() && !resp.status().is_success() {
            return Ok(None);
        }
        let uri = match Self::header(&resp, "x-uri") {
            Some(u) => u,
            None => return Ok(None),
        };
        // Use LC's own spelling of the heading so the term matches the authority.
        let label = Self::header(&resp, "x-preflabel").unwrap_or_else(|| heading.to_string());
        Ok(Some(Resolved {
            label,
            uri,
            source: self.source.into(),
            from_identifier: false,
        }))
    }
}

fn urlencoding_path(s: &str) -> String {
    // Path segment encoding: spaces as %20, not '+'.
    url::form_urlencoded::byte_serialize(s.as_bytes())
        .collect::<String>()
        .replace('+', "%20")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_identifiers() {
        let s = Lc::subjects();
        assert!(s.is_identifier("sh85005357"));
        assert!(s.is_identifier("http://id.loc.gov/authorities/subjects/sh85005357.html"));
        assert!(!s.is_identifier("Anniversaries"));
        assert!(!s.is_identifier("shoes"));
        let n = Lc::names();
        assert!(n.is_identifier("n79082354"));
        assert!(!n.is_identifier("sh85005357"));
    }

    /// Hits id.loc.gov; run with `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn live_lookups() {
        let s = Lc::subjects();
        let by_label = s.resolve("Anniversaries").unwrap().unwrap();
        assert!(by_label.uri.ends_with("sh85005357"));
        let by_id = s.resolve("sh85005357").unwrap().unwrap();
        assert_eq!(by_id.label, "Anniversaries");
        assert!(s.resolve("Notarealheadingxyzzy").unwrap().is_none());
        let n = Lc::names().resolve("Simon Fraser University").unwrap().unwrap();
        assert!(n.uri.contains("/names/n"));
    }
}
