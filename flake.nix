{
  description = "wtui monitoring tools";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    crane.url = "github:ipetkov/crane";
  };

  outputs = { self, nixpkgs, flake-utils, crane }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs { inherit system; };
        craneLib = crane.lib.${system};
        src = craneLib.cleanCargoSource (craneLib.path ./.);
        commonArgs = {
          inherit src;
          pname = "wtui";
          version = "0.1.0";
          doCheck = true;
          buildInputs = [ pkgs.pkg-config pkgs.sqlite ];
        };
        cargoArtifacts = craneLib.buildDepsOnly commonArgs;
      in {
        packages.wtui = craneLib.buildPackage (commonArgs // {
          inherit cargoArtifacts;
          cargoExtraArgs = "-p wtui";
        });

        packages.wtui-daemon = craneLib.buildPackage (commonArgs // {
          inherit cargoArtifacts;
          pname = "wtui-daemon";
          cargoExtraArgs = "-p wtui-daemon";
        });

        packages.default = self.packages.${system}.wtui;

        devShells.default = pkgs.mkShell {
          inputsFrom = [ cargoArtifacts ];
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
