# Static (musl) poppler-utils for the Linux pdftotext resource.
# Used by .github/workflows/release.yml:
#
#   nix build --impure --file scripts/pdftotext-static.nix --argstr rev <nixpkgs commit>
#
# The result's bin/pdftotext needs no shared libraries. See
# docs/Architecture_Build.md §8.
{ rev }:
let
  pkgs = (builtins.getFlake "github:NixOS/nixpkgs/${rev}").legacyPackages.x86_64-linux.pkgsStatic;
in
# minimal: no NSS (p11-kit can't be built static), cairo, curl, lcms,
# libtiff or glib; pdftotext uses none of them.
(pkgs.poppler-utils.override { minimal = true; }).overrideAttrs (old: {
  # Static archives don't carry their own dependencies, and poppler's
  # CMake links libfontconfig.a and libfreetype.a without theirs
  # (expat; libpng, bzip2, brotli, zlib). cc-wrapper puts NIX_LDFLAGS at
  # the end of every link line, after the archives that need them.
  buildInputs = (old.buildInputs or [ ]) ++ [
    pkgs.expat
    pkgs.libpng
    pkgs.bzip2
    pkgs.brotli
    pkgs.zlib
  ];
  NIX_LDFLAGS = "-lfontconfig -lexpat -lfreetype -lpng -lbz2 -lbrotlidec -lbrotlicommon -lz -lm";
  # The manual test programs aren't needed and are the first to hit link
  # problems.
  cmakeFlags = (old.cmakeFlags or [ ]) ++ [ "-DBUILD_MANUAL_TESTS=OFF" ];
  doCheck = false;
})
