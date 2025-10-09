//! Integration tests for the CSV Organiser library
//!
//! These tests exercise the public API of the library and test the interaction
//! between multiple components, simulating real-world usage scenarios.

use organise::{ColumnModifier, CsvModifier, FileExtensionModifier, ParentIdModifier, RowContext};
use std::fs::File;
use std::io::{Cursor, Write};
use tempfile::tempdir;

/// Helper function to create temporary CSV files for testing
fn create_temp_csv(
    content: &str,
) -> Result<(String, tempfile::TempDir), Box<dyn std::error::Error>> {
    let dir = tempdir()?;
    let file_path = dir.path().join("test.csv");
    let mut file = File::create(&file_path)?;
    file.write_all(content.as_bytes())?;

    Ok((file_path.to_string_lossy().into_owned(), dir))
}

/// Test basic CSV processing with built-in modifiers
#[test]
fn test_basic_csv_processing() -> Result<(), Box<dyn std::error::Error>> {
    let csv_content = r#"accessIdentifier,file,file_extension,parent_id,title
2024_19_01_001,document,pdf,,First Document
2024_19_01_002,image,jpg,,Second Image
2024_20_02_001,report,docx,,Third Report"#;

    let (input_path, _temp_dir) = create_temp_csv(&csv_content)?;
    let output_path = format!("{}_output.csv", input_path);

    let modifier = CsvModifier::new()
        .add_column_modifier("parent_id", ParentIdModifier)
        .add_column_modifier("file", FileExtensionModifier);

    let stats = modifier.process_file(&input_path, &output_path)?;

    // Verify statistics
    assert_eq!(stats.total_rows, 3);
    assert_eq!(stats.cells_modified, 6); // 3 rows × 2 columns
    assert_eq!(stats.validation_failures, 0);
    assert_eq!(stats.columns_processed.len(), 3);
    assert!(stats.columns_processed.contains("parent_id"));
    assert!(stats.columns_processed.contains("file"));
    assert!(stats.columns_processed.contains("accessIdentifier"));

    // Verify output content
    let output_content = std::fs::read_to_string(&output_path)?;
    let lines: Vec<&str> = output_content.lines().collect();

    // Check header
    assert_eq!(
        lines[0],
        "accessIdentifier,file,file_extension,parent_id,title"
    );

    // Check first row
    assert!(lines[1].contains("2024_19_01"));
    assert!(lines[1].contains("2024_19_01/document.pdf"));

    // Check second row
    assert!(lines[2].contains("2024_19_01"));
    assert!(lines[2].contains("2024_19_01/image.jpg"));

    // Check third row
    assert!(lines[3].contains("2024_20_02"));
    assert!(lines[3].contains("2024_20_02/report.docx"));

    Ok(())
}

/// Verify that the file extension modifier accepts the common misspelling `file_extention`
#[test]
fn test_csv_processing_with_file_extention_alias() -> Result<(), Box<dyn std::error::Error>> {
    let csv_content = r#"accessIdentifier,file,file_extention,parent_id,title
2024_19_01_001,document,pdf,,First Document
2024_19_01_002,image,jpg,,Second Image"#;

    let (input_path, _temp_dir) = create_temp_csv(&csv_content)?;
    let output_path = format!("{}_output.csv", input_path);

    let modifier = CsvModifier::new()
        .add_column_modifier("parent_id", ParentIdModifier)
        .add_column_modifier("file", FileExtensionModifier);

    let stats = modifier.process_file(&input_path, &output_path)?;

    assert_eq!(stats.total_rows, 2);
    assert_eq!(stats.validation_failures, 0);
    assert_eq!(stats.cells_modified, 4);

    let output_content = std::fs::read_to_string(&output_path)?;
    let mut lines = output_content.lines();

    assert_eq!(
        lines.next().unwrap(),
        "accessIdentifier,file,file_extention,parent_id,title"
    );

    let first_row = lines.next().unwrap();
    assert!(first_row.contains("2024_19_01/document.pdf"));

    let second_row = lines.next().unwrap();
    assert!(second_row.contains("2024_19_01/image.jpg"));

    Ok(())
}

