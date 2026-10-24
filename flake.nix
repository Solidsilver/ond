{
  description = "Ond: a native music player for Navidrome";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { self, nixpkgs }:
    let
      inherit (nixpkgs) lib;
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "aarch64-darwin"
        "x86_64-darwin"
      ];
      forAll = f: lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
    in
    {
      # Linux only: on macOS, Ond ships as an app bundle (scripts/bundle-macos.sh).
      packages = forAll (
        pkgs:
        lib.optionalAttrs pkgs.stdenv.hostPlatform.isLinux rec {
          ond = pkgs.callPackage ./nix/package.nix { };
          default = ond;
        }
      );

      # For a NixOS or home-manager config: `nixpkgs.overlays = [ ond.overlays.default ];`
      overlays.default = final: _prev: {
        ond = final.callPackage ./nix/package.nix { };
      };

      devShells = forAll (pkgs: {
        default = pkgs.mkShell {
          # The package's build inputs (ALSA, PipeWire, pkg-config, libclang).
          inputsFrom =
            lib.optional pkgs.stdenv.hostPlatform.isLinux
              self.packages.${pkgs.stdenv.hostPlatform.system}.ond;
          packages = with pkgs; [
            cargo
            rustc
            clippy
            rustfmt
            rust-analyzer
          ];
          env = {
            # rust-analyzer reads the standard library's source from here.
            RUST_SRC_PATH = "${pkgs.rustPlatform.rustLibSrc}";
          }
          // lib.optionalAttrs pkgs.stdenv.hostPlatform.isLinux {
            # `cargo run` loads the display and GPU libraries the same way the
            # package does, but without its RPATH.
            LD_LIBRARY_PATH =
              lib.makeLibraryPath
                self.packages.${pkgs.stdenv.hostPlatform.system}.ond.runtimeLibs;
          };
        };
      });

      checks = forAll (
        pkgs:
        lib.optionalAttrs pkgs.stdenv.hostPlatform.isLinux {
          ond = self.packages.${pkgs.stdenv.hostPlatform.system}.ond;
        }
      );

      formatter = forAll (pkgs: pkgs.nixfmt);
    };
}
