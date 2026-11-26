{
  description = "Linux executable wrappers and reproducible artifact editing";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-parts = {
      url = "github:hercules-ci/flake-parts";
      inputs.nixpkgs-lib.follows = "nixpkgs";
    };
    git-hooks-nix = {
      url = "github:cachix/git-hooks.nix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    treefmt-nix = {
      url = "github:numtide/treefmt-nix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    inputs:
    inputs.flake-parts.lib.mkFlake { inherit inputs; } {
      systems = [
        "aarch64-linux"
        "x86_64-linux"
      ];

      imports = [
        inputs.treefmt-nix.flakeModule
        inputs.git-hooks-nix.flakeModule
      ];

      perSystem =
        {
          config,
          lib,
          pkgs,
          ...
        }:
        let
          cargoArgs = {
            version = (lib.importTOML ./Cargo.toml).package.version;
            src = lib.fileset.toSource {
              root = ./.;
              fileset = lib.fileset.unions [
                ./Cargo.lock
                ./Cargo.toml
                ./examples
                ./src
              ];
            };
            cargoLock.lockFile = ./Cargo.lock;
            buildFeatures = [ "wrapper" ];
          };

        in
        {
          packages = {
            default = config.packages.tool-wrapper;
            install-adjacent-wrapper-hook = pkgs.makeSetupHook {
              name = "install-adjacent-wrapper-hook";
            } ./nix/install-adjacent-wrapper.sh;
            tool-wrapper = pkgs.pkgsStatic.rustPlatform.buildRustPackage (
              cargoArgs
              // {
                pname = "tool-wrapper";
                doCheck = false;
                allowedReferences = [ ];
                stripDebugFlags = [
                  "--strip-all"
                  "-p"
                ];
                env = {
                  CARGO_PROFILE_RELEASE_LTO = "fat";
                  CARGO_PROFILE_RELEASE_CODEGEN_UNITS = "1";
                  CARGO_PROFILE_RELEASE_OPT_LEVEL = "z";
                };
                meta = {
                  description = "Pass-through Linux executable shim";
                  mainProgram = "tool-wrapper";
                  license = lib.licenses.mit;
                  platforms = lib.platforms.linux;
                };
              }
            );
          };

          checks = {
            rust = pkgs.rustPlatform.buildRustPackage (
              cargoArgs
              // {
                pname = "tool-wrapper-rust-check";
                checkType = "debug";
                nativeCheckInputs = [ pkgs.clippy ];
                cargoTestFlags = [
                  "--locked"
                  "--all-features"
                  "--all-targets"
                ];
                postCheck = ''
                  cargo clippy --locked --offline --all-features --all-targets -- -D warnings
                '';
              }
            );

            process =
              pkgs.runCommand "tool-wrapper-process-check"
                {
                  nativeBuildInputs = [
                    pkgs.binutils
                    config.packages.install-adjacent-wrapper-hook
                  ];
                }
                ''
                  wrapper=${lib.getExe config.packages.tool-wrapper}
                  headers=$(readelf -lW -dW "$wrapper")
                  if grep -Eq '^[[:space:]]+INTERP[[:space:]]|\(NEEDED\)' <<< "$headers"; then
                    echo "Expected a static executable: $wrapper" >&2
                    exit 1
                  fi
                  source ${./nix/tests/install-adjacent-wrapper.sh} "$wrapper" ${pkgs.runtimeShell}
                  touch "$out"
                '';
          };

          devShells.default = pkgs.mkShell {
            inputsFrom = [ config.pre-commit.devShell ];
            packages = [
              pkgs.cargo
              pkgs.clippy
              pkgs.rustc
              pkgs.rustfmt
            ];
          };

          pre-commit.settings.hooks = {
            treefmt = {
              enable = true;
              package = config.treefmt.build.wrapper;
            };
            deadnix.enable = true;
            nil.enable = true;
            statix.enable = true;
          };

          treefmt = {
            projectRootFile = "flake.nix";
            settings.excludes = [
              ".direnv/**"
              "**/target/**"
              "result"
              "result-*"
            ];
            programs = {
              nixfmt.enable = true;
              rustfmt.enable = true;
            };
          };
        };
    };
}
