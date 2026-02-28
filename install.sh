#!/usr/bin/env bash
set -Eeuo pipefail

REPO="SaladDay/cc-switch-cli"
BIN_NAME="cc-switch"
INSTALL_DIR="${CC_SWITCH_INSTALL_DIR:-$HOME/.local/bin}"
TARGET="${INSTALL_DIR}/${BIN_NAME}"
RELEASES_URL="https://github.com/${REPO}/releases"

TMP_DIR=""
ASSET_NAME=""

# ── helpers ──────────────────────────────────────────────────────────

info()  { printf '  \033[1;32minfo\033[0m: %s\n' "$*"; }
warn()  { printf '  \033[1;33mwarn\033[0m: %s\n' "$*" >&2; }
err()   { printf '  \033[1;31merror\033[0m: %s\n' "$*" >&2; }

cleanup() {
  if [[ -n "${TMP_DIR}" && -d "${TMP_DIR}" ]]; then
    rm -rf "${TMP_DIR}"
  fi
}

on_error() {
  err "Installation failed (line ${1:-?})"
  err "If the problem persists, download manually: ${RELEASES_URL}"
}

need_cmd() {
  if ! command -v "$1" >/dev/null 2>&1; then
    err "Required command not found: $1"
    exit 1
  fi
}

# ── platform detection ───────────────────────────────────────────────

detect_asset() {
  local os arch
  os="$(uname -s 2>/dev/null || true)"
  arch="$(uname -m 2>/dev/null || true)"

  case "${os}" in
    Darwin)
      # Universal binary works on both Apple Silicon and Intel
      ASSET_NAME="cc-switch-cli-darwin-universal.tar.gz"
      ;;
    Linux)
      case "${arch}" in
        x86_64|amd64)
          ASSET_NAME="cc-switch-cli-linux-x64-musl.tar.gz"
          ;;
        aarch64|arm64)
          ASSET_NAME="cc-switch-cli-linux-arm64-musl.tar.gz"
          ;;
        *)
          err "Unsupported Linux architecture: ${arch}"
          err "See available assets: ${RELEASES_URL}"
          exit 1
          ;;
      esac
      ;;
    MINGW*|MSYS*|CYGWIN*|Windows_NT)
      err "This script does not support Windows."
      err "Download cc-switch-cli-windows-x64.zip from: ${RELEASES_URL}"
      exit 1
      ;;
    *)
      err "Unsupported OS: ${os}"
      err "See available assets: ${RELEASES_URL}"
      exit 1
      ;;
  esac
}

# ── download & extract ───────────────────────────────────────────────

download() {
  local url="${RELEASES_URL}/latest/download/${ASSET_NAME}"
  local dest="${TMP_DIR}/${ASSET_NAME}"

  info "Downloading ${ASSET_NAME}"

  if command -v curl >/dev/null 2>&1; then
    curl --fail --location --silent --show-error --output "${dest}" "${url}" || {
      err "Download failed: ${url}"
      exit 1
    }
  elif command -v wget >/dev/null 2>&1; then
    wget --quiet --output-document="${dest}" "${url}" || {
      err "Download failed: ${url}"
      exit 1
    }
  else
    err "Neither curl nor wget found. Please install one and retry."
    exit 1
  fi
}

extract() {
  info "Extracting archive"
  tar -xzf "${TMP_DIR}/${ASSET_NAME}" -C "${TMP_DIR}"

  if [[ ! -f "${TMP_DIR}/${BIN_NAME}" ]]; then
    err "Binary '${BIN_NAME}' not found in archive."
    exit 1
  fi
}

# ── install ──────────────────────────────────────────────────────────

install_binary() {
  mkdir -p "${INSTALL_DIR}"
  mv -f "${TMP_DIR}/${BIN_NAME}" "${TARGET}"
  chmod 755 "${TARGET}"

  # macOS: clear Gatekeeper quarantine flag
  if [[ "$(uname -s)" == "Darwin" ]] && command -v xattr >/dev/null 2>&1; then
    xattr -cr "${TARGET}" 2>/dev/null || true
  fi
}

check_path() {
  case ":${PATH}:" in
    *":${INSTALL_DIR}:"*)
      return 0
      ;;
  esac

  local shell_name profile cmd
  shell_name="$(basename "${SHELL:-bash}")"

  case "${shell_name}" in
    zsh)
      profile="\$HOME/.zshrc"
      cmd="export PATH=\"${INSTALL_DIR}:\$PATH\""
      ;;
    fish)
      profile="\$HOME/.config/fish/config.fish"
      cmd="fish_add_path ${INSTALL_DIR}"
      ;;
    *)
      profile="\$HOME/.bashrc"
      cmd="export PATH=\"${INSTALL_DIR}:\$PATH\""
      ;;
  esac

  warn "${INSTALL_DIR} is not in your PATH"
  printf '\n  Add this to %s:\n\n    %s\n\n' "${profile}" "${cmd}"
  printf '  Then restart your shell or run:\n\n    %s\n\n' "${cmd}"
}

# ── main ─────────────────────────────────────────────────────────────

main() {
  trap cleanup EXIT
  trap 'on_error "${LINENO}"' ERR

  need_cmd uname
  need_cmd tar
  need_cmd mktemp

  detect_asset

  TMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/cc-switch-install.XXXXXX")"

  download
  extract
  install_binary

  info "Installed ${BIN_NAME} to ${TARGET}"
  check_path
  printf '  Run \033[1m%s --version\033[0m to verify.\n\n' "${BIN_NAME}"
}

main "$@"
