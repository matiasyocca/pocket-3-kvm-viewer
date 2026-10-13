{
  description = "Pocket 3 KVM Viewer — Linux HDMI capture for the GPD Pocket 3";
  inputs.nixpkgs.url = "github:NixOS/nixpkgs/753cc8a3a87467296ddd1fa93f0cc3e81120ee46";
  outputs = { self, nixpkgs }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" ];
      forSystems = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
    in {
      packages = forSystems (pkgs: rec {
        pocket3-kvm-viewer = pkgs.rustPlatform.buildRustPackage {
          pname = "pocket3-kvm-viewer";
          version = "0.1.0";
          src = self;
          cargoLock.lockFile = ./Cargo.lock;
          buildFeatures = [ "native-yuv" "background-throttle" ];

          nativeBuildInputs = with pkgs; [ pkg-config wrapGAppsHook4 ];
          buildInputs = with pkgs; [
            gtk4
            gst_all_1.gstreamer
            gst_all_1.gst-plugins-base
            gst_all_1.gst-plugins-good
          ];

          postInstall = ''
            install -Dm644 data/io.github.pocket3.KvmViewer.desktop \
              "$out/share/applications/io.github.pocket3.KvmViewer.desktop"
          '';
          preFixup = ''
            makeWrapperArgs+=(
              --prefix GST_PLUGIN_SYSTEM_PATH_1_0 :
              "${pkgs.lib.makeSearchPath "lib/gstreamer-1.0" [ pkgs.gst_all_1.gstreamer.out pkgs.gst_all_1.gst-plugins-base pkgs.gst_all_1.gst-plugins-good ]}"
            )
          '';

          meta = {
            description = "Viewer for the GPD Pocket 3 KVM HDMI capture module";
            mainProgram = "pocket3-kvm-viewer";
            platforms = [ "x86_64-linux" "aarch64-linux" ];
          };
        };
        default = pocket3-kvm-viewer;
      });
      apps = forSystems (pkgs: {
        default = {
          type = "app";
          program = "${self.packages.${pkgs.stdenv.hostPlatform.system}.default}/bin/pocket3-kvm-viewer";
          meta.description = "Launch Pocket 3 KVM Viewer";
        };
      });
      devShells = forSystems (pkgs: {
        default = pkgs.mkShell {
          nativeBuildInputs = with pkgs; [ cargo rustc rustfmt clippy pkg-config ];
          buildInputs = with pkgs; [
            gtk4
            gst_all_1.gstreamer
            gst_all_1.gst-plugins-base
            gst_all_1.gst-plugins-good
          ];
        };
      });
    };
}
