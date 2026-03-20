#!/usr/bin/env bash
set -euo pipefail

# ============================================================================
# SynapseClaw Rebrand Script
# Renames SynapseClaw → SynapseClaw across the entire repository.
#
# Uses perl for LITERAL string replacement (no regex metachar issues).
#
# Usage:
#   ./dev/rebrand.sh          # dry-run
#   ./dev/rebrand.sh --apply  # apply changes
# ============================================================================

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DRY_RUN=true
[[ "${1:-}" == "--apply" ]] && DRY_RUN=false

GREEN='\033[0;32m'; YELLOW='\033[1;33m'; CYAN='\033[0;36m'; NC='\033[0m'
log()  { echo -e "${GREEN}[rebrand]${NC} $*"; }
warn() { echo -e "${YELLOW}[rebrand]${NC} $*"; }
info() { echo -e "${CYAN}[rebrand]${NC} $*"; }

cd "$REPO_ROOT"

if [[ ! -f "Cargo.toml" ]] || ! grep -q "synapseclaw" Cargo.toml 2>/dev/null; then
    echo "ERROR: Not in the SynapseClaw repository root or already rebranded"
    exit 1
fi

if $DRY_RUN; then
    warn "DRY RUN — pass --apply to execute."
fi

# ── Literal replace helper using perl ──────────────────────────────────────
# perl -pi with \Q...\E ensures NO regex interpretation of the pattern.

do_literal_replace() {
    local from="$1" to="$2"
    local count
    count=$(grep -rl --fixed-strings "$from" \
        --include='*.rs' --include='*.toml' --include='*.ts' --include='*.tsx' \
        --include='*.js' --include='*.json' --include='*.yml' --include='*.yaml' \
        --include='*.sh' --include='*.md' --include='*.txt' --include='*.html' \
        --include='*.py' --include='*.cfg' --include='*.env' --include='*.example' \
        --include='Dockerfile*' --include='PKGBUILD' --include='.SRCINFO' \
        --include='Makefile' --include='*.css' \
        --exclude-dir=.git --exclude-dir=target --exclude-dir=node_modules \
        . 2>/dev/null | wc -l)

    if [[ "$count" -eq 0 ]]; then
        return
    fi

    if $DRY_RUN; then
        info "  '$from' → '$to' ($count files)"
    else
        grep -rl --fixed-strings "$from" \
            --include='*.rs' --include='*.toml' --include='*.ts' --include='*.tsx' \
            --include='*.js' --include='*.json' --include='*.yml' --include='*.yaml' \
            --include='*.sh' --include='*.md' --include='*.txt' --include='*.html' \
            --include='*.py' --include='*.cfg' --include='*.env' --include='*.example' \
            --include='Dockerfile*' --include='PKGBUILD' --include='.SRCINFO' \
            --include='Makefile' --include='*.css' \
            --exclude-dir=.git --exclude-dir=target --exclude-dir=node_modules \
            . 2>/dev/null | while IFS= read -r file; do
                perl -pi -e "s{\Q${from}\E}{${to}}g" "$file"
            done
        log "  '$from' → '$to' ($count files)"
    fi
}

# ── Step 1: Text replacements (longest patterns first) ─────────────────────

log "Step 1: Text replacements"

# GitHub org + repo (most specific first)
do_literal_replace 'panviktor/synapseclaw'  'panviktor/synapseclaw'
do_literal_replace 'panviktor'           'panviktor'

# Domain & social
do_literal_replace 'synapseclaw.dev'        'synapseclaw.dev'
do_literal_replace '@synapseclaw'          '@synapseclaw'
do_literal_replace 'r/synapseclaw'         'r/synapseclaw'
do_literal_replace 'facebook.com/groups/synapseclaw'  'facebook.com/groups/synapseclaw'

# Crate name (before generic lowercase)
do_literal_replace 'synapseclaw'           'synapseclaw'

# Email & domain
do_literal_replace 'bot@synapseclaw.dev'       'bot@synapseclaw.dev'
do_literal_replace 'synapseclaw.dev'           'synapseclaw.dev'

# Docker cache IDs
do_literal_replace 'synapseclaw-cargo-registry' 'synapseclaw-cargo-registry'
do_literal_replace 'synapseclaw-cargo-git'      'synapseclaw-cargo-git'
do_literal_replace 'synapseclaw-target'         'synapseclaw-target'
do_literal_replace 'synapseclaw-data'           'synapseclaw-data'

# Python package (before generic)
do_literal_replace 'synapseclaw_tools'         'synapseclaw_tools'

# Systemd bot user
do_literal_replace 'synapseclaw-bot'           'synapseclaw-bot'

