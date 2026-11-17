# Modifiers Overview

This folder contains the column modifiers that the CSV preprocessor ships with. Each modifier lives in its own module so it can own any helper functions it needs without bloating `csv_modifier.rs`.

## Adding a New Modifier

1. **Create the module**
   - Add a new `snake_case.rs` file in this directory.
   - Implement the `ColumnModifier` trait exported by `organise::csv_modifier`.
   - Keep any modifier-specific helpers private to the module.

2. **Expose the type**
   - Declare the module in `mod.rs` (e.g. `pub mod my_modifier;`).
   - Re-export the type in the same file so binaries/tests can import it (`pub use my_modifier::MyModifier;`).

3. **Wire it into the CLI (optional)**
   - If you want the modifier selectable via the CLI, extend the `Modifier` enum in `cli.rs` and update `create_modifier` in `main.rs` to construct the new modifier when requested.

4. **Document configuration (if any)**
   - If the modifier reads external config (like `FieldModelModifier`), store the config alongside the module and document the expected structure.

5. **Add tests**
   - Prefer integration tests in `tests/` that exercise the modifier in combination with others.

This layout keeps modifiers isolated, making it trivial to drop in new behaviour without touching the shared CSV pipeline.