/// Ensure placeholder values like `#VALUE!` are treated as empty during processing
#[test]
fn test_csv_processing_ignores_value_placeholders() -> Result<(), Box<dyn std::error::Error>> {
    let csv_content = r#"accessIdentifier,file,file_extension,parent_id,title
#VALUE!,#VALUE!,pdf,#VALUE!,Broken Row
2024_19_01_001,document,pdf,,Valid Row"#;

    let (input_path, _temp_dir) = create_temp_csv(&csv_content)?;
    let output_path = format!("{}_output.csv", input_path);

    let modifier = CsvModifier::new()
        .add_column_modifier("parent_id", ParentIdModifier)
        .add_column_modifier("file", FileExtensionModifier);

    let stats = modifier.process_file(&input_path, &output_path)?;

    assert!(stats.validation_failures > 0);

    let output_content = std::fs::read_to_string(&output_path)?;
    assert!(output_content.contains("2024_19_01/document.pdf"));

    let mut reader = csv::Reader::from_reader(Cursor::new(output_content.as_bytes()));
    for result in reader.records() {
        let record = result?;
        let file_cell = record.get(1).unwrap_or("");
        let parent_id_cell = record.get(3).unwrap_or("");

        assert_ne!(file_cell, "#VALUE!");
        assert_ne!(parent_id_cell, "#VALUE!");
    }

    Ok(())
}

/// Test processing with validation failures
#[test]
fn test_csv_processing_with_validation_failures() -> Result<(), Box<dyn std::error::Error>> {
    let csv_content = r#"accessIdentifier,file,file_extension,parent_id,title
2024_19_01_001,document,pdf,,Valid Document
,,,,Empty Row
2024_19_01_003,,pdf,,Missing Filename
,existing_file,pdf,,Missing Access ID"#;

    let (input_path, _temp_dir) = create_temp_csv(csv_content)?;
    let output_path = format!("{}_output.csv", input_path);

    let modifier = CsvModifier::new()
        .add_column_modifier("parent_id", ParentIdModifier)
        .add_column_modifier("file", FileExtensionModifier);

    let stats = modifier.process_file(&input_path, &output_path)?;

    // Row analysis:
    // Row 1: Has accessIdentifier & file & file_extension -> both modifiers can apply -> 2 modifications
    // Row 2: Empty values -> no modifications -> 0 modifications
    // Row 3: Has accessIdentifier but no file -> parent_id can apply, file cannot -> 1 modification
    // Row 4: Has file & file_extension but no accessIdentifier -> neither can apply -> 0 modifications

    assert_eq!(stats.total_rows, 2);
    assert_eq!(stats.skipped_rows, 2);
    assert!(stats.validation_failures > 0);
    assert_eq!(stats.cells_modified, 3); // Row 1: 2 modifications, Row 3: 1 modification

    // Verify that the valid row was processed correctly
    let output_content = std::fs::read_to_string(&output_path)?;
    assert!(output_content.contains("2024_19_01/document.pdf"));
    assert!(output_content.contains("2024_19_01,Valid Document"));
    assert!(!output_content.contains("Missing Access ID"));

    Ok(())
}

/// Test custom modifier implementation
#[test]
fn test_custom_modifier_integration() -> Result<(), Box<dyn std::error::Error>> {
    // Create a custom modifier for testing
    struct UppercaseModifier;

    impl ColumnModifier for UppercaseModifier {
        fn modify(&self, value: &str, _context: &RowContext) -> String {
            value.to_uppercase()
        }

        fn description(&self) -> &str {
            "Converts text to uppercase"
        }

        fn validate(&self, value: &str, _context: &RowContext) -> bool {
            !value.trim().is_empty()
        }
    }

    let csv_content = r#"title,description,category
hello world,test description,category1
foo bar,another description,category2
,empty title,category3"#;

    let (input_path, _temp_dir) = create_temp_csv(csv_content)?;
    let output_path = format!("{}_output.csv", input_path);

    let modifier = CsvModifier::new().add_column_modifier("title", UppercaseModifier);

    let stats = modifier.process_file(&input_path, &output_path)?;

    assert_eq!(stats.total_rows, 2);
    assert_eq!(stats.skipped_rows, 1);
    assert_eq!(stats.cells_modified, 2); // Two valid titles
    assert_eq!(stats.validation_failures, 1); // One empty title

    let output_content = std::fs::read_to_string(&output_path)?;
    assert!(output_content.contains("HELLO WORLD"));
    assert!(output_content.contains("FOO BAR"));

    Ok(())
}

