# Testing Guide

This document describes the test suite for the ado-aw compiler.

## Overview

The test suite consists of:
1. **Unit tests** - Located in `src/main.rs` (in the `tests` module)
2. **Integration tests** - Located in `tests/compiler_tests.rs`

## Running Tests

To run all tests:

```bash
cargo test
```

To run only unit tests:

```bash
cargo test --lib
```

To run only integration tests:

```bash
cargo test --test compiler_tests
```

To run a specific test:

```bash
cargo test test_sanitize_filename
```

To run tests with output:

```bash
cargo test -- --nocapture
```

## Unit Tests

Unit tests are located in the `tests` module at the bottom of `src/main.rs`. These tests cover individual functions:

### `test_sanitize_filename` (6 test cases)
Tests the filename sanitization function with various inputs including:
- Spaces and special characters
- Numbers and mixed case
- Leading/trailing dashes
- Edge cases

### `test_generate_schedule_*` (3 tests)
Tests schedule generation for:
- Hourly schedules
- Daily schedules
- Deterministic behavior (same input produces same output)

### `test_generate_repositories_*` (3 tests)
Tests repository YAML generation for:
- Empty repository list
- Single repository
- Multiple repositories

### `test_generate_checkout_steps_*` (2 tests)
Tests checkout step generation for:
- Empty repository list
- Multiple repositories

### `test_generate_copilot_params_*` (3 tests)
Tests copilot parameter generation for:
- Built-in MCPs (enabled)
- Built-in MCPs (disabled)
- Custom MCPs (should be skipped)

### `test_parse_markdown_*` (4 tests)
Tests markdown parsing for:
- Valid markdown with front matter
- Missing front matter
- Unclosed front matter
- Invalid YAML in front matter

### `test_default_ref`
Tests the default repository reference value.

## Integration Tests

Integration tests are located in `tests/compiler_tests.rs`. These tests verify the overall behavior of the compiler:

### `test_compile_pipeline_basic`
Tests the basic compilation workflow including:
- Creating temporary test directories
- Writing test input files
- Verifying directory structure

### `test_compiled_yaml_structure`
Verifies that the base template contains all required markers:
- `{{ repositories }}`
- `{{ schedule }}`
- `{{ checkout_repositories }}`
- `{{ agent }}`
- `{{ agent_name }}`
- `{{ copilot_params }}`

### `test_example_file_structure`
Validates the example file (`examples/sample-agent.md`) to ensure:
- Proper YAML front matter structure
- Required fields are present
- Closing front matter delimiter exists

### `test_filename_edge_cases`
Documents expected behavior for various filename sanitization scenarios.

### `test_project_dependencies`
Verifies that all required dependencies are present in `Cargo.toml`:
- clap
- anyhow
- serde
- serde_yaml

## Test Coverage

The current test suite provides coverage for:
- ✅ Filename sanitization
- ✅ Schedule generation
- ✅ Repository configuration generation
- ✅ Checkout step generation
- ✅ Copilot parameter generation
- ✅ Markdown parsing
- ✅ Template structure validation
- ✅ Example file validation
- ✅ Dependency verification

## Adding New Tests

When adding new functionality to the compiler:

1. **Add unit tests** for new helper functions in `src/main.rs`:
   ```rust
   #[test]
   fn test_new_function() {
       // Test code here
   }
   ```

2. **Add integration tests** for end-to-end functionality in `tests/compiler_tests.rs`:
   ```rust
   #[test]
   fn test_new_integration_scenario() {
       // Test code here
   }
   ```

3. **Follow naming conventions**:
   - Unit test names: `test_<function_name>_<scenario>`
   - Integration test names: `test_<feature>_<scenario>`

4. **Include edge cases**:
   - Empty inputs
   - Invalid inputs
   - Boundary conditions
   - Error cases

## Test Standards

All tests should:
- Be self-contained and not depend on external state
- Clean up any resources they create
- Use descriptive assertion messages
- Follow the Arrange-Act-Assert pattern
- Test both success and failure cases

## Continuous Integration

These tests are designed to run in CI/CD pipelines. Ensure all tests pass before submitting pull requests:

```bash
cargo test --all
cargo clippy -- -D warnings
cargo fmt -- --check
```
