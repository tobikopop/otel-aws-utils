# Dev shell for the otel-aws-utils crate (same devenv pattern as the sibling
# projects). The crate is a plain cargo package consumed as a git dependency —
# there is nothing to build or deploy from here.
{
  pkgs,
  lib,
  config,
  inputs,
  ...
}:
{
  env = {
    DEVENV_SANDBOX = true;
  };

  packages = with pkgs; [
    git
  ];

  languages = {
    # rust-toolchain.toml is the single source for the Rust release.
    rust = {
      enable = true;
      toolchainFile = ./rust-toolchain.toml;
    };
  };

  scripts = {
    check.exec = ''
      echo "==> cargo fmt";    cargo fmt --all --check
      echo "==> cargo clippy";  cargo clippy --all-targets -- -D warnings
      echo "==> cargo test";    cargo test --locked
      echo "All checks passed"
    '';
  };
}