/// Ensure rows with container-style access identifiers are skipped
#[test]
fn test_rows_with_zero_suffix_access_identifier_are_skipped(
) -> Result<(), Box<dyn std::error::Error>> {
    let csv_content = r#"accessIdentifier,file,file_extension,parent_id,title
2024_19_01_000,document,pdf,,Container Record
2024_19_01_001,document,pdf,,Child Record"#;

    let (input_path, _temp_dir) = create_temp_csv(csv_content)?;
    let output_path = format!("{}_output.csv", input_path);

    let modifier = CsvModifier::new()
        .add_column_modifier("parent_id", ParentIdModifier)
        .add_column_modifier("file", FileExtensionModifier);

    let stats = modifier.process_file(&input_path, &output_path)?;

    assert_eq!(stats.skipped_rows, 1);
    assert_eq!(stats.total_rows, 1);
    assert_eq!(stats.cells_modified, 2);
    assert_eq!(stats.validation_failures, 1);

    let output_content = std::fs::read_to_string(&output_path)?;
    assert!(!output_content.contains("Container Record"));
    assert!(output_content.contains("Child Record"));
    assert!(output_content.contains("2024_19_01/document.pdf"));

    Ok(())
}

/// Ensure duplicate access identifiers are rejected after the first occurrence
#[test]
fn test_duplicate_access_identifiers_are_skipped() -> Result<(), Box<dyn std::error::Error>> {
    let csv_content = r#"accessIdentifier,file,file_extension,parent_id,title
2024_19_01_001,document,pdf,,Original Row
2024_19_01_001,image,jpg,,Duplicate Row
2024_19_01_002,report,pdf,,Second Unique"#;

    let (input_path, _temp_dir) = create_temp_csv(csv_content)?;
    let output_path = format!("{}_output.csv", input_path);

    let modifier = CsvModifier::new()
        .add_column_modifier("parent_id", ParentIdModifier)
        .add_column_modifier("file", FileExtensionModifier);

    let stats = modifier.process_file(&input_path, &output_path)?;

    assert_eq!(stats.total_rows, 2);
    assert_eq!(stats.skipped_rows, 1);
    assert_eq!(stats.validation_failures, 1);
    assert_eq!(stats.cells_modified, 4);

    let output_content = std::fs::read_to_string(&output_path)?;
    assert!(output_content.contains("2024_19_01/document.pdf"));
    assert!(output_content.contains("2024_19_01/report.pdf"));
    assert!(!output_content.contains("Duplicate Row"));

    Ok(())
}

/// Ensure rows with empty titles are skipped before modifiers run
#[test]
fn test_rows_with_empty_title_are_skipped() -> Result<(), Box<dyn std::error::Error>> {
    let csv_content = r#"accessIdentifier,file,file_extension,parent_id,title
2024_19_01_001,document,pdf,,
2024_19_01_002,image,jpg,,Valid Title"#;

    let (input_path, _temp_dir) = create_temp_csv(csv_content)?;
    let output_path = format!("{}_output.csv", input_path);

    let modifier = CsvModifier::new()
        .add_column_modifier("parent_id", ParentIdModifier)
        .add_column_modifier("file", FileExtensionModifier);

    let stats = modifier.process_file(&input_path, &output_path)?;

    assert_eq!(stats.total_rows, 1);
    assert_eq!(stats.skipped_rows, 1);
    assert!(stats.validation_failures > 0);

    let output_content = std::fs::read_to_string(&output_path)?;
    assert!(output_content.contains("2024_19_01_002"));
    assert!(output_content.contains("2024_19_01/image.jpg"));
    assert!(!output_content.contains(",,"));

    Ok(())
}

