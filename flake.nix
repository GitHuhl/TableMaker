{
  description = "TableMaker - turn recordings of a voice into a playable instrument";

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

      # What the app (winit + wgpu) loads at runtime.
      guiLibs = pkgs: with pkgs; [ libxkbcommon vulkan-loader wayland libx11 libxcursor libxrandr libxi ];

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
        nativeBuildInputs = [ pkgs.pkg-config pkgs.makeWrapper ];
        # ALSA for audio out and MIDI in (PipeWire serves it through its ALSA layer).
        buildInputs = [ pkgs.alsa-lib ];
        postInstall = ''
          wrapProgram $out/bin/tablemaker \
            --prefix LD_LIBRARY_PATH : ${pkgs.lib.makeLibraryPath (guiLibs pkgs)}
        '';
        meta.description = "Turn recordings of a voice into a playable instrument";
        meta.mainProgram = "tablemaker";
        meta.license = pkgs.lib.licenses.agpl3Plus;
      };
    in {
      packages = forAll (pkgs: { default = tablemaker pkgs; });

      devShells = forAll (pkgs: {
        default = pkgs.mkShell {
          nativeBuildInputs = with pkgs; [ cargo rustc clippy rustfmt pkg-config ];
          buildInputs = with pkgs; [ alsa-lib ];
          # xvfb-run + mesa: headless screenshots of the app.
          packages = with pkgs; [ xvfb-run mesa ] ++ guiLibs pkgs;
          shellHook = ''
            # The app (cargo run -p tablemaker-app) dlopens these.
            export LD_LIBRARY_PATH=${pkgs.lib.makeLibraryPath (guiLibs pkgs)}''${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}
            # Software Vulkan (Mesa lavapipe) for headless runs.
            export TABLEMAKER_LAVAPIPE_ICD=${pkgs.mesa}/share/vulkan/icd.d/lvp_icd.x86_64.json
          '';
        };
      });

      apps = forAll (pkgs: {
        default = { type = "app"; program = "${self.packages.${pkgs.stdenv.hostPlatform.system}.default}/bin/tablemaker"; };
      });

      checks = forAll (pkgs: { default = self.packages.${pkgs.stdenv.hostPlatform.system}.default; });
    };
}
