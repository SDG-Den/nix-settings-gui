{
  description = "nix-settings-gui: a comment-driven settings GUI for Nix";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = {
    self,
    nixpkgs,
    flake-utils,
    ...
  }:
    flake-utils.lib.eachDefaultSystem (
      system: let
        pkgs = import nixpkgs {inherit system;};
        inherit (pkgs) lib;

        runtimePath = lib.makeBinPath (with pkgs; [
          nix
          nixd
          nixfmt
        ]);

        nativeBuildInputs = with pkgs; [
          pkg-config
          wrapGAppsHook4
        ];

        buildInputs = with pkgs; [
          gtk4
          libadwaita
          glib
        ];

        nix-settings-gui = pkgs.rustPlatform.buildRustPackage {
          pname = "nix-settings-gui";
          version = "0.1.0";
          src = lib.cleanSource ./.;
          cargoLock.lockFile = ./Cargo.lock;

          inherit nativeBuildInputs buildInputs;

          doCheck = true;

          preFixup = ''
            gappsWrapperArgs+=(--prefix PATH : "${runtimePath}")
          '';

          postInstall = ''
            install -Dm644 ${./nix-settings-gui.desktop} \
              "$out/share/applications/nix-settings-gui.desktop"
          '';

          meta = with lib; {
            description = "A comment-driven settings GUI for Nix";
            mainProgram = "nix-settings-gui";
            license = licenses.gpl3Plus;
            platforms = platforms.linux;
          };
        };
      in {
        packages = {
          inherit nix-settings-gui;
          default = nix-settings-gui;
        };

        apps.default = {
          type = "app";
          program = "${nix-settings-gui}/bin/nix-settings-gui";
        };

        checks.default = nix-settings-gui;

        devShells.default = pkgs.mkShell {
          nativeBuildInputs =
            nativeBuildInputs
            ++ (with pkgs; [
              cargo
              rustc
              rustfmt
              clippy
              rust-analyzer
            ]);

          buildInputs =
            buildInputs
            ++ (with pkgs; [
              nix
              nixd
              nixfmt
            ]);

          RUST_SRC_PATH = pkgs.rustPlatform.rustLibSrc;
        };
      }
    )
    // {
      nixosModules.default = import ./modules/nixos.nix self;
      nixosModules.nix-settings-gui = import ./modules/nixos.nix self;
    };
}
