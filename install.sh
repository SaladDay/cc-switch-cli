#!/usr/bin/env bash
set -Eeuo pipefail

REPO="SaladDay/cc-switch-cli"
BIN_NAME="cc-switch"
INSTALL_DIR="${CC_SWITCH_INSTALL_DIR:-$HOME/.local/bin}"
TARGET="${INSTALL_DIR}/${BIN_NAME}"
RELEASES_URL="https://github.com/${REPO}/releases"
FORCE_OVERWRITE="${CC_SWITCH_FORCE:-0}"
LINUX_LIBC="${CC_SWITCH_LINUX_LIBC:-auto}"
VERSION="${1:-latest}"
[[ "${VERSION}" == "latest" ]] || [[ "${VERSION}" == v* ]] || VERSION="v${VERSION}"

TMP_DIR=""
ASSET_NAME=""
ASSET_CANDIDATES=()

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

installed_version() {
  local candidate="${1:-}"

  if [[ -z "${candidate}" || ! -x "${candidate}" ]]; then
    return 0
  fi

  "${candidate}" --version 2>/dev/null | head -n 1 || true
}

confirm_overwrite_if_needed() {
  local target_version reply

  if [[ ! -e "${TARGET}" ]]; then
    return 0
  fi

  target_version="$(installed_version "${TARGET}")"

  if [[ "${FORCE_OVERWRITE}" == "1" ]]; then
    warn "Existing installation detected at ${TARGET}${target_version:+ (${target_version})}; continuing because CC_SWITCH_FORCE=1"
    return 0
  fi

  if ! exec 3<> /dev/tty 2>/dev/null; then
    err "Existing installation detected at ${TARGET}${target_version:+ (${target_version})}."
    err "Nothing was overwritten. Re-run interactively to confirm the update, or set CC_SWITCH_FORCE=1 to allow overwrite."
    exit 1
  fi

  printf '  Existing installation detected at %s' "${TARGET}" >&3
  if [[ -n "${target_version}" ]]; then
    printf ' (%s)' "${target_version}" >&3
  fi
  printf '.\n\n  [U]pdate or [C]ancel? [U/c] ' >&3
  IFS= read -r reply <&3
  exec 3>&-

  case "${reply:-u}" in
    u|U|update|UPDATE|"")
      return 0
      ;;
    c|C|cancel|CANCEL)
      info "Installation canceled."
      exit 0
      ;;
    *)
      err "Unrecognized choice: ${reply}"
      err "Nothing was overwritten. Re-run and choose Update, or set CC_SWITCH_FORCE=1 to allow overwrite."
      exit 1
      ;;
  esac
}

linux_libc_mode() {
  case "${LINUX_LIBC}" in
    auto|AUTO|"")
      printf 'auto'
      ;;
    musl|MUSL)
      printf 'musl'
      ;;
    glibc|GLIBC|gnu|GNU)
      printf 'glibc'
      ;;
    *)
      err "Unsupported CC_SWITCH_LINUX_LIBC value: ${LINUX_LIBC}"
      err "Use one of: auto, musl, glibc"
      exit 1
      ;;
  esac
}

set_linux_asset_candidates() {
  local arch="$1"
  local mode
  mode="$(linux_libc_mode)"

  case "${arch}" in
    x86_64|amd64)
      case "${mode}" in
        auto)
          ASSET_CANDIDATES=(
            "cc-switch-cli-linux-x64-musl.tar.gz"
            "cc-switch-cli-linux-x64.tar.gz"
          )
          ;;
        musl)
          ASSET_CANDIDATES=("cc-switch-cli-linux-x64-musl.tar.gz")
          ;;
        glibc)
          ASSET_CANDIDATES=("cc-switch-cli-linux-x64.tar.gz")
          ;;
      esac
      ;;
    aarch64|arm64)
      case "${mode}" in
        auto)
          ASSET_CANDIDATES=(
            "cc-switch-cli-linux-arm64-musl.tar.gz"
            "cc-switch-cli-linux-arm64.tar.gz"
          )
          ;;
        musl)
          ASSET_CANDIDATES=("cc-switch-cli-linux-arm64-musl.tar.gz")
          ;;
        glibc)
          ASSET_CANDIDATES=("cc-switch-cli-linux-arm64.tar.gz")
          ;;
      esac
      ;;
    *)
      err "Unsupported Linux architecture: ${arch}"
      err "See available assets: ${RELEASES_URL}"
      exit 1
      ;;
  esac

  case "${mode}" in
    auto)
      info "Linux defaults to the static musl build and falls back to glibc if needed."
      ;;
    musl)
      info "Using Linux musl build because CC_SWITCH_LINUX_LIBC=${LINUX_LIBC}."
      ;;
    glibc)
      info "Using Linux glibc build because CC_SWITCH_LINUX_LIBC=${LINUX_LIBC}."
      ;;
  esac
}

