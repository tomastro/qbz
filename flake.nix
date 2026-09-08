{
  description = "QBZ — Native hi-fi Qobuz desktop player for Linux";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs { inherit system; };

        # ──────────────────────────────────────────────
        # VERSION BUMP: update version, rev, and srcHash when tagging a new
        # release. QBZ builds from the Qt crates/ workspace; there is no Node
        # dependency graph.
        # ──────────────────────────────────────────────
        qbzVersion = "2.1.0";
        qbzRev     = "v${qbzVersion}";
        # POST-RELEASE: replace from the published v2.1.0 tag.
        srcHash    = "sha256-Yc5f7DjAFAjYgTnT2yv7zjIxmXPJ8ZdLaoPnHJoFRX0=";

        # Libraries opened by name at runtime rather than linked into qbz.
        # Qt's own graphics/plugin closure is handled by wrapQtAppsHook.
        runtimeLibs = with pkgs; [
          libjack2
        ];
        runtimeBins = with pkgs; [
          pipewire
          pulseaudio
          xdg-utils
        ];
      in
      {
        packages.default = pkgs.rustPlatform.buildRustPackage rec {
          pname = "qbz";
          version = qbzVersion;

          src = pkgs.fetchFromGitHub {
            owner = "vicrodh";
            repo  = "qbz";
            rev   = qbzRev;
            hash  = srcHash;
          };

          cargoRoot = "crates";
          buildAndTestSubdir = cargoRoot;
          # Build only the app binary, not every workspace member's tests.
          cargoBuildFlags = [ "-p" "qbz-qt" ];

          cargoLock = {
            lockFile = "${src}/crates/Cargo.lock";
          };

          # cxx-qt needs qtbase + qtdeclarative with their private headers (the
          # RHI items include <rhi/qrhi.h>). qsb is deliberately absent: the
          # audited, committed shaders ship unchanged. qtwayland provides the
          # Wayland platform plugin and qtsvg the SVG image plugin.
          # wrapQtAppsHook sets the plugin/QML paths the binary needs at run
          # time. The Qt 6 package set is above QBZ's 6.8 floor.
          nativeBuildInputs = with pkgs; [
            pkg-config
            cmake
            nasm
            qt6.qmake
            qt6.wrapQtAppsHook
          ];

          buildInputs = with pkgs; [
            alsa-lib
            libjack2
            qt6.qtbase
            qt6.qtdeclarative
            qt6.qtsvg
            qt6.qtwayland
          ];

          # Tests need an offscreen QPA + D-Bus the sandbox does not have;
          # the crates are tested in the repo's CI (test-crates.yml).
          doCheck = false;

          postInstall = ''
            # wrapQtAppsHook wraps $out/bin/qbz. Add only runtime helpers and
            # libraries that QBZ itself opens by name.
            qtWrapperArgs+=(--prefix PATH : ${pkgs.lib.makeBinPath runtimeBins})
            qtWrapperArgs+=(--prefix LD_LIBRARY_PATH : ${pkgs.lib.makeLibraryPath runtimeLibs})

            install -Dm644 $src/packaging/linux/qbz.desktop \
              $out/share/applications/com.blitzfc.qbz.desktop
            install -Dm644 $src/packaging/flatpak/com.blitzfc.qbz.metainfo.xml \
              $out/share/metainfo/com.blitzfc.qbz.metainfo.xml
            for size in 32 48 64 128 256 512; do
              install -Dm644 $src/packaging/icons/"$size"x"$size".png \
                $out/share/icons/hicolor/"$size"x"$size"/apps/qbz.png
            done
            install -Dm644 $src/LICENSE $out/share/licenses/qbz/LICENSE
            cp -r $src/licenses $out/share/licenses/qbz/third-party
          '';

          meta = with pkgs.lib; {
            description = "Native, full-featured hi-fi Qobuz desktop player for Linux";
            homepage = "https://qbz.lol";
            license = licenses.mit;
            mainProgram = "qbz";
            platforms = platforms.linux;
          };
        };

        # Dev shell with all build dependencies
        devShells.default = pkgs.mkShell {
          inputsFrom = [ self.packages.${system}.default ];

          packages = with pkgs; [
            rust-analyzer
            rustfmt
            clippy
          ];

          # The package's `postInstall` wraps the installed binary with
          # LD_LIBRARY_PATH for the dlopen'd JACK backend. Inside `nix develop`
          # we run `crates/target/debug/qbz` directly, with no wrapper, so
          # replicate it here.
          shellHook = ''
            export LD_LIBRARY_PATH="${pkgs.lib.makeLibraryPath runtimeLibs}''${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
          '';
        };
      });
}
