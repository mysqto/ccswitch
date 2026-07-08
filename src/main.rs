//! Thin binary entry point that delegates to the CLI adapter.

fn main() -> anyhow::Result<()> {
    ccswitch::cli_shim::run()
}
