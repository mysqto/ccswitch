//! Smoke test: the crate's public pure helpers behave as documented.

use ccswitch::cli;
use std::path::Path;

#[test]
fn help_text_and_reserved_words_are_public() {
    assert!(cli::help_text().contains("ccswitch <name>"));
    assert!(cli::is_reserved("list"));
    assert!(!cli::is_reserved("dev"));
}

#[test]
fn default_paths_resolve_under_home() {
    let home = Path::new("/home/example");
    assert_eq!(
        cli::ccswitch_home(None, home),
        home.join(".claude").join("accounts")
    );
    assert_eq!(cli::config_path(home), home.join(".claude.json"));
}
