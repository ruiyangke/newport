{
  description = "Newport development and CI tools";

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
          rust = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;
          development = pkgs.mkShell {
            packages = with pkgs; [
              rust nodejs_22 pkg-config git openssh openssl
              bashInteractive zsh fish coreutils perl actionlint shellcheck
            ] ++ lib.optionals stdenv.hostPlatform.isLinux [ xclip wl-clipboard ];
            shellHook = pkgs.lib.optionalString pkgs.stdenv.hostPlatform.isLinux ''
              export NEWPORT_TEST_XCLIP="${pkgs.xclip}/bin/xclip"
            '';
          };
        in {
          default = development;
        } // pkgs.lib.optionalAttrs pkgs.stdenv.hostPlatform.isDarwin {
          remote = pkgs.mkShell {
            inputsFrom = [ development ];
            packages = with pkgs; [ colima docker-client ];
          };
        });
    };
}
