# The pre-built self-extracting release asset recorded in `binary-url.json`.
{
  lib,
  stdenv,
  fetchurl,
  bzip2,
  gnutar,
  name,
  program ? name,
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

  nativeBuildInputs = [
    bzip2
    gnutar
  ];

  installPhase = ''
    runHook preInstall

    mkdir -p "$out/libexec/${name}" "$out/bin"
    (
      cd "$out/libexec/${name}"
      sh "$src" --extract >/dev/null
    )

    app_path="$(
      sed -n 's|.* \(/nix/store/[^ ]*\)/bin/[^ ]*.*|\1|p' \
        "$out/libexec/${name}/dat/nix/store/"*-startup | head -n1
    )"
    [ -n "$app_path" ] || {
      echo "binary: could not find the bundled application path" >&2
      exit 1
    }

    if [ -d "$out/libexec/${name}/dat$app_path/share" ]; then
      mkdir -p "$out/share"
      cp -r "$out/libexec/${name}/dat$app_path/share/." "$out/share/"
      chmod -R u+w "$out/share"
    fi

    cat > "$out/bin/${program}" <<EOF
    #!${stdenv.shell}
    set -eu
    cd "$out/libexec/${name}/dat"
    . ../env
    exec ../run "\$@"
    EOF
    chmod 755 "$out/bin/${program}"

    ${postInstall}

    runHook postInstall
  '';

  meta = {
    description = "${info.repo} pre-built Linux binary (${info.tag})";
    homepage = "https://github.com/${info.repo}";
    sourceProvenance = [ lib.sourceTypes.binaryNativeCode ];
    mainProgram = program;
    platforms = [ system ];
  };
}
