## Rust devShell for cc-switch cross-compilation via cargo-zigbuild.
## Usage:
##   cd src-tauri && nix develop
##   cargo zigbuild --target x86_64-unknown-linux-gnu --release
##   cargo zigbuild --target aarch64-unknown-linux-gnu --release
##
## cargo-zigbuild uses zig as the C cross-compiler, so no container
## runtime (podman/docker) is needed. rusqlite bundled SQLite and
## rquickjs QuickJS C code are compiled by zig automatically.

{
  description = "Rust devShell for cc-switch-tui (cargo-zigbuild cross-compilation)";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs = { self, nixpkgs }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" "x86_64-darwin" "aarch64-darwin" ];
      forAllSystems = f:
        nixpkgs.lib.genAttrs systems (system:
          f system (import nixpkgs { inherit system; }));
    in
    {
      devShells = forAllSystems (system: pkgs:
        let
          isDarwin = pkgs.stdenv.isDarwin;
          crossTargets = [
            "x86_64-unknown-linux-gnu"
            "aarch64-unknown-linux-gnu"
          ];
          crossTargetsArm = pkgs.lib.optionals isDarwin [
            "aarch64-apple-darwin"
          ];
          allTargets = crossTargets ++ crossTargetsArm;
          targetListCmd = builtins.concatStringsSep " " (map (t: "rustup target add ${t}") allTargets);
        in
        {
          default = pkgs.mkShell {
            name = "cc-switch-rust";

            packages = with pkgs; [
              ## cross-compilation via zig (no container needed)
              cargo-zigbuild
              zig

              ## rusqlite bundled SQLite needs cmake
              cmake

              ## rustup manages toolchain + cross targets
              ## (rust-toolchain.toml in this dir pins the version)
              rustup

              ## dev helpers
              cargo-watch
            ];

            shellHook = ''
              ## Install pinned toolchain if missing (rustup stores in ~/.rustup/)
              if ! rustup toolchain list 2>/dev/null | grep -q '1.94.0'; then
                echo "Installing rustup 1.94.0 toolchain..."
                rustup toolchain install 1.94.0 --profile minimal --no-self-update 2>&1 | tail -1
              fi

              ## Ensure rustup reads local rust-toolchain.toml
              export RUSTUP_TOOLCHAIN=1.94.0

              ## Add cross-compilation targets
              for tgt in ${builtins.toString allTargets}; do
                if ! rustup target list --installed 2>/dev/null | grep -q "^$tgt$"; then
                  echo "Adding rustup target: $tgt"
                  rustup target add "$tgt" 2>&1 | tail -1
                fi
              done

              echo ""
              echo "cc-switch Rust devShell (cargo-zigbuild)"
              echo "========================================"
              echo "Cross-compile:"
              echo "  cargo zigbuild --target x86_64-unknown-linux-gnu --release"
              echo "  cargo zigbuild --target aarch64-unknown-linux-gnu --release"
              ${pkgs.lib.optionalString isDarwin ''
              echo "  cargo zigbuild --target aarch64-apple-darwin --release"
              ''}
              echo ""
              echo "Native build:"
              echo "  cargo build --release"
              echo ""
            '';
          };
        });
    };
}
