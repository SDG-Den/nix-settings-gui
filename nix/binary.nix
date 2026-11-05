# The pre-built self-extracting release asset recorded in `binary-url.json`.
{
  lib,
  stdenv,
  fetchurl,
  name,
  file,
  info,
  postInstall ? "",
}:

let
  system = stdenv.hostPlatform.system;
in
assert lib.assertMsg (
  info.system == system
) "${name}: binary-url.json records a ${info.system} binary, not ${system}";
stdenv.mkDerivation {
  pname = name;
  inherit (info) version;

  src = fetchurl {
    inherit (info) url hash;
    name = file;
  };

  dontUnpack = true;
  dontFixup = true;

  installPhase = ''
    runHook preInstall
    install -Dm755 $src $out/bin/${name}
    ${postInstall}
    runHook postInstall
  '';

  meta = {
    description = "${info.repo} pre-built Linux binary (${info.tag})";
    homepage = "https://github.com/${info.repo}";
    sourceProvenance = [ lib.sourceTypes.binaryNativeCode ];
    mainProgram = name;
    platforms = [ system ];
  };
}
