{
  description = "Airlift: temporary, PIN-protected LAN file sharing from the terminal to any browser";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { self, nixpkgs }:
    let
      # No x86_64-darwin: nixpkgs unstable dropped Intel Macs in 26.11.
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "aarch64-darwin"
      ];
      forAll = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
    in
    {
      overlays.default = final: prev: {
        airlift = final.callPackage ./nix/package.nix { };
      };

      packages = forAll (pkgs: rec {
        airlift = pkgs.callPackage ./nix/package.nix { };
        # Same binary without ImageMagick/ffmpeg on its PATH: smaller
        # closure, HEIC and video thumbnails only if the system has tools.
        airlift-minimal = airlift.override { withThumbnailTools = false; };
        default = airlift;
      });

      apps = forAll (pkgs: {
        default = {
          type = "app";
          program = "${self.packages.${pkgs.stdenv.hostPlatform.system}.default}/bin/airlift";
          meta.description = "Share files on the LAN: nix run .# -- send movie.mp4";
        };
      });

      devShells = forAll (pkgs: {
        default = pkgs.mkShell {
          packages = with pkgs; [
            cargo
            rustc
            clippy
            rustfmt
            rust-analyzer
            imagemagick
            ffmpeg-headless
          ];
        };
      });
    };
}
