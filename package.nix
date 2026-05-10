{ pkgs, debug ? false }: pkgs.callPackage
  ({ pkg-config
   , wayland
   , libxkbcommon
   , libglvnd
   , wayland-scanner
   , makeWrapper
   }:
  let
    naersk = pkgs.callPackage
      (fetchTarball "https://github.com/nix-community/naersk/archive/378614f37a6bee5a3f2ef4f825a73d948d3ae921.zip")
      { };
  in
  naersk.buildPackage {
    src = ./source;
    release = !debug;
    nativeBuildInputs = [ pkg-config wayland-scanner makeWrapper ];
    buildInputs = [ wayland libxkbcommon libglvnd ];
    RUSTFLAGS = toString (map (arg: "-C link-arg=" + arg) [
      "-Wl,--push-state,--no-as-needed"
      "-lEGL"
      "-lwayland-client"
      "-Wl,--pop-state"
    ]);
    postInstall = ''
      wrapProgram $out/bin/mononocle \
        --prefix LD_LIBRARY_PATH : ${pkgs.lib.makeLibraryPath [ wayland libxkbcommon libglvnd ]}
    '';
  })
{ }
