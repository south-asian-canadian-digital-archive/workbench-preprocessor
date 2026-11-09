//! The SACDA ingest standard: one spreadsheet in, a checked Workbench bundle out.
//!
//! - `config`     g/config/sacda.toml + schema.toml
//! - `source`     load and clean the sheet
//! - `tree`       collection -> component -> item tree from identifiers
//! - `precheck`   every check, findings and fixes
//! - `authority`  LCSH / LC name lookups for new terms
//! - `suggest`    "did you mean" for columns and terms
//! - `edtf`       EDTF validation and normalisation
//! - `site`       read-only JSON:API client
//! - `bundle`     write CSVs + configs + manifest
//! - `run`        drive Workbench over a bundle
//! - `doctor`     machine + site health check

pub mod authority;
pub mod bundle;
pub mod config;
pub mod doctor;
pub mod edtf;
pub mod precheck;
pub mod run;
pub mod site;
pub mod source;
pub mod suggest;
pub mod tree;
