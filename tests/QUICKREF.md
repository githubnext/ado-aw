# Quick Test Reference

## Run Tests

```bash
# All tests
cargo test

# Unit tests only
cargo test --lib

# Integration tests only  
cargo test --test compiler_tests

# Specific test
cargo test test_sanitize_filename

# With output
cargo test -- --nocapture
```

## Test Structure

```
ado-aw/
├── src/
│   └── main.rs              # 18 unit tests (line 337+)
└── tests/
    ├── compiler_tests.rs    # 8 integration tests
    ├── fixtures/            # Test data
    │   ├── minimal-agent.md
    │   └── complete-agent.md
    ├── README.md            # Full testing guide
    ├── SUMMARY.md           # Implementation details
    ├── VERIFICATION.md      # Acceptance criteria proof
    └── EXAMPLES.md          # Code patterns & best practices
```

## Test Count

- **Unit Tests**: 18
- **Integration Tests**: 8
- **Total**: 26 tests
- **Fixtures**: 2 files

## Coverage

✅ Filename sanitization  
✅ Schedule generation (hourly/daily)  
✅ Repository configuration  
✅ Checkout steps  
✅ Copilot parameters  
✅ Markdown parsing  
✅ Error handling  
✅ Edge cases  
✅ Template validation  

## Quick Test Example

```rust
#[test]
fn test_example() {
    let result = sanitize_filename("Hello World!");
    assert_eq!(result, "hello-world");
}
```

## Need Help?

- Full guide: `tests/README.md`
- Examples: `tests/EXAMPLES.md`
- Requirements: `tests/VERIFICATION.md`
