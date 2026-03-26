pub mod cli;
pub mod csv_modifier;
pub mod google_sheets;
pub mod item_csv_generator;
pub mod pipeline;
pub mod modifiers;

pub use cli::{Cli, Commands, Modifier};
pub use csv_modifier::{ColumnModifier, CsvModifier, ProcessingStats, RowContext};
pub use item_csv_generator::{ItemCsvGenerator, ItemGenerationStats};
pub use modifiers::{
    AccessIdentifierValidator, FieldDescriptionSemicolonEscaper, FieldModelModifier,
    FileExtensionModifier, ParentIdModifier,
};

pub use pipeline::{
    ProcessResult,
    determine_items_output_path,
    determine_processed_output_path,
    determine_processed_output_path_for_sheets,
    generate_items_from_path,
    generate_items_from_source,
    generate_items_from_url,
    process_csv_and_maybe_generate_items,
    process_google_sheets_and_maybe_generate_items,
};