/// Ensure textual data is sanitized and field_description is quoted
#[test]
fn test_text_sanitization_and_description_quotes() -> Result<(), Box<dyn std::error::Error>> {
    let nbsp = '\u{00A0}';
    let csv_content = format!(
        "accessIdentifier,file,file_extension,parent_id,title,field_description\n{}
{}
{}
",
        "2024_19_01_001,asset,pdf,,Peopleâ€™s Archive,Peopleâ€™s collection overview",
        "2024_19_01_002,asset,pdf,,MontrÃ©al Stories,\"Already quoted\"",
        format!("2024_19_01_003,asset,pdf,,Valid Title,{}Leading NBSP", nbsp)
    );

    let (input_path, _temp_dir) = create_temp_csv(&csv_content)?;
    let output_path = format!("{}_output.csv", input_path);

    let modifier = CsvModifier::new()
        .add_column_modifier("parent_id", ParentIdModifier)
        .add_column_modifier("file", FileExtensionModifier);

    let stats = modifier.process_file(&input_path, &output_path)?;

    assert_eq!(stats.total_rows, 3);
    assert_eq!(stats.skipped_rows, 0);

    let output_content = std::fs::read_to_string(&output_path)?;
    println!("{:?}", output_content);

    assert!(output_content.contains("People’s Archive"));
    assert!(output_content.contains("Montréal Stories"));
    assert!(output_content.contains("\"\"People’s collection overview\"\""));
    assert!(output_content.contains("\"\"Already quoted\"\""));
    assert!(output_content.contains("\"\" Leading NBSP\"\""));

    Ok(())
}

/// Test cross-column modifier functionality
#[test]
fn test_cross_column_modifier_integration() -> Result<(), Box<dyn std::error::Error>> {
    // Create a modifier that uses data from other columns
    struct FullNameModifier;

    impl ColumnModifier for FullNameModifier {
        fn modify(&self, _value: &str, context: &RowContext) -> String {
            let first = context.get_or_empty("first_name");
            let last = context.get_or_empty("last_name");

            if first.is_empty() && last.is_empty() {
                "Unknown".to_string()
            } else {
                format!("{} {}", first, last).trim().to_string()
            }
        }

        fn description(&self) -> &str {
            "Creates full name from first and last name columns"
        }
    }

    let csv_content = r#"first_name,last_name,full_name,email
John,Doe,,john.doe@example.com
Jane,Smith,,jane.smith@example.com
Bob,,,bob@example.com
,Johnson,,johnson@example.com"#;

    let (input_path, _temp_dir) = create_temp_csv(csv_content)?;
    let output_path = format!("{}_output.csv", input_path);

    let modifier = CsvModifier::new().add_column_modifier("full_name", FullNameModifier);

    let stats = modifier.process_file(&input_path, &output_path)?;

    assert_eq!(stats.total_rows, 4);
    assert_eq!(stats.cells_modified, 4);

    let output_content = std::fs::read_to_string(&output_path)?;
    assert!(output_content.contains("John Doe"));
    assert!(output_content.contains("Jane Smith"));
    assert!(output_content.contains("Bob"));
    assert!(output_content.contains("Johnson"));

    Ok(())
}

/// Test multiple modifiers on different columns
#[test]
fn test_multiple_modifiers_integration() -> Result<(), Box<dyn std::error::Error>> {
    struct PrefixModifier {
        prefix: String,
    }

    impl ColumnModifier for PrefixModifier {
        fn modify(&self, value: &str, _context: &RowContext) -> String {
            if value.is_empty() {
                value.to_string()
            } else {
                format!("{}{}", self.prefix, value)
            }
        }

        fn description(&self) -> &str {
            "Adds prefix to non-empty values"
        }

        fn validate(&self, value: &str, _context: &RowContext) -> bool {
            !value.is_empty()
        }
    }

    let csv_content = r#"accessIdentifier,title,file,file_extension,parent_id
2024_19_01_001,Document Title,document,pdf,
2024_20_02_001,Another Title,report,docx,"#;

    let (input_path, _temp_dir) = create_temp_csv(csv_content)?;
    let output_path = format!("{}_output.csv", input_path);

    let modifier = CsvModifier::new()
        .add_column_modifier("parent_id", ParentIdModifier)
        .add_column_modifier("file", FileExtensionModifier)
        .add_column_modifier(
            "title",
            PrefixModifier {
                prefix: "DOC_".to_string(),
            },
        );

    let stats = modifier.process_file(&input_path, &output_path)?;

    assert_eq!(stats.total_rows, 2);
    assert_eq!(stats.cells_modified, 6); // 2 rows × 3 columns
    assert_eq!(stats.validation_failures, 0);
    assert_eq!(stats.columns_processed.len(), 4);

    let output_content = std::fs::read_to_string(&output_path)?;

    // Check that all modifiers were applied
    assert!(output_content.contains("DOC_Document Title"));
    assert!(output_content.contains("DOC_Another Title"));
    assert!(output_content.contains("2024_19_01/document.pdf"));
    assert!(output_content.contains("2024_20_02/report.docx"));
    assert!(output_content.contains("2024_19_01"));
    assert!(output_content.contains("2024_20_02"));

    Ok(())
}

