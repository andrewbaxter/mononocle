{ pkgs, debug ? false }: pkgs.callPackage
  ({ pkg-config
   , wayland
   , libxkbcommon
   , mesa
   , wayland-scanner
   }:
  let
    naersk = pkgs.callPackage
      (fetchTarball "https://github.com/nix-community/naersk/archive/378614f37a6bee5a3f2ef4f825a73d948d3ae921.zip")
      { };
  in
  naersk.buildPackage {
    src = ./source;
    release = !debug;
    nativeBuildInputs = [ pkg-config wayland-scanner ];
    buildInputs = [ wayland libxkbcommon mesa ];
  })
{ }
