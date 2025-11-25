#!/bin/bash

# ==============================================================================
# README Version Updater for CC-Switch CLI
# ==============================================================================
# This script automatically updates version numbers in README.md and README_ZH.md
#
# Usage:
#   ./scripts/update-readme-version.sh           # Auto-detect from Cargo.toml
#   ./scripts/update-readme-version.sh 4.2.0     # Specify version manually
#
# What it updates:
#   - Badge version in both READMEs
#   - All download links (macOS, Linux x64/ARM64, Windows)
#   - All extraction commands
# ==============================================================================

set -e  # Exit on error

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# ==============================================================================
# Helper Functions
# ==============================================================================

log_info() {
    echo -e "${BLUE}ℹ${NC} $1"
}

log_success() {
    echo -e "${GREEN}✓${NC} $1"
}

log_warning() {
    echo -e "${YELLOW}⚠${NC} $1"
}

log_error() {
    echo -e "${RED}✗${NC} $1"
}

# ==============================================================================
# Get Current Version from Cargo.toml
# ==============================================================================

get_cargo_version() {
    local cargo_toml="src-tauri/Cargo.toml"

    if [[ ! -f "$cargo_toml" ]]; then
        log_error "Cargo.toml not found at: $cargo_toml"
        exit 1
    fi

    # Extract version from Cargo.toml (format: version = "X.X.X")
    local version=$(grep '^version = ' "$cargo_toml" | head -1 | sed 's/.*"\(.*\)".*/\1/')

    if [[ -z "$version" ]]; then
        log_error "Failed to extract version from $cargo_toml"
        exit 1
    fi

    echo "$version"
}

# ==============================================================================
# Validate Version Format
# ==============================================================================

validate_version() {
    local version=$1

    # Check if version matches X.X.X format (semantic versioning)
    if ! [[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
        log_error "Invalid version format: $version"
        log_error "Expected format: X.X.X (e.g., 4.1.0)"
        exit 1
    fi
}

# ==============================================================================
# Extract Current Version from README
# ==============================================================================

get_readme_version() {
    local readme_file=$1

    if [[ ! -f "$readme_file" ]]; then
        log_error "README not found: $readme_file"
        exit 1
    fi

    # Extract version from badge line
    local version=$(grep -m 1 'img.shields.io/badge/version-' "$readme_file" | sed 's/.*version-\([0-9.]*\)-.*/\1/')

    if [[ -z "$version" ]]; then
        log_warning "Could not detect current version in $readme_file"
        echo "unknown"
    else
        echo "$version"
    fi
}

# ==============================================================================
# Update README File
# ==============================================================================

update_readme() {
    local readme_file=$1
    local old_version=$2
    local new_version=$3

    if [[ ! -f "$readme_file" ]]; then
        log_error "File not found: $readme_file"
        return 1
    fi

    # Create backup
    cp "$readme_file" "${readme_file}.bak"
    log_info "Created backup: ${readme_file}.bak"

    # Use sed to replace all version occurrences
    # Note: Using | as delimiter to avoid conflicts with / in URLs

    # 1. Update badge version (version-X.X.X-blue.svg)
    sed -i '' "s|version-[0-9.]*-blue\.svg|version-${new_version}-blue.svg|g" "$readme_file"

    # 2. Update macOS download link and tar command
    sed -i '' "s|cc-switch-cli-v[0-9.]*-darwin-universal\.tar\.gz|cc-switch-cli-v${new_version}-darwin-universal.tar.gz|g" "$readme_file"

    # 3. Update Linux x64 download link and tar command
    sed -i '' "s|cc-switch-cli-v[0-9.]*-linux-x64-musl\.tar\.gz|cc-switch-cli-v${new_version}-linux-x64-musl.tar.gz|g" "$readme_file"

    # 4. Update Linux ARM64 download link and tar command
    sed -i '' "s|cc-switch-cli-v[0-9.]*-linux-arm64-musl\.tar\.gz|cc-switch-cli-v${new_version}-linux-arm64-musl.tar.gz|g" "$readme_file"

    # 5. Update Windows download link
    sed -i '' "s|cc-switch-cli-v[0-9.]*-windows-x64\.zip|cc-switch-cli-v${new_version}-windows-x64.zip|g" "$readme_file"

    # Verify that changes were made
    if diff -q "$readme_file" "${readme_file}.bak" > /dev/null 2>&1; then
        log_warning "No changes made to $readme_file (version might already be up-to-date)"
        rm "${readme_file}.bak"
        return 0
    fi

    log_success "Updated: $readme_file"

    # Show diff summary
    local changes=$(diff -u "${readme_file}.bak" "$readme_file" | grep -c '^[-+]' || echo "0")
    log_info "  Changed lines: $((changes / 2))"
}

# ==============================================================================
# Main Script
# ==============================================================================

main() {
    echo ""
    log_info "CC-Switch README Version Updater"
    echo "========================================"
    echo ""

    # Determine target version
    local new_version=""

    if [[ $# -eq 0 ]]; then
        # No argument provided, read from Cargo.toml
        log_info "Reading version from src-tauri/Cargo.toml..."
        new_version=$(get_cargo_version)
        log_success "Detected version: $new_version"
    elif [[ $# -eq 1 ]]; then
        # Version provided as argument
        new_version=$1
        log_info "Using provided version: $new_version"
    else
        log_error "Usage: $0 [version]"
        log_error "Example: $0 4.2.0"
        exit 1
    fi

    # Validate version format
    validate_version "$new_version"

    echo ""
    log_info "Checking current README versions..."

    # Get current versions from READMEs
    local readme_en="README.md"
    local readme_zh="README_ZH.md"

    local old_version_en=$(get_readme_version "$readme_en")
    local old_version_zh=$(get_readme_version "$readme_zh")

    echo ""
    log_info "Version Summary:"
    echo "  README.md:    $old_version_en → $new_version"
    echo "  README_ZH.md: $old_version_zh → $new_version"

    # Check if update is needed
    if [[ "$old_version_en" == "$new_version" && "$old_version_zh" == "$new_version" ]]; then
        echo ""
        log_success "All README files are already up-to-date with version $new_version"
        exit 0
    fi

    echo ""
    log_info "Starting update process..."
    echo ""

    # Update both README files
    update_readme "$readme_en" "$old_version_en" "$new_version"
    update_readme "$readme_zh" "$old_version_zh" "$new_version"

    echo ""
    log_success "All README files updated successfully!"
    echo ""
    log_info "Updated files:"
    echo "  - README.md"
    echo "  - README_ZH.md"
    echo ""
    log_info "Next steps:"
    echo "  1. Review changes: git diff README.md README_ZH.md"
    echo "  2. Commit changes: git add README*.md && git commit -m 'chore: bump version to $new_version'"
    echo "  3. If needed, restore backups: mv README.md.bak README.md && mv README_ZH.md.bak README_ZH.md"
    echo ""
}

# Run main function
main "$@"
