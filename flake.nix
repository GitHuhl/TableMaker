{
  description = "TableMaker - wavetables from audio clips, cut at oversampled zero crossings";

  # Pinned to the NixOS stable channel. The channel tarball is used instead of
  # github:NixOS/nixpkgs so the flake also resolves behind proxies that block
  # GitHub archive downloads.
  inputs.nixpkgs.url = "https://channels.nixos.org/nixos-26.05/nixexprs.tar.xz";

  outputs = { self, nixpkgs }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" ];
      forAll = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});

      # The one version number: the workspace's, in rust/Cargo.toml.
      version = (builtins.fromTOML (builtins.readFile ./rust/Cargo.toml)).workspace.package.version;

      tablemaker = pkgs: pkgs.rustPlatform.buildRustPackage {
        pname = "tablemaker";
        inherit version;
        src = self;
        cargoRoot = "rust";
        buildAndTestSubdir = "rust";
        cargoLock.lockFile = ./rust/Cargo.lock;
        cargoBuildFlags = [ "-p" "tablemaker-app" ];
        # Every crate's tests, the end-to-end CLI test included.
        cargoTestFlags = [ "--workspace" ];
        meta.description = "Build wavetables from audio clips by oversampled zero-crossing cycle extraction";
        meta.mainProgram = "tablemaker";
        meta.license = pkgs.lib.licenses.agpl3Plus;
      };
    in {
      packages = forAll (pkgs: { default = tablemaker pkgs; });

      devShells = forAll (pkgs: {
        default = pkgs.mkShell {
          nativeBuildInputs = with pkgs; [ cargo rustc clippy rustfmt ];
        };
      });

      apps = forAll (pkgs: {
        default = { type = "app"; program = "${self.packages.${pkgs.stdenv.hostPlatform.system}.default}/bin/tablemaker"; };
      });

      checks = forAll (pkgs: { default = self.packages.${pkgs.stdenv.hostPlatform.system}.default; });
    };
}
