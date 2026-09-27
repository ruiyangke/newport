{
  description = "Porthop development and agent CI tools";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-26.05";
    rust-overlay.url = "github:oxalica/rust-overlay";
    rust-overlay.inputs.nixpkgs.follows = "nixpkgs";
  };

  outputs = { nixpkgs, rust-overlay, ... }:
    let
      systems = [ "aarch64-darwin" "x86_64-darwin" "aarch64-linux" "x86_64-linux" ];
      forAllSystems = nixpkgs.lib.genAttrs systems;
    in {
      devShells = forAllSystems (system:
        let
          pkgs = import nixpkgs {
            inherit system;
            overlays = [ rust-overlay.overlays.default ];
          };
          rust = pkgs.rust-bin.stable."1.98.0".minimal.override {
            extensions = [ "clippy" "rustfmt" ];
          };
        in {
          default = pkgs.mkShell {
            packages = with pkgs; [
              rust nodejs_22 python3 pkg-config git openssh
              bashInteractive zsh fish coreutils perl
            ] ++ lib.optionals stdenv.hostPlatform.isLinux [ xclip wl-clipboard ];
            shellHook = pkgs.lib.optionalString pkgs.stdenv.hostPlatform.isLinux ''
              export PORTHOP_TEST_XCLIP="${pkgs.xclip}/bin/xclip"
            '';
          };
        });
    };
}
