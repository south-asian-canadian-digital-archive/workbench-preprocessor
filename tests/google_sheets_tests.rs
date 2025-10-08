//! Integration tests specifically for Google Sheets functionality
//!
//! These tests focus on URL parsing, conversion, and HTTP client behavior
//! (without actually making network requests in most cases).

use organise::CsvModifier;

/// Test comprehensive Google Sheets URL parsing and conversion
#[test]
fn test_google_sheets_url_parsing_comprehensive() -> Result<(), Box<dyn std::error::Error>> {
    let test_cases = vec![
        // Standard edit URLs
        (
            "https://docs.google.com/spreadsheets/d/1BxiMVs0XRA5nFMdKvBdBZjgmUUqptlbs74OgvE2upms/edit#gid=0",
            "1BxiMVs0XRA5nFMdKvBdBZjgmUUqptlbs74OgvE2upms"
        ),
        (
            "https://docs.google.com/spreadsheets/d/abc123def456ghi789/edit?usp=sharing",
            "abc123def456ghi789"
        ),
        // URLs with different sheet IDs
        (
            "https://docs.google.com/spreadsheets/d/1234567890abcdef/edit",
            "1234567890abcdef"
        ),
        // Complex sheet IDs
        (
            "https://docs.google.com/spreadsheets/d/1Bxi-MV_s0XRA5nF-MdKv_BdBZjgm-UUqptlbs74OgvE2upms/edit#gid=123",
            "1Bxi-MV_s0XRA5nF-MdKv_BdBZjgm-UUqptlbs74OgvE2upms"
        ),
    ];

    for (input_url, expected_sheet_id) in test_cases {
        let csv_url = CsvModifier::google_sheets_to_csv_url(input_url)?;

        // Verify the CSV URL format
        assert!(csv_url.starts_with("https://docs.google.com/spreadsheets/d/"));
        assert!(csv_url.ends_with("/export?format=csv"));
        assert!(csv_url.contains(expected_sheet_id));

        // Verify the complete expected URL
        let expected_csv_url = format!(
            "https://docs.google.com/spreadsheets/d/{}/export?format=csv",
            expected_sheet_id
        );
        assert_eq!(csv_url, expected_csv_url);
    }

    Ok(())
}

/// Test edge cases and error conditions for Google Sheets URLs
#[test]
fn test_google_sheets_url_error_cases() {
    let error_cases = vec![
        // Not a Google Sheets URL
        (
            "https://example.com/spreadsheet",
            "Should reject non-Google URLs",
        ),
        (
            "https://sheets.google.com/spreadsheets/d/123/edit",
            "Should reject wrong domain",
        ),
        // Missing components
        (
            "https://docs.google.com/spreadsheets/d/",
            "Should reject missing sheet ID",
        ),
        (
            "https://docs.google.com/spreadsheets/d/edit",
            "Should reject malformed path",
        ),
        (
            "https://docs.google.com/spreadsheets/",
            "Should reject incomplete path",
        ),
        // Invalid URLs
        ("not-a-url-at-all", "Should reject invalid URL format"),
        ("", "Should reject empty string"),
        ("https://", "Should reject incomplete URL"),
        // Wrong Google service
        (
            "https://docs.google.com/document/d/123/edit",
            "Should reject Google Docs",
        ),
        (
            "https://docs.google.com/presentation/d/123/edit",
            "Should reject Google Slides",
        ),
    ];

    for (invalid_url, description) in error_cases {
        let result = CsvModifier::google_sheets_to_csv_url(invalid_url);
        assert!(
            result.is_err(),
            "Failed test case: {} - {}",
            description,
            invalid_url
        );
    }
}

/// Test URL normalization and cleaning
#[test]
fn test_google_sheets_url_normalization() -> Result<(), Box<dyn std::error::Error>> {
    // All these should produce the same result
    let equivalent_urls = vec![
        "https://docs.google.com/spreadsheets/d/test123/edit",
        "https://docs.google.com/spreadsheets/d/test123/edit#gid=0",
        "https://docs.google.com/spreadsheets/d/test123/edit?usp=sharing",
        "https://docs.google.com/spreadsheets/d/test123/edit#gid=456",
        "https://docs.google.com/spreadsheets/d/test123/edit?usp=sharing&other=param",
        "https://docs.google.com/spreadsheets/d/test123/edit?usp=sharing#gid=789",
    ];

    let expected_result = "https://docs.google.com/spreadsheets/d/test123/export?format=csv";

    for url in equivalent_urls {
        let result = CsvModifier::google_sheets_to_csv_url(url)?;
        assert_eq!(
            result, expected_result,
            "URL normalization failed for: {}",
            url
        );
    }

    Ok(())
}

