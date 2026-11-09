//! Read-only client for the Islandora site: JSON:API for vocabularies and node
//! lookups, plus the `/lang-code` export. Nothing here writes to the site -
//! Workbench does the writing.

use super::config::{read_credentials, Config, SiteCfg};
use anyhow::{anyhow, bail, Context, Result};
use reqwest::blocking::{Client, Response};
use reqwest::StatusCode;
use serde_json::Value;
use std::collections::HashMap;
use std::thread::sleep;
use std::time::Duration;

pub const JSONAPI_HELP: &str = "JSON:API is off on this site. Turn it on under Extend \
(/admin/modules, \"JSON:API\") or with `drush en jsonapi`, and keep it read-only at \
/admin/config/services/jsonapi. Docs: \
https://www.drupal.org/docs/core-modules-and-themes/core-modules/jsonapi-module";

#[derive(Debug, Clone)]
pub struct Term {
    pub name: String,
    pub tid: String,
    pub authority_uris: Vec<String>,
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
                Err(_) => {}
            }
            sleep(delay);
            delay = (delay * 2).min(Duration::from_secs(8));
        }
        unreachable!()
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

    /// Every term in a vocabulary, following JSON:API pagination.
    pub fn vocabulary(&self, vid: &str) -> Result<Vec<Term>> {
        let mut out = Vec::new();
        let mut url = Some(format!(
            "{}/jsonapi/taxonomy_term/{vid}?page%5Blimit%5D=50&sort=drupal_internal__tid",
            self.host()
        ));
        while let Some(u) = url.take() {
            let r = self.get(&u)?;
            if r.status() == StatusCode::NOT_FOUND {
                // An unknown vocabulary 404s; so does a site without JSON:API.
                bail!("vocabulary '{vid}' isn't on {} (or JSON:API is off)", self.host());
            }
            if !r.status().is_success() {
                bail!("{} answered {} listing '{vid}'", self.host(), r.status());
            }
            let doc: Value = r.json()?;
            for item in doc["data"].as_array().into_iter().flatten() {
                let attrs = &item["attributes"];
                let name = attrs["name"].as_str().unwrap_or_default().to_string();
                let tid = attrs["drupal_internal__tid"]
                    .as_i64()
                    .map(|t| t.to_string())
                    .unwrap_or_default();
                let mut uris = Vec::new();
                for key in ["field_authority_link", "field_external_uri"] {
                    match &attrs[key] {
                        Value::Array(a) => uris.extend(
                            a.iter().filter_map(|l| l["uri"].as_str().map(String::from)),
                        ),
                        Value::Object(o) => {
                            if let Some(u) = o.get("uri").and_then(Value::as_str) {
                                uris.push(u.to_string())
                            }
                        }
                        _ => {}
                    }
                }
                out.push(Term {
                    name,
                    tid,
                    authority_uris: uris,
                });
            }
            url = doc["links"]["next"]["href"].as_str().map(String::from);
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

pub fn urlencode(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}
