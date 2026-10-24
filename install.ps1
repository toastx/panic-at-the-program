# Installs the latest panic release: irm https://raw.githubusercontent.com/toastx/panic-at-the-program/master/install.ps1 | iex
$ErrorActionPreference = "Stop"

$repo = "toastx/panic-at-the-program"
$dir = if ($env:PANIC_INSTALL_DIR) { $env:PANIC_INSTALL_DIR } else { "$env:USERPROFILE\.panic\bin" }

$tag = (Invoke-RestMethod "https://api.github.com/repos/$repo/releases/latest").tag_name
$asset = "panic-$($tag.TrimStart('v'))-x86_64-pc-windows-msvc.zip"
$url = "https://github.com/$repo/releases/download/$tag/$asset"

$tmp = Join-Path ([IO.Path]::GetTempPath()) ([Guid]::NewGuid())
New-Item -ItemType Directory $tmp | Out-Null
try {
    $zip = Join-Path $tmp $asset
    Invoke-WebRequest $url -OutFile $zip -UseBasicParsing
    Invoke-WebRequest "$url.sha256" -OutFile "$zip.sha256" -UseBasicParsing
    $expected = ((Get-Content "$zip.sha256" -Raw) -split '\s+')[0]
    if ((Get-FileHash $zip -Algorithm SHA256).Hash -ne $expected) { throw "checksum mismatch for $asset" }
    New-Item -ItemType Directory -Force $dir | Out-Null
    Expand-Archive $zip -DestinationPath $dir -Force
} finally {
    Remove-Item -Recurse -Force $tmp
}

$userPath = [Environment]::GetEnvironmentVariable("Path", "User")
if (($userPath -split ';') -notcontains $dir) {
    [Environment]::SetEnvironmentVariable("Path", "$userPath;$dir", "User")
    Write-Host "added $dir to your user PATH (restart your terminal)"
}
Write-Host "installed panic $tag to $dir\panic.exe"
