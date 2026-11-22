{
  description = "Aurelia — a CLI Steam launcher";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    # nixpkgs' Rust lags; pin via rust-toolchain.toml.
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      flake-utils,
      rust-overlay,
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ (import rust-overlay) ];
        };
        inherit (pkgs) lib;
        # Same toolchain as rustup and CI.
        toolchain = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;
        rustPlatform = pkgs.makeRustPlatform {
          cargo = toolchain;
          rustc = toolchain;
        };
        manifest = (lib.importTOML ./Cargo.toml).package;

        # `target/` is a local build artefact, not input: including it would make
        # the source hash change on every cargo run and copy gigabytes into the
        # store.
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

        packages.aurelia = rustPlatform.buildRustPackage {
          pname = "aurelia";
          inherit (manifest) version;
          inherit src;

          cargoLock = {
            lockFile = ./Cargo.lock;
          };

          # cmake and bindgen are for aws-lc-sys, the crypto backend rustls pulls
          # in. No OpenSSL: reqwest is configured with rustls, so nothing in the
          # tree links against it.
          nativeBuildInputs = [
            pkgs.cmake
            pkgs.perl
            pkgs.pkg-config
            rustPlatform.bindgenHook
          ];

          # The compression -sys crates (bzip2, lzma, zstd) link these when a
          # system copy is present. Protobuf codegen is pure Rust, so protoc is
          # deliberately absent.
          buildInputs = [
            pkgs.bzip2
            pkgs.dbus
            pkgs.xz
            pkgs.zstd
          ];

          # nixpkgs sets AWS_LC_SYS_USE_SYSTEM=1; build bundled.
          preBuild = ''
            export AWS_LC_SYS_USE_SYSTEM=0
          '';

          # Matches the cargo matrix jobs, which also only build. The suite reads
          # the real Steam directories under $HOME, which the sandbox does not have.
          doCheck = false;

          meta = {
            inherit (manifest) description;
            homepage = "https://github.com/Drackrath/Aurelia";
            license = lib.licenses.gpl3Plus;
            mainProgram = "aurelia";
            platforms = lib.platforms.unix;
          };
        };

        devShells.default = pkgs.mkShell {
          inputsFrom = [ self.packages.${system}.aurelia ];
          # Toolchain ships clippy and rustfmt.
          packages = [
            pkgs.cargo-deb
            toolchain
          ];
        };
      }
    );
}
