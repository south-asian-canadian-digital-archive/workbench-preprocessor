#!/bin/sh
# Install (or update) organise, the SACDA ingest tool, on macOS or Linux.
#
#   curl -fsSL https://github.com/south-asian-canadian-digital-archive/workbench-preprocessor/releases/latest/download/install.sh | sh
#
# Picks the build for this machine from GitHub releases, checks its SHA-256,
# puts `organise` in ~/.local/bin (and on PATH), remembers where the g/ configs
# folder is, and finishes with `organise doctor`. Safe to re-run.
#
# Options (env vars): ORGANISE_VERSION=v0.2.0 pins a release;
# ORGANISE_BIN_DIR=/some/dir changes the install folder; SACDA_G=/path/to/g
# skips the g/ search.
set -eu

REPO="south-asian-canadian-digital-archive/workbench-preprocessor"
BIN_DIR="${ORGANISE_BIN_DIR:-$HOME/.local/bin}"
VERSION="${ORGANISE_VERSION:-latest}"

say() { printf '  %s\n' "$*"; }
die() { printf '  ! %s\n' "$*" >&2; exit 1; }

need() { command -v "$1" >/dev/null 2>&1 || die "this needs '$1' - install it and re-run"; }
need curl
need tar
if command -v sha256sum >/dev/null 2>&1; then SHA="sha256sum"; elif command -v shasum >/dev/null 2>&1; then SHA="shasum -a 256"; else die "need sha256sum or shasum"; fi

# --- which build -----------------------------------------------------------
os=$(uname -s)
arch=$(uname -m)
case "$os" in
  Darwin) plat="darwin" ;;
  Linux) plat="linux" ;;
  *) die "unsupported OS '$os' - on Windows use install.ps1" ;;
esac
case "$arch" in
  arm64|aarch64) arch="aarch64" ;;
  x86_64|amd64) arch="x86_64" ;;
  *) die "unsupported CPU '$arch'" ;;
esac
if [ "$plat" = "linux" ] && [ "$arch" = "aarch64" ]; then
  die "there's no Linux ARM build yet - build from source with cargo"
fi
asset="workbench-preprocessor-$plat-$arch.tar.gz"

if [ "$VERSION" = "latest" ]; then
  base="https://github.com/$REPO/releases/latest/download"
else
  base="https://github.com/$REPO/releases/download/$VERSION"
fi

printf '\nInstalling organise (%s, %s/%s)\n' "$VERSION" "$plat" "$arch"

# --- download + verify ------------------------------------------------------
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
curl -fsSL "$base/$asset" -o "$tmp/$asset" || die "couldn't download $asset ($base) - check the internet connection"
if curl -fsSL "$base/SHASUMS256.txt" -o "$tmp/SHASUMS256.txt"; then
  want=$(grep " $asset\$" "$tmp/SHASUMS256.txt" | awk '{print $1}')
  got=$($SHA "$tmp/$asset" | awk '{print $1}')
  [ -n "$want" ] || die "SHASUMS256.txt doesn't list $asset"
  [ "$want" = "$got" ] || die "checksum mismatch for $asset - download corrupted? re-run"
  say "checksum ok"
else
  say "no SHASUMS256.txt on this release - skipping the checksum"
fi

mkdir -p "$tmp/x" "$BIN_DIR"
tar -xzf "$tmp/$asset" -C "$tmp/x"
[ -f "$tmp/x/organise" ] || die "$asset has no organise binary"
install -m 0755 "$tmp/x/organise" "$BIN_DIR/organise"
# macOS quarantines downloaded binaries; this one came from curl, but be sure.
if [ "$plat" = "darwin" ]; then xattr -d com.apple.quarantine "$BIN_DIR/organise" 2>/dev/null || true; fi
say "installed $("$BIN_DIR/organise" --version 2>/dev/null || echo organise) -> $BIN_DIR/organise"

# --- PATH -------------------------------------------------------------------
case ":$PATH:" in
  *":$BIN_DIR:"*) ;;
  *)
    shell_rc=""
    case "${SHELL:-}" in
      */zsh) shell_rc="$HOME/.zshrc" ;;
      */bash) shell_rc="$HOME/.bashrc"; [ "$plat" = "darwin" ] && shell_rc="$HOME/.bash_profile" ;;
      */fish) shell_rc="" ;;
    esac
    line="export PATH=\"$BIN_DIR:\$PATH\""
    if [ -n "$shell_rc" ] && ! grep -qs "$BIN_DIR" "$shell_rc"; then
      printf '\n# added by the organise installer\n%s\n' "$line" >> "$shell_rc"
      say "added $BIN_DIR to PATH in $shell_rc (open a new terminal to pick it up)"
    elif [ -z "$shell_rc" ]; then
      say "add $BIN_DIR to your PATH to run organise from anywhere"
    fi
    ;;
esac

# --- where's g/? ---------------------------------------------------------------
is_g() { [ -f "$1/config/sacda.toml" ]; }
g="${SACDA_G:-}"
if [ -z "$g" ] || ! is_g "$g"; then
  g=""
  for c in "$PWD" "$PWD/g" "$HOME/Desktop/islandora_workbench/g" "$HOME/Documents/GitHub/islandora_workbench/g" \
           "$HOME/islandora_workbench/g" "$HOME/Projects/sasi/islandora_workbench/g"; do
    if is_g "$c"; then g=$(cd "$c" && pwd); break; fi
  done
fi
if [ -z "$g" ] && [ -r /dev/tty ]; then
  printf '  Where is the workbench g/ folder (has config/sacda.toml)? Leave empty to skip: '
  read -r typed </dev/tty || typed=""
  if [ -n "$typed" ] && is_g "$typed"; then g=$(cd "$typed" && pwd); fi
fi
cfg_dir="${XDG_CONFIG_HOME:-$HOME/.config}/sacda"
if [ -n "$g" ]; then
  mkdir -p "$cfg_dir"
  printf '# written by the organise installer\ng_root = "%s"\n' "$g" > "$cfg_dir/organise.toml"
  say "configs: $g (saved in $cfg_dir/organise.toml)"
else
  say "no g/ folder found - organise will look for one from wherever you run it"
fi

printf '\n'
"$BIN_DIR/organise" doctor || true
printf '\nDone. Run `organise` in the folder with your spreadsheet.\n\n'
