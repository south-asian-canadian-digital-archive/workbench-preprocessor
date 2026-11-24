//! Read-only client for the Islandora site: JSON:API for vocabularies and node
//! lookups, plus the `/lang-code` export. Nothing here writes to the site -
//! Workbench does the writing.

use super::config::{read_credentials, Config, SiteCfg};
use anyhow::{anyhow, bail, Context, Result};
use reqwest::blocking::{Client, Response};
use reqwest::StatusCode;
use serde_json::Value;
use std::collections::HashMap;
use std::time::Duration;

pub const JSONAPI_HELP: &str = "JSON:API is off on this site. Turn it on under Extend \
(/admin/modules, \"JSON:API\") or with `drush en jsonapi`, and keep it read-only at \
/admin/config/services/jsonapi. Docs: \
https://www.drupal.org/docs/core-modules-and-themes/core-modules/jsonapi-module";

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Term {
    pub name: String,
    pub tid: String,
    pub authority_uris: Vec<String>,
    /// Last changed, seconds since 1970 (0 when the site didn't say).
    #[serde(default)]
    pub changed: i64,
}

/// The site's answer to `term_matches`.
#[derive(Debug, Default)]
pub struct TermMatches {
    /// (vocabulary, value) -> the terms it could be.
    pub matches: HashMap<(String, String), Vec<Term>>,
    /// Link -> (vocabulary, term) holding it.
    pub uris: HashMap<String, Vec<(String, Term)>>,
}

impl TermMatches {
    fn from_json(v: &Value) -> Self {
        let term = |t: &Value| Term {
            name: t["name"].as_str().unwrap_or_default().to_string(),
            tid: t["tid"].as_str().map(str::to_string).or_else(|| t["tid"].as_i64().map(|n| n.to_string())).unwrap_or_default(),
            authority_uris: t["uris"].as_array().into_iter().flatten().filter_map(|u| u.as_str().map(str::to_string)).collect(),
            changed: 0,
        };
        let mut out = Self::default();
        for m in v["matches"].as_array().into_iter().flatten() {
            let key = (m["vid"].as_str().unwrap_or_default().to_string(), m["value"].as_str().unwrap_or_default().to_string());
            out.matches.insert(key, m["terms"].as_array().into_iter().flatten().map(term).collect());
        }
        if let Some(o) = v["uris"].as_object() {
            for (uri, list) in o {
                out.uris.insert(
                    uri.clone(),
                    list.as_array().into_iter().flatten().map(|t| (t["vid"].as_str().unwrap_or_default().to_string(), term(t))).collect(),
                );
            }
        }
        out
    }
}

#[derive(Debug, Clone)]
pub struct NodeRef {
    pub nid: String,
    pub bundle: String,
    pub title: String,
}

