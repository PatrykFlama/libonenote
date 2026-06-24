{
  description = "Unofficial high-level API and CLI for Microsoft OneNote files";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { self, nixpkgs, ... }:
    let
      supportedSystems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      forAllSystems = nixpkgs.lib.genAttrs supportedSystems;
    in
    {
      packages = forAllSystems (
        system:
        let
          pkgs = import nixpkgs { inherit system; };
        in
        {
          default = pkgs.rustPlatform.buildRustPackage {
            pname = "onenote-cli";
            version = "0.1.0";
            src = pkgs.lib.fileset.toSource {
              root = ./.;
              fileset = pkgs.lib.fileset.unions [
                ./Cargo.toml
                ./Cargo.lock
                ./src
                ./crates
                ./README.md
                ./LICENSE
              ];
            };
            cargoHash = "sha256-o8SiwibhTJSjglWosPSFSTYtlTiWDfup5veHyt3SS4U=";
            cargoBuildFlags = [
              "-p"
              "onenote-cli"
            ];
            cargoTestFlags = [
              "-p"
              "libonenote"
              "-p"
              "onenote-cli"
            ];
            cargoInstallFlags = [
              "-p"
              "onenote-cli"
            ];

            meta = {
              description = "Inspect and extract Microsoft OneNote files";
              homepage = "https://github.com/PatrykFlama/libonenote";
              license = pkgs.lib.licenses.mit;
              mainProgram = "onenote";
              platforms = pkgs.lib.platforms.linux;
            };
          };
        }
      );

      apps = forAllSystems (system: {
        default = {
          type = "app";
          program = "${self.packages.${system}.default}/bin/onenote";
          meta.description = "Inspect and extract OneNote files";
        };
      });

      devShells = forAllSystems (
        system:
        let
          pkgs = import nixpkgs { inherit system; };
        in
        {
          default = pkgs.mkShell {
            packages = [
              pkgs.cargo
              pkgs.rustc
              pkgs.rustfmt
              pkgs.clippy
            ];
          };
        }
      );
    };
}
