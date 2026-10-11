# Airlift as a Nix package.
#
# The binary itself has no runtime dependencies. HEIC and video thumbnails
# come from external tools found on PATH (see docs/usage.md), so by default
# the wrapper puts ImageMagick and ffmpeg on the PATH of the installed
# binary. Pass `withThumbnailTools = false` for a tools-free build.
{
  lib,
  rustPlatform,
  makeWrapper,
  imagemagick,
  ffmpeg-headless,
  withThumbnailTools ? true,
}:
let
  manifest = (lib.importTOML ../Cargo.toml).package;
in
rustPlatform.buildRustPackage {
  pname = manifest.name;
  version = manifest.version;

  # Only what the build needs; docs and notes do not invalidate the cache.
  src = lib.fileset.toSource {
    root = ../.;
    fileset = lib.fileset.unions [
      ../Cargo.toml
      ../Cargo.lock
      ../src
      ../web
      ../tests
    ];
  };

  # The lock file is the source of truth for dependency hashes, so there is
  # no cargoHash to update on every `cargo update`.
  cargoLock.lockFile = ../Cargo.lock;

  nativeBuildInputs = [ makeWrapper ];

  postInstall = lib.optionalString withThumbnailTools ''
    wrapProgram $out/bin/airlift \
      --prefix PATH : ${lib.makeBinPath [ imagemagick ffmpeg-headless ]}
  '';

  meta = {
    description = manifest.description;
    mainProgram = "airlift";
    platforms = lib.platforms.unix;
  };
}