pub struct Site {
    pub name: String,
    pub cfg: SiteCfg,
    client: Client,
    auth: Option<(String, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Login {
    Ok,
    BadPassword,
    /// 403 with good-looking credentials: usually Drupal's flood control.
    Locked,
    NoCredentials(String),
}

impl Site {
    pub fn new(cfg: &Config, name: Option<&str>) -> Result<Self> {
        let (name, site) = cfg.site(name)?;
        let auth = read_credentials(&cfg.g_root.join(&site.credentials)).ok();
        let client = Client::builder()
            .user_agent(concat!("sacda-organise/", env!("CARGO_PKG_VERSION")))
            .timeout(Duration::from_secs(60))
            .build()?;
        Ok(Self {
            name,
            cfg: site,
            client,
            auth,
        })
    }

    pub fn host(&self) -> &str {
        self.cfg.host.trim_end_matches('/')
    }

    /// GET with basic auth when we have it, retrying 429/5xx (the site caps
    /// concurrent API requests and answers 429 past the cap).
    fn get(&self, url: &str) -> Result<Response> {
        let mut delay = Duration::from_millis(500);
        for attempt in 0..8 {
            if crate::progress::cancelled_here() {
                bail!("cancelled");
            }
            let mut req = self.client.get(url);
            if let Some((u, p)) = &self.auth {
                req = req.basic_auth(u, Some(p));
            }
            match req.send() {
                Ok(r) if r.status() == StatusCode::TOO_MANY_REQUESTS || r.status().is_server_error() => {
                    if attempt == 7 {
                        return Ok(r);
                    }
                }
                Ok(r) => return Ok(r),
                Err(e) if attempt == 7 => return Err(e).with_context(|| format!("GET {url}")),
                // Can't connect at all (no such host, refused): one more try,
                // then give up - waiting out the backoff doesn't bring a site back.
                Err(e) if e.is_connect() && attempt >= 1 => return Err(e).with_context(|| format!("GET {url}")),
                Err(_) => {}
            }
            if !crate::progress::sleep(delay) {
                bail!("cancelled");
            }
            delay = (delay * 2).min(Duration::from_secs(8));
        }
        unreachable!()
    }

    /// JSON:API's next page, rebuilt against our own host: the link is from the
    /// response, and behind a proxy it can name http:// or an internal host -
    /// the credentials go only where they were configured for.
    fn next_page(&self, doc: &Value) -> Option<String> {
        let href = doc["links"]["next"]["href"].as_str()?;
        let at = href.find("/jsonapi/")?;
        Some(format!("{}{}", self.host(), &href[at..]))
    }

    fn get_json(&self, url: &str) -> Result<Value> {
        let r = self.get(url)?;
        let status = r.status();
        if !status.is_success() {
            bail!("{} answered {} for {}", self.host(), status, url);
        }
        r.json().with_context(|| format!("{url} didn't return JSON"))
    }

    pub fn reachable(&self) -> Result<()> {
        self.client
            .get(self.host())
            .send()
            .with_context(|| format!("can't reach {}", self.host()))?;
        Ok(())
    }

    pub fn jsonapi_enabled(&self) -> Result<bool> {
        let r = self.get(&format!("{}/jsonapi", self.host()))?;
        Ok(r.status().is_success())
    }

    /// The media IDs of a node (Islandora's "media of" listing, as the
    /// signed-in user sees it - organise signs in as an admin).
    pub fn media_ids(&self, nid: &str) -> Result<Vec<String>> {
        let url = format!("{}/node/{nid}/media?_format=json", self.host());
        let v = self.get_json(&url)?;
        let list = v.as_array().with_context(|| format!("{url} didn't return a list"))?;
        let ids: Vec<String> = list
            .iter()
            .filter_map(|m| {
                let id = &m["mid"][0]["value"];
                id.as_u64().map(|n| n.to_string()).or_else(|| id.as_str().map(str::to_string))
            })
            .collect();
        // Media whose ID can't be read would look like none at all.
        if ids.len() != list.len() {
            bail!("{url}: couldn't read the IDs of {} media", list.len() - ids.len());
        }
        Ok(ids)
    }

    /// A node as Drupal's REST API gives it (field -> list of values).
    pub fn node_json(&self, nid: &str) -> Result<Value> {
        self.get_json(&format!("{}/node/{nid}?_format=json", self.host()))
    }

    /// Deletes one term: an ingest's rollback, and the only change organise
    /// makes to a site itself (nodes are created and deleted by Workbench).
    /// Already gone counts as done.
    ///
    /// Only when it's still the term the ingest made: `vocab` and `name`
    /// must match, else it's left alone (Ok(false)).
    pub fn delete_term(&self, tid: &str, vocab: &str, name: &str) -> Result<bool> {
        let (user, pass) = self.auth.as_ref().context("no credentials for this site")?;
        let url = format!("{}/taxonomy/term/{tid}?_format=json", self.host());
        let r = self.get(&url)?;
        if r.status() == StatusCode::NOT_FOUND {
            return Ok(true); // already gone
        }
        if !r.status().is_success() {
            bail!("{} answered {} for {url}", self.host(), r.status());
        }
        let v: Value = r.json().with_context(|| format!("{url} didn't return JSON"))?;
        let got_name = v["name"][0]["value"].as_str().unwrap_or("");
        let got_vocab = v["vid"][0]["target_id"].as_str().unwrap_or("");
        if got_name != name || got_vocab != vocab {
            return Ok(false);
        }
        let mut delay = Duration::from_millis(500);
        for attempt in 0..6 {
            let r = self.client.delete(&url).basic_auth(user, Some(pass)).send();
            match r {
                Ok(r) if r.status().is_success() || r.status() == StatusCode::NOT_FOUND => return Ok(true),
                Ok(r) if r.status() != StatusCode::TOO_MANY_REQUESTS && !r.status().is_server_error() => {
                    bail!("{} answered {}", self.host(), r.status())
                }
                Ok(r) if attempt == 5 => bail!("{} answered {}", self.host(), r.status()),
                Err(e) if attempt == 5 => return Err(e).with_context(|| format!("DELETE {url}")),
                _ => {}
            }
            if !crate::progress::sleep(delay) {
                bail!("cancelled");
            }
            delay = (delay * 2).min(Duration::from_secs(8));
        }
        unreachable!()
    }

    /// Logs in with the credentials file by fetching the current user.
    pub fn login(&self) -> Login {
        let Some((user, pass)) = &self.auth else {
            return Login::NoCredentials(self.cfg.credentials.clone());
        };
        // JSON:API's entry point names the signed-in user under meta.links.me;
        // anonymous requests get no `me`. (user/login_status refuses basic auth.)
        let url = format!("{}/jsonapi", self.host());
        let r = self.client.get(&url).basic_auth(user, Some(pass)).send();
        match r {
            Ok(r) => match r.status() {
                s if s.is_success() => {
                    let doc: Value = r.json().unwrap_or(Value::Null);
                    if doc["meta"]["links"]["me"].is_object() {
                        Login::Ok
                    } else {
                        Login::BadPassword
                    }
                }
                StatusCode::FORBIDDEN => Login::Locked,
                _ => Login::BadPassword,
            },
            Err(_) => Login::BadPassword,
        }
    }

    /// The fields a vocabulary's terms have, found the way Workbench finds them
    /// (the default form display's config dependencies). A vocabulary with no
    /// fields of its own has no form display config and 404s.
    pub fn term_fields(&self, vid: &str) -> Result<Vec<String>> {
        let r = self.get(&format!(
            "{}/entity/entity_form_display/taxonomy_term.{vid}.default?_format=json",
            self.host()
        ))?;
        if r.status() == StatusCode::NOT_FOUND {
            return Ok(Vec::new());
        }
        if !r.status().is_success() {
            bail!("{} answered {} for the fields of '{vid}'", self.host(), r.status());
        }
        let doc: Value = r.json()?;
        let prefix = format!("field.field.taxonomy_term.{vid}.");
        Ok(doc["dependencies"]["config"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|d| d.as_str()?.strip_prefix(&prefix).map(String::from))
            .collect())
    }

    /// Every term in a vocabulary, following JSON:API pagination.
    pub fn vocabulary(&self, vid: &str) -> Result<Vec<Term>> {
        self.vocabulary_since(vid, None)
    }

    /// A vocabulary's terms changed at or after `since` (every term with
    /// None). Only the fields organise reads, 50 a page (JSON:API's most), six
    /// pages at a time: the pages are known by their offset.
    pub fn vocabulary_since(&self, vid: &str, since: Option<i64>) -> Result<Vec<Term>> {
        const PAGE: usize = 50;
        const AT_ONCE: usize = 6;
        let filter = since
            .map(|t| format!("&filter%5Bc%5D%5Bcondition%5D%5Bpath%5D=changed&filter%5Bc%5D%5Bcondition%5D%5Boperator%5D=%3E%3D&filter%5Bc%5D%5Bcondition%5D%5Bvalue%5D={t}"))
            .unwrap_or_default();
        let base = format!(
            "{}/jsonapi/taxonomy_term/{vid}?page%5Blimit%5D={PAGE}&sort=drupal_internal__tid\
             &fields%5Btaxonomy_term--{vid}%5D=name,drupal_internal__tid,changed,field_authority_link,field_external_uri{filter}",
            self.host()
        );
        let job = crate::progress::current();
        let mut out = Vec::new();
        let mut offset = 0;
        loop {
            // One page first: most vocabularies fit in it.
            let at_once = if offset == 0 { 1 } else { AT_ONCE };
            let pages: Vec<Result<Vec<Term>>> = std::thread::scope(|sc| {
                let handles: Vec<_> = (0..at_once)
                    .map(|k| {
                        let url = format!("{base}&page%5Boffset%5D={}", offset + k * PAGE);
                        let job = job.clone();
                        sc.spawn(move || {
                            let _job = job.map(crate::progress::enter);
                            self.term_page(&url, vid)
                        })
                    })
                    .collect();
                handles.into_iter().map(|h| h.join().unwrap_or_else(|_| Err(anyhow!("a page fetch panicked")))).collect()
            });
            let mut last = false;
            for p in pages {
                let p = p?;
                last |= p.len() < PAGE;
                out.extend(p);
            }
            if last {
                return Ok(out);
            }
            offset += at_once * PAGE;
        }
    }

    /// Ask the site which existing terms each value could be (the
    /// sacda_organise module's /api/organise/term-matches): one request for
    /// many values, nothing downloaded. None when the site doesn't have the
    /// module yet.
    pub fn term_matches(&self, values: &[(String, String)], uris: &[String]) -> Result<Option<TermMatches>> {
        let (user, pass) = self.auth.as_ref().context("no credentials for this site")?;
        let url = format!("{}/api/organise/term-matches", self.host());
        let body = serde_json::json!({
            "values": values.iter().map(|(vid, value)| serde_json::json!({"vid": vid, "value": value})).collect::<Vec<_>>(),
            "uris": uris,
        });
        let mut delay = Duration::from_millis(500);
        for attempt in 0..6 {
            if crate::progress::cancelled_here() {
                bail!("cancelled");
            }
            match self.client.post(&url).basic_auth(user, Some(pass)).json(&body).send() {
                Ok(r) if r.status() == StatusCode::NOT_FOUND || r.status() == StatusCode::METHOD_NOT_ALLOWED => return Ok(None),
                Ok(r) if r.status().is_success() => {
                    let v: Value = r.json().with_context(|| format!("{url} didn't return JSON"))?;
                    return Ok(Some(TermMatches::from_json(&v)));
                }
                Ok(r) if r.status() != StatusCode::TOO_MANY_REQUESTS && !r.status().is_server_error() => {
                    bail!("{} answered {} for {url}", self.host(), r.status())
                }
                Ok(r) if attempt == 5 => bail!("{} answered {} for {url}", self.host(), r.status()),
                // A timed-out match may still be running on the site: don't send it again.
                Err(e) if attempt == 5 || e.is_timeout() || (e.is_connect() && attempt >= 1) => return Err(e).with_context(|| format!("POST {url}")),
                _ => {}
            }
            if !crate::progress::sleep(delay) {
                bail!("cancelled");
            }
            delay = (delay * 2).min(Duration::from_secs(8));
        }
        unreachable!()
    }

    /// One page of a vocabulary.
    fn term_page(&self, url: &str, vid: &str) -> Result<Vec<Term>> {
        let r = self.get(url)?;
        if r.status() == StatusCode::NOT_FOUND {
            // An unknown vocabulary 404s; so does a site without JSON:API.
            bail!("vocabulary '{vid}' isn't on {} (or JSON:API is off)", self.host());
        }
        if !r.status().is_success() {
            bail!("{} answered {} listing '{vid}'", self.host(), r.status());
        }
        let doc: Value = r.json()?;
        Ok(doc["data"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|item| {
                let attrs = &item["attributes"];
                let mut uris = Vec::new();
                for key in ["field_authority_link", "field_external_uri"] {
                    match &attrs[key] {
                        Value::Array(a) => uris.extend(a.iter().filter_map(|l| l["uri"].as_str().map(String::from))),
                        Value::Object(o) => {
                            if let Some(u) = o.get("uri").and_then(Value::as_str) {
                                uris.push(u.to_string())
                            }
                        }
                        _ => {}
                    }
                }
                Term {
                    name: attrs["name"].as_str().unwrap_or_default().to_string(),
                    tid: attrs["drupal_internal__tid"].as_i64().map(|t| t.to_string()).unwrap_or_default(),
                    authority_uris: uris,
                    changed: attrs["changed"].as_str().and_then(unix_time).unwrap_or(0),
                }
            })
            .collect())
    }

    /// Every archival_component and islandora_object whose identifier starts
    /// with `prefix`, as (identifier, node). One paginated query per bundle, so
    /// a 2,000-row sheet costs a few dozen requests instead of 4,000.
    pub fn nodes_under(&self, prefix: &str) -> Result<Vec<(String, NodeRef)>> {
        let mut out = Vec::new();
        for bundle in ["archival_component", "islandora_object"] {
            let mut url = Some(format!(
                "{}/jsonapi/node/{bundle}?filter%5Bp%5D%5Bcondition%5D%5Bpath%5D=field_identifier\
                 &filter%5Bp%5D%5Bcondition%5D%5Boperator%5D=STARTS_WITH\
                 &filter%5Bp%5D%5Bcondition%5D%5Bvalue%5D={}\
                 &fields%5Bnode--{bundle}%5D=title,drupal_internal__nid,field_identifier\
                 &page%5Blimit%5D=50&sort=drupal_internal__nid",
                self.host(),
                urlencode(prefix)
            ));
            while let Some(u) = url.take() {
                let doc = self.get_json(&u)?;
                for item in doc["data"].as_array().into_iter().flatten() {
                    let a = &item["attributes"];
                    let (Some(id), Some(nid)) = (a["field_identifier"].as_str(), a["drupal_internal__nid"].as_i64())
                    else {
                        continue;
                    };
                    out.push((
                        id.trim().to_string(),
                        NodeRef {
                            nid: nid.to_string(),
                            bundle: bundle.into(),
                            title: a["title"].as_str().unwrap_or_default().into(),
                        },
                    ));
                }
                url = self.next_page(&doc);
            }
        }
        Ok(out)
    }

    /// An existing archival_component or islandora_object with this identifier.
    pub fn find_node(&self, identifier: &str) -> Result<Option<NodeRef>> {
        for bundle in ["archival_component", "islandora_object"] {
            let url = format!(
                "{}/jsonapi/node/{bundle}?filter%5Bfield_identifier%5D={}&fields%5Bnode--{bundle}%5D=title,drupal_internal__nid",
                self.host(),
                urlencode(identifier)
            );
            let doc = self.get_json(&url)?;
            if let Some(first) = doc["data"].as_array().and_then(|a| a.first()) {
                let a = &first["attributes"];
                return Ok(Some(NodeRef {
                    nid: a["drupal_internal__nid"]
                        .as_i64()
                        .map(|n| n.to_string())
                        .ok_or_else(|| anyhow!("node without nid"))?,
                    bundle: bundle.into(),
                    title: a["title"].as_str().unwrap_or_default().into(),
                }));
            }
        }
        Ok(None)
    }

    /// Language code (field_code) -> term ID, from the site's `/lang-code` view.
    pub fn language_codes(&self) -> Result<HashMap<String, String>> {
        let doc = self.get_json(&format!("{}/lang-code?_format=json", self.host()))?;
        let mut map = HashMap::new();
        for t in doc.as_array().into_iter().flatten() {
            if let (Some(code), Some(tid)) = (t["field_code"].as_str(), t["tid"].as_str()) {
                let code = code.trim().to_lowercase();
                if !code.is_empty() {
                    map.entry(code).or_insert_with(|| tid.to_string());
                }
            }
        }
        Ok(map)
    }
}

/// "2026-11-02T19:45:58+00:00" as seconds since 1970.
pub fn unix_time(s: &str) -> Option<i64> {
    let (date, rest) = s.split_once('T')?;
    let mut d = date.split('-').map(|p| p.parse::<i64>());
    let (y, m, day) = (d.next()?.ok()?, d.next()?.ok()?, d.next()?.ok()?);
    let time = rest.get(..8)?;
    let mut t = time.split(':').map(|p| p.parse::<i64>());
    let (hh, mm, ss) = (t.next()?.ok()?, t.next()?.ok()?, t.next()?.ok()?);
    let tz = &rest[8..];
    let offset = match tz.chars().next() {
        Some(sign @ ('+' | '-')) => {
            let h: i64 = tz.get(1..3)?.parse().ok()?;
            let m: i64 = tz.get(4..6)?.parse().ok()?;
            (h * 3600 + m * 60) * if sign == '-' { -1 } else { 1 }
        }
        _ => 0,
    };
    // Days from 1970-01-01 (Howard Hinnant's days_from_civil).
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    Some(days * 86400 + hh * 3600 + mm * 60 + ss - offset)
}

pub fn urlencode(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

#[cfg(test)]
mod match_tests {
    use super::*;

    #[test]
    fn reads_term_matches() {
        let v = serde_json::json!({
            "version": 1,
            "matches": [
                {"vid": "subject", "value": "Anniversarys", "terms": [{"tid": "1414", "name": "Anniversaries", "uris": ["http://id.loc.gov/x"]}]},
                {"vid": "subject", "value": "Nothing", "terms": []}
            ],
            "uris": {"http://id.loc.gov/y": [{"vid": "subject", "tid": 12, "name": "Y", "uris": []}]}
        });
        let m = TermMatches::from_json(&v);
        let t = &m.matches[&("subject".to_string(), "Anniversarys".to_string())];
        assert_eq!((t[0].tid.as_str(), t[0].name.as_str()), ("1414", "Anniversaries"));
        assert_eq!(t[0].authority_uris, vec!["http://id.loc.gov/x"]);
        assert!(m.matches[&("subject".to_string(), "Nothing".to_string())].is_empty());
        assert_eq!(m.uris["http://id.loc.gov/y"][0].1.tid, "12");
    }

    #[test]
    fn reads_drupal_times() {
        assert_eq!(unix_time("1970-01-01T00:00:00+00:00"), Some(0));
        assert_eq!(unix_time("2026-11-02T19:45:58+00:00"), Some(1793648758));
        assert_eq!(unix_time("2026-11-02T11:45:58-08:00"), Some(1793648758));
    }
}