# ── platform detection ───────────────────────────────────────────────

detect_asset() {
  local os arch
  os="$(uname -s 2>/dev/null || true)"
  arch="$(uname -m 2>/dev/null || true)"
  ASSET_CANDIDATES=()

  case "${os}" in
    Darwin)
      # Universal binary works on both Apple Silicon and Intel
      ASSET_CANDIDATES=("cc-switch-cli-darwin-universal.tar.gz")
      ;;
    Linux)
      set_linux_asset_candidates "${arch}"
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

  ASSET_NAME="${ASSET_CANDIDATES[0]}"
}

# ── download & extract ───────────────────────────────────────────────

download_asset() {
  local url="$1"
  local dest="$2"

  if command -v curl >/dev/null 2>&1; then
    curl --fail --location --silent --show-error --output "${dest}" "${url}"
  elif command -v wget >/dev/null 2>&1; then
    wget --quiet --output-document="${dest}" "${url}"
  else
    err "Neither curl nor wget found. Please install one and retry."
    exit 1
  fi
}

download() {
  local asset_name url dest

  for asset_name in "${ASSET_CANDIDATES[@]}"; do
    if [[ "${VERSION}" == "latest" ]]; then
      url="${RELEASES_URL}/latest/download/${asset_name}"
    else
      url="${RELEASES_URL}/download/${VERSION}/${asset_name}"
    fi
    dest="${TMP_DIR}/${asset_name}"

    info "Downloading ${asset_name}"

    if download_asset "${url}" "${dest}"; then
      ASSET_NAME="${asset_name}"
      return 0
    fi

    rm -f "${dest}"
    warn "Download failed: ${url}"
  done

  err "Unable to download a compatible release asset."
  err "See available assets: ${RELEASES_URL}"
  exit 1
}

extract() {
  info "Extracting archive"
  LC_ALL=C tar -xzf "${TMP_DIR}/${ASSET_NAME}" -C "${TMP_DIR}"

  if [[ ! -f "${TMP_DIR}/${BIN_NAME}" ]]; then
    err "Binary '${BIN_NAME}' not found in archive."
    exit 1
  fi
}

# ── install ──────────────────────────────────────────────────────────

install_binary() {
  local staged_target="${TARGET}.new"

  mkdir -p "${INSTALL_DIR}"
  rm -f "${staged_target}"
  cp "${TMP_DIR}/${BIN_NAME}" "${staged_target}"
  chmod 755 "${staged_target}"
  mv -f "${staged_target}" "${TARGET}"
  chmod 755 "${TARGET}"

  # macOS: clear Gatekeeper quarantine flag
  if [[ "$(uname -s)" == "Darwin" ]] && command -v xattr >/dev/null 2>&1; then
    xattr -cr "${TARGET}" 2>/dev/null || true
  fi
}

check_path_shadow() {
  local resolved
  resolved="$(command -v "${BIN_NAME}" 2>/dev/null || true)"

  if [[ -n "${resolved}" && "${resolved}" != "${TARGET}" ]]; then
    warn "${BIN_NAME} currently resolves to ${resolved}, so ${TARGET} may be shadowed."
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
  confirm_overwrite_if_needed

  TMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/cc-switch-install.XXXXXX")"

  download
  extract
  install_binary

  info "Installed ${BIN_NAME} to ${TARGET}"
  check_path_shadow
  check_path
  printf '  Run \033[1m%s --version\033[0m to verify.\n\n' "${BIN_NAME}"
}

main "$@"
