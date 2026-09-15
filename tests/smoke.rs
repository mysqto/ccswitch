//! Smoke test: the crate's public pure helpers behave as documented.

use ccswitch::cli;
use ccswitch::ClaudeEnv;
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
    let env = ClaudeEnv::default();
    let claude = env.config_dir(home);
    assert_eq!(claude, home.join(".claude"));
    assert_eq!(cli::ccswitch_home(None, &claude), claude.join("accounts"));
    assert_eq!(cli::isolate_home(None, &claude), claude.join("profiles"));
    assert_eq!(env.config_path(home), home.join(".claude.json"));
    assert_eq!(env.keychain_service(), "Claude Code-credentials");
}

#[test]
fn claude_config_dir_moves_the_whole_account_including_the_keychain_item() {
    let home = Path::new("/home/example");
    let env = ClaudeEnv::new(Some("/opt/claude-work".to_string()), None, None);
    let claude = env.config_dir(home);
    assert_eq!(claude, Path::new("/opt/claude-work"));
    assert_eq!(env.config_path(home), claude.join(".claude.json"));
    assert_eq!(env.credentials_dir(home), claude);
    assert_eq!(cli::ccswitch_home(None, &claude), claude.join("accounts"));
    // Crucially a *different* Keychain item from the default account's.
    assert_ne!(
        env.keychain_service(),
        ClaudeEnv::default().keychain_service()
    );
    assert!(env
        .keychain_service()
        .starts_with("Claude Code-credentials-"));
}
