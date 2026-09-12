# Nix packaging

The repository flake builds Suzumushi from source for `x86_64-linux`. It exposes
`packages.x86_64-linux.default` and `packages.x86_64-linux.suzumushi`, both with
the canonical `suzumushi` executable and a relative `suzu` symlink.

`flake.lock` pins Nixpkgs, including Rust and the native libraries. The package
uses `Cargo.lock` for Rust dependencies and reads the version from `Cargo.toml`.
Adding or updating this packaging does not require an application version bump.
Version 1.1.4 is the first stable release containing the flake. Use its tag for
the stable package, or `shrek` to follow development.

## Install or run

With flakes enabled, install into your user profile:

```sh
nix profile add github:nuggocto/suzumushi/v1.1.4
```

To enable flakes for any command here, put
`--extra-experimental-features 'nix-command flakes'` immediately after `nix`.

You can also run without adding anything to your profile:

```sh
nix run github:nuggocto/suzumushi/v1.1.4 -- init ./suzumushi
nix run github:nuggocto/suzumushi/v1.1.4 -- --root ./suzumushi
```

Add your audio below `./suzumushi/audio/library/` before starting playback.
Nix provides the client libraries; the player connects to your existing audio
and desktop D-Bus sessions.

## Declarative installation

Add an input to your existing NixOS or Home Manager flake:

```nix
inputs.suzumushi.url = "github:nuggocto/suzumushi/v1.1.4";
```

Make `inputs` available to your NixOS modules by passing
`specialArgs = { inherit inputs; };` to your existing `nixpkgs.lib.nixosSystem`
call. Then add the package in a module:

```nix
{ inputs, pkgs, ... }:
{
  environment.systemPackages = [
    inputs.suzumushi.packages.${pkgs.stdenv.hostPlatform.system}.default
  ];
}
```

For standalone Home Manager, pass `extraSpecialArgs = { inherit inputs; };`
to `homeManagerConfiguration` and use `home.packages` in place of
`environment.systemPackages`. When Home Manager is a NixOS module, pass the
inputs through `home-manager.extraSpecialArgs` instead.

Keep Suzumushi's own Nixpkgs input unless you intend to test it against a
different toolchain. Its pinned compiler must satisfy the Rust version in
`Cargo.toml`.

## Check a checkout

From a checkout containing the flake:

```sh
nix fmt -- --check flake.nix packaging/nix/package.nix
nix flake check --no-update-lock-file --print-build-logs
nix build --no-update-lock-file
./result/bin/suzu --version
```

Nix checks the release build and the existing Rust test suite, then exercises
the installed commands: version, the `suzu` symlink, library initialization, and
metadata scanning of the MP3 fixture. Interactive playback and desktop media
controls also need testing with a running audio and D-Bus session.

New files must be added to Git before Nix will include them in a Git checkout.
The build recipe includes only the manifest, lockfile, source, tests, and
license, so local libraries and build output stay out of the package.

To update the pinned Nix dependencies, run `nix flake update nixpkgs`, rerun the
checks, and commit the generated lockfile. A Nix job in the normal CI workflow
checks formatting, builds and tests the package, and exercises `nix run`.
