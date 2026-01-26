#[test]
fn cli_tests() {
    let t = trycmd::TestCases::new();
    t.case("tests/cmd/*.md");
    // Set sandbox mode for consistent behavior
    t.env("LOCALD_SANDBOX_ACTIVE", "1");
}