/// Test that the CSV URL format is correct and would be usable by HTTP clients
#[test]
fn test_csv_url_format_validity() -> Result<(), Box<dyn std::error::Error>> {
    let test_url =
        "https://docs.google.com/spreadsheets/d/1BxiMVs0XRA5nFMdKvBdBZjgmUUqptlbs74OgvE2upms/edit";
    let csv_url = CsvModifier::google_sheets_to_csv_url(test_url)?;

    // Parse the URL to ensure it's valid
    let parsed_url = url::Url::parse(&csv_url)?;

    assert_eq!(parsed_url.scheme(), "https");
    assert_eq!(parsed_url.host_str(), Some("docs.google.com"));
    assert_eq!(
        parsed_url.path(),
        "/spreadsheets/d/1BxiMVs0XRA5nFMdKvBdBZjgmUUqptlbs74OgvE2upms/export"
    );

    // Check query parameters
    let query_params: std::collections::HashMap<_, _> = parsed_url.query_pairs().collect();
    assert_eq!(query_params.get("format"), Some(&"csv".into()));

    Ok(())
}

/// Test various Google Sheets ID formats that exist in the wild
#[test]
fn test_real_world_sheet_id_formats() -> Result<(), Box<dyn std::error::Error>> {
    // These are based on actual Google Sheets ID patterns
    let real_world_ids = vec![
        "1BxiMVs0XRA5nFMdKvBdBZjgmUUqptlbs74OgvE2upms", // Example from Google docs
        "1234567890abcdefghijklmnopqrstuvwxyzABCDEF",   // Alphanumeric
        "1ABC-123_def456GHI789jkl_MNO-PQR",             // With hyphens and underscores
        "1a",                                           // Minimal valid ID
    ];

    // Test with a very long ID separately
    let long_id = format!("1{}", "a".repeat(100));
    let mut all_ids = real_world_ids.clone();
    all_ids.push(&long_id);

    for sheet_id in all_ids {
        let input_url = format!("https://docs.google.com/spreadsheets/d/{}/edit", sheet_id);
        let expected_output = format!(
            "https://docs.google.com/spreadsheets/d/{}/export?format=csv",
            sheet_id
        );

        let result = CsvModifier::google_sheets_to_csv_url(&input_url)?;
        assert_eq!(result, expected_output, "Failed for sheet ID: {}", sheet_id);
    }

    Ok(())
}

/// Test concurrent URL conversion (to ensure thread safety)
#[test]
fn test_concurrent_url_conversion() -> Result<(), Box<dyn std::error::Error>> {
    use std::sync::Arc;
    use std::thread;

    let test_urls = Arc::new(vec![
        "https://docs.google.com/spreadsheets/d/id1/edit",
        "https://docs.google.com/spreadsheets/d/id2/edit",
        "https://docs.google.com/spreadsheets/d/id3/edit",
        "https://docs.google.com/spreadsheets/d/id4/edit",
        "https://docs.google.com/spreadsheets/d/id5/edit",
    ]);

    let mut handles = vec![];

    // Spawn multiple threads to test concurrent access
    for i in 0..5 {
        let urls = Arc::clone(&test_urls);
        let handle = thread::spawn(move || {
            let url = &urls[i];
            let result = CsvModifier::google_sheets_to_csv_url(url);
            assert!(result.is_ok(), "Thread {} failed to convert URL", i);
            result.unwrap()
        });
        handles.push(handle);
    }

    // Wait for all threads and collect results
    let results: Vec<String> = handles
        .into_iter()
        .map(|h| h.join().expect("Thread panicked"))
        .collect();

    // Verify all results
    for (i, result) in results.iter().enumerate() {
        let expected = format!(
            "https://docs.google.com/spreadsheets/d/id{}/export?format=csv",
            i + 1
        );
        assert_eq!(result, &expected);
    }

    Ok(())
}
