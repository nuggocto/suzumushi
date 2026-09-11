{
  description = "A calm, fully local terminal audio player for Linux";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { nixpkgs, ... }:
    let
      system = "x86_64-linux";
      pkgs = nixpkgs.legacyPackages.${system};
      suzumushi = pkgs.callPackage ./packaging/nix/package.nix { };
    in
    {
      packages.${system} = {
        inherit suzumushi;
        default = suzumushi;
      };

      checks.${system}.suzumushi = suzumushi;
      formatter.${system} = pkgs.nixfmt;
    };
}