# Config path (literal dot — no regex issue with perl \Q\E)
do_literal_replace '.synapseclaw'              '.synapseclaw'

# Env vars (UPPERCASE)
do_literal_replace 'SYNAPSECLAW_'              'SYNAPSECLAW_'
do_literal_replace 'SYNAPSECLAW'               'SYNAPSECLAW'

# PascalCase
do_literal_replace 'SynapseClaw'               'SynapseClaw'

# Lowercase (last — most generic)
do_literal_replace 'synapseclaw'               'synapseclaw'

# ── Step 2: Fork attribution (NOTICE) ─────────────────────────────────────

log "Step 2: Writing NOTICE with upstream attribution"
if ! $DRY_RUN; then
    cat > NOTICE << 'NOTICE_EOF'
SynapseClaw
Copyright 2025-2026 SynapseClaw Contributors

SynapseClaw is a fork of SynapseClaw (https://github.com/panviktor/synapseclaw),
originally developed by SynapseClaw Labs under MIT and Apache 2.0 licenses.

Forked to add inter-agent IPC coordination, multi-agent fleet management,
webhook push delivery, and security hardening.

Original Notice
===============

SynapseClaw
Copyright 2025 SynapseClaw Labs

This product includes software developed at SynapseClaw Labs
(https://github.com/panviktor).

License
=======

This software is available under a dual-license model:

  1. MIT License — see LICENSE-MIT
  2. Apache License 2.0 — see LICENSE-APACHE

You may use either license. Contributors grant rights under both.
NOTICE_EOF
    log "  NOTICE written"
else
    info "  Would write NOTICE with fork attribution"
fi

# ── Step 3: Rename files and directories ───────────────────────────────────

log "Step 3: Rename files and directories"

rename_path() {
    local src="$1" dst="$2"
    if [[ -e "$src" ]]; then
        if $DRY_RUN; then
            info "  mv $src → $dst"
        else
            mv "$src" "$dst"
            log "  $src → $dst"
        fi
    fi
}

rename_path 'python/synapseclaw_tools'      'python/synapseclaw_tools'
rename_path 'dist/scoop/synapseclaw.json'   'dist/scoop/synapseclaw.json'

# ── Step 4: Regenerate lock files ──────────────────────────────────────────

log "Step 4: Regenerate lock files"
if ! $DRY_RUN; then
    cargo generate-lockfile 2>&1 || warn "  cargo generate-lockfile failed"
    (cd web && npm install --package-lock-only 2>/dev/null) || warn "  npm install failed"
    log "  Lock files regenerated"
else
    info "  Would regenerate Cargo.lock and package-lock.json"
fi

# ── Step 5: Verify ────────────────────────────────────────────────────────

log "Step 5: Verification"
if ! $DRY_RUN; then
    for pat in 'synapseclaw' 'panviktor' 'SYNAPSECLAW_' 'SynapseClaw' 'synapseclaw'; do
        count=$(grep -rl --fixed-strings "$pat" \
            --include='*.rs' --include='*.toml' --include='*.ts' --include='*.tsx' \
            --include='*.json' --include='*.yml' --include='*.sh' --include='*.md' \
            --include='*.py' --include='*.html' \
            --exclude-dir=.git --exclude-dir=target --exclude-dir=node_modules \
            . 2>/dev/null | grep -v NOTICE | grep -v rebrand.sh | wc -l)
        if [[ "$count" -gt 0 ]]; then
            warn "  ⚠ $count files still contain '$pat'"
            grep -rl --fixed-strings "$pat" \
                --include='*.rs' --include='*.toml' --include='*.ts' --include='*.tsx' \
                --include='*.json' --include='*.yml' --include='*.sh' --include='*.md' \
                --include='*.py' --include='*.html' \
                --exclude-dir=.git --exclude-dir=target --exclude-dir=node_modules \
                . 2>/dev/null | grep -v NOTICE | grep -v rebrand.sh | head -5
        else
            log "  ✓ '$pat' — clean"
        fi
    done
fi

# ── Done ──────────────────────────────────────────────────────────────────

echo ""
if $DRY_RUN; then
    warn "═══════════════════════════════════════════"
    warn "  DRY RUN complete. Run --apply to execute."
    warn "═══════════════════════════════════════════"
else
    log "═══════════════════════════════════════════"
    log "  Rebrand complete: SynapseClaw → SynapseClaw"
    log ""
    log "  Next steps:"
    log "    1. cargo build && cd web && npm run build"
    log "    2. git diff --stat"
    log "    3. Rename GitHub repo → panviktor/synapseclaw"
    log "    4. Migrate ~/.synapseclaw → ~/.synapseclaw"
    log "    5. Rename systemd units"
    log "═══════════════════════════════════════════"
fi
