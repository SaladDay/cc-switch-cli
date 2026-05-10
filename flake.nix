{
  description = "Nix packaging for cc-switch-tui";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs = { self, nixpkgs }:
    let
      cargoManifest = builtins.fromTOML (builtins.readFile ./src-tauri/Cargo.toml);
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];
      forAllSystems = f:
        nixpkgs.lib.genAttrs systems (system:
          f system (import nixpkgs { inherit system; }));
    in
    {
      packages = forAllSystems (system: pkgs:
        let
          cc_switch_tui = pkgs.rustPlatform.buildRustPackage {
            pname = cargoManifest.package.name;
            version = cargoManifest.package.version;

            src = pkgs.lib.cleanSource ./.;

            cargoRoot = "src-tauri";
            buildAndTestSubdir = "src-tauri";
            cargoLock = {
              lockFile = ./src-tauri/Cargo.lock;
            };

            # The upstream repository owns the Rust test suite. The flake package is
            # intended to build and install the CLI on NixOS without depending on
            # host-specific assistant CLIs or live config fixtures during checkPhase.
            doCheck = false;

            meta = with pkgs.lib; {
              description = "TUI manager for Claude Code, Codex, Gemini, OpenCode, OpenClaw, and Hermes";
              homepage = "https://github.com/handy-sun/cc-switch-tui";
              license = licenses.mit;
              mainProgram = "cc-switch-tui";
              platforms = platforms.unix;
            };
          };
        in
        {
          cc-switch-tui = cc_switch_tui;
          # legacy alias kept for transition
          cc-switch = cc_switch_tui;
          default = cc_switch_tui;
        });
    };
}
