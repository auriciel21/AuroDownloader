# Installateur Windows (NSIS, par utilisateur, non signé ; SPEC 4.6).
#
# Construit `aurod` et `auro` (espace de travail du moteur), les place dans `binaries/`
# sous le nom que Tauri attend (`<nom>-<triplet>.exe`), puis l'application et son
# installateur. `tauri.bundle.json` ajoute ces deux binaires à la configuration : ils ne
# sont pas dans `tauri.conf.json`, sinon `cargo build` et `cargo test` exigeraient
# qu'ils existent. Demande `cargo install tauri-cli --version "^2" --locked`.
#
# Résultat : target\release\bundle\nsis\AuroDownloader_<version>_x64-setup.exe
$ErrorActionPreference = 'Stop'
$root = Resolve-Path "$PSScriptRoot\..\.."

function Run([string]$what, [scriptblock]$cmd) {
    & $cmd
    if ($LASTEXITCODE -ne 0) { throw "$what : échec (code $LASTEXITCODE)" }
}

$triple = ((rustc -vV) -match '^host: ') -replace '^host: ', ''
Run 'moteur et CLI' { cargo build --release --manifest-path "$root\Cargo.toml" -p auro-daemon -p auro-cli }

$bin = New-Item -ItemType Directory -Force "$PSScriptRoot\binaries"
foreach ($name in 'aurod', 'auro') {
    Copy-Item "$root\target\release\$name.exe" "$bin\$name-$triple.exe" -Force
}

Push-Location $PSScriptRoot
try {
    Run 'installateur' { cargo tauri build --bundles nsis --config tauri.bundle.json }
} finally {
    Pop-Location
}
