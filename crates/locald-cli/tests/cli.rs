#[test]
fn cli_tests() {
    let t = trycmd::TestCases::new();
    // Set sandbox mode for consistent behavior
    t.env("LOCALD_SANDBOX_ACTIVE", "1");
    t.case("tests/cmd/*.md");
}
