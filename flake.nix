{
  description = "mxt, a terminal Matrix client with nvim-like keybindings";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { self, nixpkgs }:
    let
      inherit (nixpkgs) lib;
      # nixpkgs dropped x86_64-darwin in 2026.
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "aarch64-darwin"
      ];
      forAllSystems = f: lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
      workspace = (lib.importTOML ./Cargo.toml).workspace.package;
    in
    {
      packages = forAllSystems (pkgs: {
        default = pkgs.rustPlatform.buildRustPackage {
          pname = "mxt";
          inherit (workspace) version;
          # Only what Cargo needs, so editing docs does not rebuild the package.
          src = lib.fileset.toSource {
            root = ./.;
            fileset = lib.fileset.unions [
              ./Cargo.toml
              ./Cargo.lock
              ./crates
            ];
          };
          cargoLock.lockFile = ./Cargo.lock;
          cargoBuildFlags = [ "--package" "mxt" ];
          cargoTestFlags = [ "--workspace" ];
          # matrix-sdk needs these (the same inputs as nixpkgs' iamb package).
          nativeBuildInputs = [ pkgs.pkg-config ];
          buildInputs = [
            pkgs.sqlite
            pkgs.aws-lc
          ];
          meta = {
            description = "A terminal Matrix client with nvim-like keybindings";
            homepage = "https://github.com/kisepichu/mxt";
            license = lib.licenses.mit;
            mainProgram = "mxt";
          };
        };
      });

      devShells = forAllSystems (pkgs: {
        default = pkgs.mkShell {
          inputsFrom = [ self.packages.${pkgs.stdenv.hostPlatform.system}.default ];
          packages = with pkgs; [
            cargo
            rustc
            clippy
            rustfmt
            rust-analyzer
            cargo-deny
            prek
          ];
          RUST_SRC_PATH = "${pkgs.rustPlatform.rustLibSrc}";
        };
      });

      checks = forAllSystems (pkgs: {
        package = self.packages.${pkgs.stdenv.hostPlatform.system}.default;
      });
    };
}
