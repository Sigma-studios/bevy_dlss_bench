# nix-shell for building/running bevy_dlss_bench on NixOS (NVIDIA driver, Vulkan).
{ pkgs ? import <nixpkgs> { } }:

let
  runtimeLibs = with pkgs; [
    vulkan-loader
    libxkbcommon
    wayland
    xorg.libX11
    xorg.libXcursor
    xorg.libXi
    xorg.libXrandr
    alsa-lib
    udev
    stdenv.cc.cc.lib # libstdc++ for the NGX static lib / DLSS runtime
  ];
in
pkgs.mkShell {
  nativeBuildInputs = with pkgs; [
    pkg-config
    just
    git
    clang
  ];
  buildInputs = with pkgs; [
    vulkan-headers
    vulkan-loader
    vulkan-tools # vulkaninfo, handy to confirm the NVIDIA ICD is picked
    alsa-lib
    udev
    libxkbcommon
    wayland
  ];

  # bindgen (dlss_wgpu build script) needs libclang.
  LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";
  # dlss_wgpu reads $VULKAN_SDK/include; vulkan-headers has exactly that layout.
  VULKAN_SDK = "${pkgs.vulkan-headers}";

  shellHook = ''
    # NixOS exposes the NVIDIA driver (libvulkan ICD, libnvidia-ngx / NGX core) here.
    export LD_LIBRARY_PATH="/run/opengl-driver/lib:${pkgs.lib.makeLibraryPath runtimeLibs}''${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
    # Pinned DLSS SDK into vendor/ (no-op once present). DLSS_SDK is set by .cargo/config.toml.
    ./scripts/fetch-dlss-sdk.sh || echo "warning: DLSS SDK fetch failed; build with --no-default-features" >&2
  '';
}
