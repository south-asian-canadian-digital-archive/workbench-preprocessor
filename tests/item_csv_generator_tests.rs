use anyhow::Result;
use organise::ItemCsvGenerator;
use std::fs::File;
use std::io::Write;
use tempfile::tempdir;

fn create_test_csv(path: &str, content: &str) -> Result<()> {
    let mut file = File::create(path)?;
    file.write_all(content.as_bytes())?;
    Ok(())
}

#[test]
fn test_generate_items_basic() -> Result<()> {
    let dir = tempdir()?;
    let input_path = dir.path().join("modified.csv");
    let output_path = dir.path().join("items.csv");

    let csv_content = "accessIdentifier,parent_id,fileTitle,file\n\
                      2024_19_01_001,2024_19_01,Annual Report 2024,2024_19_01/document1.pdf\n\
                      2024_19_01_002,2024_19_01,Annual Report 2024,2024_19_01/document2.pdf\n\
                      2024_19_01_003,2024_19_01,Annual Report 2024,2024_19_01/document3.pdf\n\
                      2024_20_01_001,2024_20_01,Monthly Newsletter,2024_20_01/newsletter1.pdf\n";

    create_test_csv(input_path.to_str().unwrap(), csv_content)?;

    let stats = ItemCsvGenerator::generate(
        input_path.to_str().unwrap(),
        output_path.to_str().unwrap(),
        None,
    )?;

    assert_eq!(stats.unique_parents, 2);
    assert_eq!(stats.total_items, 4);
    assert_eq!(stats.skipped_rows, 0);

    let output_content = std::fs::read_to_string(&output_path)?;
    assert!(output_content.contains("2024_19_01,Annual Report 2024,3,"));
    assert!(output_content.contains("2024_20_01,Monthly Newsletter,1,"));

    Ok(())
}

#[test]
fn test_generate_items_with_empty_parent_ids() -> Result<()> {
    let dir = tempdir()?;
    let input_path = dir.path().join("modified.csv");
    let output_path = dir.path().join("items.csv");

    let csv_content = "accessIdentifier,parent_id,fileTitle,file\n\
                      2024_19_01_001,2024_19_01,Annual Report 2024,2024_19_01/document1.pdf\n\
                      2024_19_01_002,,Annual Report 2024,document2.pdf\n\
                      2024_19_01_003,2024_19_01,Annual Report 2024,2024_19_01/document3.pdf\n";

    create_test_csv(input_path.to_str().unwrap(), csv_content)?;

    let stats = ItemCsvGenerator::generate(
        input_path.to_str().unwrap(),
        output_path.to_str().unwrap(),
        None,
    )?;

    assert_eq!(stats.unique_parents, 1);
    assert_eq!(stats.total_items, 3);
    assert_eq!(stats.skipped_rows, 1);

    Ok(())
}

#[test]
fn test_generate_items_missing_column() {
    let dir = tempdir().unwrap();
    let input_path = dir.path().join("modified.csv");
    let output_path = dir.path().join("items.csv");

    let csv_content = "accessIdentifier,file\n2024_19_01_001,document1.pdf\n";
    create_test_csv(input_path.to_str().unwrap(), csv_content).unwrap();

    let result = ItemCsvGenerator::generate(
        input_path.to_str().unwrap(),
        output_path.to_str().unwrap(),
        None,
    );

    assert!(result.is_err());
}

#[test]
fn test_generate_items_title_priority() -> Result<()> {
    let dir = tempdir()?;
    let input_path = dir.path().join("modified.csv");
    let output_path = dir.path().join("items.csv");

    let csv_content = "accessIdentifier,parent_id,fileTitle,file\n\
                      2024_19_01_001,2024_19_01,,2024_19_01/document1.pdf\n\
                      2024_19_01_002,2024_19_01,Annual Report 2024,2024_19_01/document2.pdf\n";

    create_test_csv(input_path.to_str().unwrap(), csv_content)?;

    let stats = ItemCsvGenerator::generate(
        input_path.to_str().unwrap(),
        output_path.to_str().unwrap(),
        None,
    )?;

    assert_eq!(stats.unique_parents, 1);
    assert_eq!(stats.total_items, 2);

    let output_content = std::fs::read_to_string(&output_path)?;
    assert!(output_content.contains("Annual Report 2024"));

    Ok(())
}

#[test]
fn test_generate_items_ignores_value_placeholders() -> Result<()> {
    let dir = tempdir()?;
    let input_path = dir.path().join("modified.csv");
    let output_path = dir.path().join("items.csv");

    let csv_content = "accessIdentifier,parent_id,fileTitle,file\n\
                      2024_19_01_001,#VALUE!,#VALUE!,#VALUE!\n\
                      2024_19_01_002,2024_19_01,Annual Report 2024,#VALUE!\n";

    create_test_csv(input_path.to_str().unwrap(), csv_content)?;

    let stats = ItemCsvGenerator::generate(
        input_path.to_str().unwrap(),
        output_path.to_str().unwrap(),
        None,
    )?;

    assert_eq!(stats.unique_parents, 1);
    assert_eq!(stats.total_items, 2);
    assert_eq!(stats.skipped_rows, 1);

    let output_content = std::fs::read_to_string(&output_path)?;
    assert!(!output_content.contains("#VALUE!"));
    assert!(output_content.contains("2024_19_01"));

    Ok(())
}

#[test]
fn test_generate_items_populates_node_column() -> Result<()> {
    let dir = tempdir()?;
    let input_path = dir.path().join("modified.csv");
    let output_path = dir.path().join("items.csv");

    let csv_content = "accessIdentifier,parent_id,fileTitle,file\n\
                      2024_19_01_001,2024_19_01,Annual Report 2024,doc.pdf\n";

    create_test_csv(input_path.to_str().unwrap(), csv_content)?;

    let node_value = "19";
    let stats = ItemCsvGenerator::generate(
        input_path.to_str().unwrap(),
        output_path.to_str().unwrap(),
        Some(node_value),
    )?;

    assert_eq!(stats.unique_parents, 1);

    let output_content = std::fs::read_to_string(&output_path)?;
    assert!(output_content.contains(",19"));

    Ok(())
}
