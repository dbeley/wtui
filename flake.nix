{
  description = "wtui monitoring tools";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs { inherit system; };
        rustPkg = { pname, cargoFlags ? [] }: pkgs.rustPlatform.buildRustPackage {
          inherit pname;
          version = "0.1.0";
          src = ./.;
          cargoLock = {
            lockFile = ./Cargo.lock;
          };
          cargoBuildFlags = cargoFlags;
          buildInputs = [ pkgs.pkg-config pkgs.sqlite ];
        };
      in {
        packages.wtui = rustPkg { pname = "wtui"; cargoFlags = [ "-p" "wtui" ]; };
        packages.wtui-daemon = rustPkg { pname = "wtui-daemon"; cargoFlags = [ "-p" "wtui-daemon" ]; };
        packages.default = self.packages.${system}.wtui;

        devShells.default = pkgs.mkShell {
          buildInputs = [
            pkgs.rustup
            pkgs.pkg-config
            pkgs.sqlite
            pkgs.openssl
            pkgs.cargo
            pkgs.clippy
            pkgs.rustfmt
          ];
        };
      });
}
