{
  description = "Safe-Rust Bambu Studio rewrite (iced + wgpu/Vulkan)";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      rust-overlay,
    }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      forAllSystems = nixpkgs.lib.genAttrs systems;
      pkgsFor =
        system:
        import nixpkgs {
          inherit system;
          overlays = [ rust-overlay.overlays.default ];
        };
      rustFor =
        pkgs:
        pkgs.rust-bin.stable.latest.default.override {
          extensions = [
            "rustfmt"
            "clippy"
          ];
        };
      srcFor =
        pkgs:
        pkgs.lib.fileset.toSource {
          root = ./.;
          fileset = pkgs.lib.fileset.unions [
            ./Cargo.toml
            ./Cargo.lock
            ./crates
            ./resources
            ./tests
            ./rust-toolchain.toml
            ./LICENSE
            ./README.md
          ];
        };
    in
    {
      packages = forAllSystems (
        system:
        let
          pkgs = pkgsFor system;
          rust = rustFor pkgs;
          rustPlatform = pkgs.makeRustPlatform {
            rustc = rust;
            cargo = rust;
          };
          gpuLibs = with pkgs; [
            vulkan-loader
            wayland
            libxkbcommon
            libGL
            libx11
            libxcursor
            libxi
            libxrandr
          ];
          common = {
            pname = "bambu-studio-rs";
            version = "0.1.0";
            src = srcFor pkgs;
            cargoLock.lockFile = ./Cargo.lock;
            auditable = false;
            nativeBuildInputs = [
              pkgs.pkg-config
              pkgs.makeWrapper
            ];
            buildInputs = gpuLibs ++ [
              pkgs.openssl
            ];
            env = {
              WGPU_BACKEND = "vulkan";
            };
          };
          wrapVulkan =
            drv:
            drv.overrideAttrs (old: {
              postFixup = (old.postFixup or "") + ''
                for bin in "$out"/bin/*; do
                  wrapProgram "$bin" \
                    --set WGPU_BACKEND vulkan \
                    --prefix LD_LIBRARY_PATH : ${pkgs.lib.makeLibraryPath gpuLibs}
                done
              '';
            });
        in
        rec {
          bambu-cli = wrapVulkan (
            rustPlatform.buildRustPackage (
              common
              // {
                pname = "bambu-cli";
                cargoBuildFlags = [ "-p" "bambu-cli" ];
                cargoTestFlags = [ "-p" "bambu-cli" "-p" "bambu-geom" "-p" "bambu-slicer" ];
                doCheck = true;
              }
            )
          );
          bambu-ui = wrapVulkan (
            rustPlatform.buildRustPackage (
              common
              // {
                pname = "bambu-ui";
                cargoBuildFlags = [ "-p" "bambu-ui" ];
                doCheck = false;
              }
            )
          );
          default = bambu-cli;
          bambu-studio-rs = bambu-ui;
        }
      );

      devShells = forAllSystems (
        system:
        let
          pkgs = pkgsFor system;
          rust = rustFor pkgs;
          gpuLibs = with pkgs; [
            vulkan-loader
            wayland
            libxkbcommon
            libGL
            libx11
            libxcursor
            libxi
            libxrandr
            pkg-config
            openssl
          ];
        in
        {
          default = pkgs.mkShell {
            packages = [
              rust
              pkgs.cargo-deny
              pkgs.pkg-config
            ] ++ gpuLibs;
            WGPU_BACKEND = "vulkan";
            LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath gpuLibs;
          };
        }
      );

      formatter = forAllSystems (system: (pkgsFor system).nixfmt-rfc-style);
    };
}