/// Test Google Sheets URL conversion functionality
#[test]
fn test_google_sheets_url_conversion_integration() -> Result<(), Box<dyn std::error::Error>> {
    let test_cases = vec![
        (
            "https://docs.google.com/spreadsheets/d/1BxiMVs0XRA5nFMdKvBdBZjgmUUqptlbs74OgvE2upms/edit#gid=0",
            "https://docs.google.com/spreadsheets/d/1BxiMVs0XRA5nFMdKvBdBZjgmUUqptlbs74OgvE2upms/export?format=csv"
        ),
        (
            "https://docs.google.com/spreadsheets/d/1BxiMVs0XRA5nFMdKvBdBZjgmUUqptlbs74OgvE2upms/edit?usp=sharing",
            "https://docs.google.com/spreadsheets/d/1BxiMVs0XRA5nFMdKvBdBZjgmUUqptlbs74OgvE2upms/export?format=csv"
        ),
        (
            "https://docs.google.com/spreadsheets/d/abc123def456/edit",
            "https://docs.google.com/spreadsheets/d/abc123def456/export?format=csv"
        ),
    ];

    for (input_url, expected_output) in test_cases {
        let result = CsvModifier::google_sheets_to_csv_url(input_url)?;
        assert_eq!(
            result, expected_output,
            "Failed to convert URL: {}",
            input_url
        );
    }

    // Test invalid URLs
    let invalid_urls = vec![
        "https://example.com/not-google-sheets",
        "not-a-url",
        "https://docs.google.com/spreadsheets/d/", // Missing sheet ID
    ];

    for invalid_url in invalid_urls {
        let result = CsvModifier::google_sheets_to_csv_url(invalid_url);
        assert!(
            result.is_err(),
            "Expected error for invalid URL: {}",
            invalid_url
        );
    }

    Ok(())
}

/// Test error handling for file operations
#[test]
fn test_error_handling_integration() {
    let modifier = CsvModifier::new();

    // Test with non-existent input file
    let result = modifier.process_file("non_existent_file.csv", "output.csv");
    assert!(result.is_err());

    // Test with invalid output path (directory that doesn't exist)
    let csv_content = "col1,col2\nvalue1,value2\n";
    if let Ok((input_path, _temp_dir)) = create_temp_csv(csv_content) {
        let result = modifier.process_file(&input_path, "/non_existent_directory/output.csv");
        assert!(result.is_err());
    }
}

/// Test performance characteristics with larger datasets
#[test]
fn test_performance_integration() -> Result<(), Box<dyn std::error::Error>> {
    // Create a larger CSV for performance testing
    let mut csv_content = String::from("accessIdentifier,file,file_extension,parent_id,title\n");

    for i in 1..=1000 {
        csv_content.push_str(&format!(
            "2024_19_01_{:03},document_{},pdf,,Document {}\n",
            i, i, i
        ));
    }

    let (input_path, _temp_dir) = create_temp_csv(&csv_content)?;
    let output_path = format!("{}_output.csv", input_path);

    let modifier = CsvModifier::new()
        .add_column_modifier("parent_id", ParentIdModifier)
        .add_column_modifier("file", FileExtensionModifier);

    let start = std::time::Instant::now();
    let stats = modifier.process_file(&input_path, &output_path)?;
    let duration = start.elapsed();

    // Verify processing completed successfully
    assert_eq!(stats.total_rows, 1000);
    assert_eq!(stats.cells_modified, 2000); // 1000 rows × 2 columns
    assert_eq!(stats.validation_failures, 0);

    // Performance should be reasonable (less than 1 second for 1000 rows)
    assert!(
        duration.as_secs() < 1,
        "Processing took too long: {:?}",
        duration
    );

    println!("Processed 1000 rows in {:?}", duration);

    Ok(())
}
