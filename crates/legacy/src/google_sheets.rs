use crate::csv_modifier::{CsvModifier, ProcessingStats};
use anyhow::{Context, Result};
use csv::Reader;
use std::io::Cursor;

fn is_valid_sheet_id(id: &str) -> bool {
    if id.len() < 2 || id == "edit" {
        return false;
    }

    let mut chars = id.chars();
    if let Some(first_char) = chars.next() {
        if !first_char.is_alphanumeric() {
            return false;
        }
    } else {
        return false;
    }

    id.chars()
        .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
}

impl CsvModifier {
    /// Convert Google Sheets URL to CSV export URL
    pub fn google_sheets_to_csv_url(url: &str) -> Result<String> {
        let url = url::Url::parse(url).context("Invalid Google Sheets URL")?;

        if url.host_str() != Some("docs.google.com") {
            anyhow::bail!(
                "URL must be from docs.google.com, got: {}",
                url.host_str().unwrap_or("unknown")
            );
        }

        // The tab: `#gid=N` (what the address bar shows) or `?gid=N`. Without
        // it the export is the first tab, whichever tab the link was for.
        let gid = url
            .fragment()
            .into_iter()
            .flat_map(|f| f.split('&'))
            .chain(url.query().into_iter().flat_map(|q| q.split('&')))
            .find_map(|kv| kv.strip_prefix("gid="))
            .filter(|g| !g.is_empty() && g.bytes().all(|b| b.is_ascii_digit()))
            .map(|g| format!("&gid={g}"))
            .unwrap_or_default();
        let path = url.path();
        if let Some(start) = path.find("/spreadsheets/d/") {
            let id_start = start + 16;
            if let Some(end) = path[id_start..].find('/') {
                let sheet_id = &path[id_start..id_start + end];
                if sheet_id.is_empty() || !is_valid_sheet_id(sheet_id) {
                    anyhow::bail!("Invalid or empty spreadsheet ID in URL: {}", url);
                }
                return Ok(format!(
                    "https://docs.google.com/spreadsheets/d/{}/export?format=csv{gid}",
                    sheet_id
                ));
            } else {
                let sheet_id = &path[id_start..];
                if sheet_id.is_empty() || !is_valid_sheet_id(sheet_id) {
                    anyhow::bail!("Invalid or empty spreadsheet ID in URL: {}", url);
                }
                return Ok(format!(
                    "https://docs.google.com/spreadsheets/d/{}/export?format=csv{gid}",
                    sheet_id
                ));
            }
        }

        anyhow::bail!("Could not extract spreadsheet ID from URL - path should contain '/spreadsheets/d/': {}", url)
    }

    pub fn fetch_google_sheets_csv(url: &str) -> Result<String> {
        let csv_url = Self::google_sheets_to_csv_url(url)?;

        let response = reqwest::blocking::get(&csv_url)
            .with_context(|| format!("Failed to fetch Google Sheets CSV from: {}", csv_url))?;

        if !response.status().is_success() {
            anyhow::bail!(
                "HTTP error {}: Failed to fetch Google Sheets data",
                response.status()
            );
        }

        let html = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.contains("text/html"));
        let text = response.text().context("Failed to read response body as text")?;
        // A sheet that isn't shared answers 200 with Google's sign-in page.
        if html || text.trim_start().starts_with("<!DOCTYPE html") || text.trim_start().starts_with("<html") {
            anyhow::bail!(
                "Google sent a web page instead of the sheet - share it as \"Anyone with the link can view\" and try again"
            );
        }
        Ok(text)
    }

    /// Process CSV data from Google Sheets URL and write to output file
    pub fn process_google_sheets(
        &self,
        sheets_url: &str,
        output_path: &str,
    ) -> Result<ProcessingStats> {
        let csv_data = Self::fetch_google_sheets_csv(sheets_url)?;
        let mut reader = Reader::from_reader(Cursor::new(csv_data));
        self.process_csv_reader(&mut reader, output_path)
    }
}
