{
  description = "Napstr Tauri development shell";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs = { self, nixpkgs }:
    let
      system = "x86_64-linux";
      pkgs = import nixpkgs { inherit system; };
      gstreamerPlugins = with pkgs.gst_all_1; [
        gst-plugins-base
        gst-plugins-good
        gst-plugins-bad
        gst-libav
      ];
    in {
      devShells.${system}.default = pkgs.mkShell {
        packages = with pkgs; [
          nodejs_22
          rustc
          cargo
          rustfmt
          appimage-run
          alsa-lib
          curl
          file
          tor
          zenity
          pkg-config
          dbus
          gtk3
          webkitgtk_4_1
          gst_all_1.gstreamer
          wayland
          librsvg
          patchelf
        ] ++ gstreamerPlugins;
        # Unwrapped Tauri dev binaries need WebKit's media plugins at runtime.
        GST_PLUGIN_PATH_1_0 = pkgs.lib.makeSearchPath "lib/gstreamer-1.0" gstreamerPlugins;
        nativeBuildInputs = with pkgs; [
          alsa-lib.dev
          dbus.dev
          gtk3.dev
          webkitgtk_4_1.dev
          wayland.dev
        ];
      };
    };
}
