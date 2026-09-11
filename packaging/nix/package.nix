{
  lib,
  stdenv,
  rustPlatform,
  pkg-config,
  alsa-lib,
  pipewire,
  dbus,
}:
let
  manifest = (builtins.fromTOML (builtins.readFile ../../Cargo.toml)).package;
in
rustPlatform.buildRustPackage {
  pname = manifest.name;
  inherit (manifest) version;

  src = lib.fileset.toSource {
    root = ../..;
    fileset = lib.fileset.unions [
      ../../Cargo.toml
      ../../Cargo.lock
      ../../src
      ../../tests
      ../../LICENSE
    ];
  };

  cargoLock.lockFile = ../../Cargo.lock;

  nativeBuildInputs = [
    pkg-config
    rustPlatform.bindgenHook
  ];
  buildInputs = [
    alsa-lib
    pipewire
    dbus
  ];

  # Helper lifecycle tests need a shell inside the Nix build sandbox.
  postPatch = ''
    substituteInPlace src/metadata.rs src/audio/decoder.rs \
      --replace-fail '"/bin/sh"' '"${stdenv.shell}"'
  '';

  postInstall = ''
    ln -s suzumushi "$out/bin/suzu"
    install -Dm644 LICENSE "$out/share/licenses/suzumushi/LICENSE"
  '';

  doInstallCheck = true;
  installCheckPhase = ''
    runHook preInstallCheck

    test "$(readlink "$out/bin/suzu")" = suzumushi
    "$out/bin/suzu" --version | grep -Fx 'suzumushi ${manifest.version}'
    test_root="$TMPDIR/suzumushi-install-check"
    "$out/bin/suzumushi" init "$test_root"
    cp tests/fixtures/audio/tone.mp3 "$test_root/audio/library/"
    "$out/bin/suzu" diagnose --root "$test_root" > diagnosis.txt
    grep -Fx 'tracks: 1' diagnosis.txt
    grep -Fx 'warnings: 0' diagnosis.txt

    runHook postInstallCheck
  '';

  meta = {
    inherit (manifest) description;
    homepage = manifest.repository;
    license = lib.licenses.asl20;
    mainProgram = "suzumushi";
    platforms = [ "x86_64-linux" ];
  };
}
