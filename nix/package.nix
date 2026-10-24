# Ond for Linux. Written as a plain nixpkgs-style package (`callPackage`), so
# the flake, an overlay, or nixpkgs itself can all build it the same way.
{
  lib,
  rustPlatform,
  pkg-config,
  alsa-lib,
  pipewire,
  libxkbcommon,
  wayland,
  vulkan-loader,
  libGL,
  libX11,
  libXcursor,
  libXi,
  libXrandr,
  libxcb,
}:

let
  cargoToml = lib.importTOML ../Cargo.toml;

  # Opened at run time (dlopen) rather than linked: the windowing system,
  # keyboard handling and the GPU. Without them on the RPATH the app finds no
  # display on NixOS.
  runtimeLibs = [
    libxkbcommon
    wayland
    vulkan-loader
    libGL
    libX11
    libXcursor
    libXi
    libXrandr
    libxcb
  ];
in
rustPlatform.buildRustPackage {
  pname = "ond";
  inherit (cargoToml.workspace.package) version;

  src = lib.fileset.toSource {
    root = ../.;
    fileset = lib.fileset.unions [
      ../Cargo.toml
      ../Cargo.lock
      ../crates
      ../packaging
    ];
  };

  cargoLock.lockFile = ../Cargo.lock;
  cargoBuildFlags = [
    "-p"
    "ond-player"
  ];

  nativeBuildInputs = [
    pkg-config
    # PipeWire's bindings are generated with bindgen, which needs libclang.
    rustPlatform.bindgenHook
  ];
  buildInputs = [
    alsa-lib
    pipewire
  ];

  # CI runs the tests on every push; several render the UI offscreen, which
  # needs a GPU the build sandbox doesn't have.
  doCheck = false;

  postInstall = ''
    mv $out/bin/Ond $out/bin/ond
    install -Dm644 packaging/linux/dev.solidsilver.ond.desktop -t $out/share/applications
    install -Dm644 packaging/linux/dev.solidsilver.ond.metainfo.xml -t $out/share/metainfo
    mkdir -p $out/share/icons
    cp -r packaging/linux/hicolor $out/share/icons/
  '';

  postFixup = ''
    patchelf --add-rpath ${lib.makeLibraryPath runtimeLibs} $out/bin/ond
  '';

  passthru = { inherit runtimeLibs; };

  meta = {
    description = "Native music player for Navidrome and other OpenSubsonic servers";
    homepage = "https://github.com/Solidsilver/ond";
    license = with lib.licenses; [
      mit
      asl20
    ];
    mainProgram = "ond";
    platforms = lib.platforms.linux;
  };
}
