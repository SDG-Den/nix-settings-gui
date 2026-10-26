{
  description = "nix-settings-gui: a comment-driven settings GUI for Nix";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    tinted-theming = {
      url = "github:tinted-theming/schemes";
      flake = false;
    };
  };

  outputs = {
    self,
    nixpkgs,
    flake-utils,
    tinted-theming,
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
          wlr-randr
        ]);

        nativeBuildInputs = with pkgs; [
          pkg-config
          mold
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
          src = lib.fileset.toSource {
            root = ./.;
            fileset = lib.fileset.unions [
              (lib.fileset.fileFilter (f: f.hasExt "rs") ./.)
              (lib.fileset.fileFilter (f: f.name == "Cargo.toml" || f.name == "Cargo.lock" || f.name == "org.nix_settings_gui.NixSettingsGui.desktop") ./.)
              ./modules
              ./src
              ./tests
            ];
          };
          cargoLock.lockFile = ./Cargo.lock;

          TINTED_SCHEMES_DIR = "${tinted-theming}";

          RUSTFLAGS = "-C link-arg=-fuse-ld=mold";
          inherit nativeBuildInputs buildInputs;

          doCheck = true;

          preFixup = ''
            gappsWrapperArgs+=(--prefix PATH : "${runtimePath}")
          '';

          postInstall = ''
            install -Dm644 ${./org.nix_settings_gui.NixSettingsGui.desktop} \
              "$out/share/applications/org.nix_settings_gui.NixSettingsGui.desktop"
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

          TINTED_SCHEMES_DIR = "${tinted-theming}";
          RUST_SRC_PATH = pkgs.rustPlatform.rustLibSrc;
        };
      }
    )
    // {
      nixosModules.default = import ./modules/nixos.nix self;
      nixosModules.nix-settings-gui = import ./modules/nixos.nix self;
    };
}
