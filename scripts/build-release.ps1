$ErrorActionPreference = 'Stop'
$root = Resolve-Path (Join-Path $PSScriptRoot '..')
$desktop = Join-Path $root 'apps/desktop/src-tauri'
$binaries = Join-Path $desktop 'binaries'

cargo build --manifest-path (Join-Path $root 'core/Cargo.toml') -p anyfindd --release
New-Item -ItemType Directory -Force $binaries | Out-Null
Copy-Item (Join-Path $root 'core/target/release/anyfindd.exe') `
  (Join-Path $binaries 'anyfindd-x86_64-pc-windows-msvc.exe') -Force

Push-Location $desktop
try {
  cargo tauri build
} finally {
  Pop-Location
}
