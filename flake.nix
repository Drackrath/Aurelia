{
  description = "Aurelia — a CLI Steam launcher";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs =
    { self, nixpkgs, flake-utils }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = nixpkgs.legacyPackages.${system};
        inherit (pkgs) lib;
        manifest = (lib.importTOML ./Cargo.toml).package;

        # `target/` is a local build artefact, not input: including it would make
        # the source hash change on every cargo run and copy gigabytes into the
        # store. The vendored crates under `vendor/` *are* path dependencies and
        # must stay.
        src = lib.cleanSourceWith {
          name = "aurelia-source";
          src = ./.;
          filter =
            path: type:
            let
              base = baseNameOf path;
            in
            !(type == "directory" && (base == "target" || base == ".git"));
        };
      in
      {
        packages.default = self.packages.${system}.aurelia;

        packages.aurelia = pkgs.rustPlatform.buildRustPackage {
          pname = manifest.name;
          inherit (manifest) version;
          inherit src;

          cargoLock = {
            lockFile = ./Cargo.lock;
            # steam-vent is a git dependency pinned to a tag. Fetching it by the
            # revision already recorded in Cargo.lock avoids carrying a second
            # hash that has to be updated by hand on every bump.
            allowBuiltinFetchGit = true;
          };

          # cmake and bindgen are for aws-lc-sys, the crypto backend rustls pulls
          # in. No OpenSSL: reqwest is configured with rustls, so nothing in the
          # tree links against it.
          nativeBuildInputs = [
            pkgs.cmake
            pkgs.perl
            pkgs.pkg-config
            pkgs.rustPlatform.bindgenHook
          ];

          # The compression -sys crates (bzip2, lzma, zstd) link these when a
          # system copy is present. Protobuf codegen is pure Rust, so protoc is
          # deliberately absent.
          buildInputs = [
            pkgs.bzip2
            pkgs.xz
            pkgs.zstd
          ];

          # Matches the cargo matrix jobs, which also only build. The suite reads
          # the real Steam directories under $HOME, which the sandbox does not have.
          doCheck = false;

          meta = {
            inherit (manifest) description;
            homepage = "https://github.com/Drackrath/Aurelia";
            license = lib.licenses.gpl3Only;
            mainProgram = "aurelia";
            platforms = lib.platforms.unix;
          };
        };

        devShells.default = pkgs.mkShell {
          inputsFrom = [ self.packages.${system}.aurelia ];
          packages = [
            pkgs.cargo-deb
            pkgs.clippy
            pkgs.rustfmt
          ];
        };
      }
    );
}
