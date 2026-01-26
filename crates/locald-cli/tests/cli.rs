#[test]
fn cli_tests() {
    let t = trycmd::TestCases::new();
    // Set sandbox mode for consistent behavior
    t.env("LOCALD_SANDBOX_ACTIVE", "1");

    t.case("tests/cmd/ai-schema.md");
    t.case("tests/cmd/doctor-json.md");
    t.case("tests/cmd/docs-cli.md");
    t.case("tests/cmd/error-messages.md");
    t.case("tests/cmd/help-subcommands.md");
    t.case("tests/cmd/version.md");

    if cfg!(feature = "experimental-cnb")
        || cfg!(feature = "experimental-containers")
        || cfg!(feature = "experimental-plugins")
        || cfg!(feature = "experimental-vmm")
    {
        t.case("tests/cmd/help-nightly.md");
    } else {
        t.case("tests/cmd/help.md");
    }
}
