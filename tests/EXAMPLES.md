# Test Examples and Patterns

This document provides examples of common testing patterns used in the ado-aw test suite.

## Basic Unit Test Pattern

```rust
#[test]
fn test_function_name() {
    // Arrange - Set up test data
    let input = "test input";
    
    // Act - Call the function being tested
    let result = function_to_test(input);
    
    // Assert - Verify the result
    assert_eq!(result, "expected output");
}
```

## Testing Functions with Structs

```rust
#[test]
fn test_with_struct() {
    let repo = Repository {
        repository: "test-repo".to_string(),
        repo_type: "git".to_string(),
        name: "org/test-repo".to_string(),
        repo_ref: "refs/heads/main".to_string(),
    };
    
    let result = generate_repositories(&vec![repo]);
    
    assert!(result.contains("repository: test-repo"));
    assert!(result.contains("type: git"));
}
```

## Testing Error Cases

```rust
#[test]
fn test_error_handling() {
    let invalid_content = "no front matter";
    
    let result = parse_markdown(invalid_content);
    
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("must start with YAML"));
}
```

## Testing with Multiple Cases

```rust
#[test]
fn test_multiple_inputs() {
    let test_cases = vec![
        ("input1", "expected1"),
        ("input2", "expected2"),
        ("input3", "expected3"),
    ];
    
    for (input, expected) in test_cases {
        let result = sanitize_filename(input);
        assert_eq!(result, expected, "Failed for input: {}", input);
    }
}
```

## Testing HashMap Configurations

```rust
#[test]
fn test_with_hashmap() {
    let mut mcps = HashMap::new();
    mcps.insert("ado".to_string(), McpConfig::Enabled(true));
    mcps.insert("es-chat".to_string(), McpConfig::Enabled(true));
    
    let result = generate_copilot_params(&mcps);
    
    assert!(result.contains("--prompt"));
    // MCPs are handled via the MCP firewall, not --mcp flags
    assert!(!result.contains("--mcp ado"));
    assert!(!result.contains("--mcp es-chat"));
}
```

## Testing with Complex Options

```rust
#[test]
fn test_with_options() {
    let mut mcps = HashMap::new();
    mcps.insert(
        "custom-tool".to_string(),
        McpConfig::WithOptions(McpOptions {
            command: Some("node".to_string()),
            args: vec!["server.js".to_string()],
            allowed: vec!["func1".to_string()],
            env: HashMap::new(),
        }),
    );
    
    let result = generate_copilot_params(&mcps);
    
    assert!(!result.contains("--mcp custom-tool"));
}
```

## Integration Test Pattern

```rust
#[test]
fn test_integration_scenario() {
    // Create test environment
    let temp_dir = std::env::temp_dir().join(format!("test-{}", std::process::id()));
    fs::create_dir_all(&temp_dir).expect("Failed to create temp dir");
    
    // Set up test files
    let test_file = temp_dir.join("test.md");
    fs::write(&test_file, "test content").expect("Failed to write test file");
    
    // Run test assertions
    assert!(test_file.exists());
    
    // Cleanup
    let _ = fs::remove_dir_all(&temp_dir);
}
```

## Testing File Content

```rust
#[test]
fn test_file_content() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("examples")
        .join("sample-agent.md");
    
    assert!(path.exists(), "File should exist");
    
    let content = fs::read_to_string(&path)
        .expect("Should be able to read file");
    
    assert!(content.starts_with("---"));
    assert!(content.contains("name:"));
}
```

## Testing for Deterministic Behavior

```rust
#[test]
fn test_deterministic() {
    let result1 = generate_schedule("agent", "daily");
    let result2 = generate_schedule("agent", "daily");
    
    assert_eq!(result1, result2, "Function should be deterministic");
}
```

## Testing Empty/Null Cases

```rust
#[test]
fn test_empty_input() {
    let repos: Vec<Repository> = vec![];
    let result = generate_repositories(&repos);
    
    assert_eq!(result, "");
}
```

## Testing String Contains

```rust
#[test]
fn test_output_format() {
    let schedule = generate_schedule("test", "hourly");
    
    assert!(schedule.contains("schedules:"));
    assert!(schedule.contains("cron:"));
    assert!(schedule.contains("branches:"));
    assert!(schedule.contains("main"));
}
```

## Testing String Patterns

```rust
#[test]
fn test_string_pattern() {
    let schedule = generate_schedule("test", "daily");
    
    // Should have specific pattern
    assert!(schedule.contains("cron:"));
    
    // Should NOT have hourly pattern
    assert!(!schedule.contains("* * * * *"));
}
```

## Testing Multiple Assertions

```rust
#[test]
fn test_multiple_assertions() {
    let repos = vec![
        Repository {
            repository: "repo1".to_string(),
            repo_type: "git".to_string(),
            name: "org/repo1".to_string(),
            repo_ref: "refs/heads/main".to_string(),
        },
        Repository {
            repository: "repo2".to_string(),
            repo_type: "git".to_string(),
            name: "org/repo2".to_string(),
            repo_ref: "refs/heads/dev".to_string(),
        },
    ];
    
    let result = generate_repositories(&repos);
    
    // Test for first repo
    assert!(result.contains("repository: repo1"));
    assert!(result.contains("org/repo1"));
    
    // Test for second repo
    assert!(result.contains("repository: repo2"));
    assert!(result.contains("org/repo2"));
    assert!(result.contains("refs/heads/dev"));
}
```

## Testing Result Types

```rust
#[test]
fn test_result_ok() {
    let content = r#"---
name: "Test"
description: "Test description"
---

Body content"#;
    
    let result = parse_markdown(content);
    
    assert!(result.is_ok());
    
    let (front_matter, body) = result.unwrap();
    assert_eq!(front_matter.name, "Test");
    assert!(body.contains("Body content"));
}

#[test]
fn test_result_err() {
    let content = "No front matter";
    
    let result = parse_markdown(content);
    
    assert!(result.is_err());
}
```

## Best Practices

### 1. Use Descriptive Test Names
```rust
// Good
#[test]
fn test_sanitize_filename_removes_special_characters() { }

// Less clear
#[test]
fn test_sanitize() { }
```

### 2. Include Assertion Messages
```rust
// Good
assert!(path.exists(), "Config file should exist at {}", path.display());

// Less informative
assert!(path.exists());
```

### 3. Test One Thing Per Test
```rust
// Good - separate tests
#[test]
fn test_empty_input() { }

#[test]
fn test_single_item() { }

#[test]
fn test_multiple_items() { }

// Less maintainable - one large test
#[test]
fn test_all_cases() { 
    // Tests empty, single, and multiple in one function
}
```

### 4. Use Constants for Test Data
```rust
const TEST_AGENT_NAME: &str = "Test Agent";
const TEST_DESCRIPTION: &str = "Test description";

#[test]
fn test_with_constants() {
    let config = create_config(TEST_AGENT_NAME, TEST_DESCRIPTION);
    // ...
}
```

### 5. Clean Up Resources
```rust
#[test]
fn test_with_cleanup() {
    let temp_dir = create_temp_dir();
    
    // Test code here
    
    // Always cleanup, even if test fails
    let _ = fs::remove_dir_all(&temp_dir);
}
```

## Running Specific Test Patterns

```bash
# Run all tests with "sanitize" in the name
cargo test sanitize

# Run all tests in a specific module
cargo test tests::

# Run integration tests only
cargo test --test compiler_tests

# Run with output visible
cargo test -- --nocapture

# Run tests sequentially (not parallel)
cargo test -- --test-threads=1
```

## Common Test Assertions

```rust
// Equality
assert_eq!(actual, expected);
assert_ne!(actual, unexpected);

// Boolean
assert!(condition);
assert!(condition, "Custom error message");

// String contains
assert!(string.contains("substring"));
assert!(string.starts_with("prefix"));
assert!(string.ends_with("suffix"));

// Result/Option
assert!(result.is_ok());
assert!(result.is_err());
assert!(option.is_some());
assert!(option.is_none());

// Collections
assert!(vec.is_empty());
assert_eq!(vec.len(), 3);
assert!(vec.contains(&item));
```

## Debugging Tests

```rust
#[test]
fn test_with_debug_output() {
    let result = generate_schedule("test", "daily");
    
    // Print for debugging (visible with --nocapture)
    println!("Generated schedule: {}", result);
    eprintln!("Debug info: {:?}", result);
    
    assert!(result.contains("cron:"));
}
```

Run with: `cargo test test_with_debug_output -- --nocapture`
