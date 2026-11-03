#!/usr/bin/env bash
set -euo pipefail

if [ "$#" -ne 4 ]; then
    echo "Usage: $0 <version> <macos_artifact_path> <linux_x86_64_artifact_path> <linux_aarch64_artifact_path>"
    exit 1
fi

VERSION=$1
MACOS_ARTIFACT_INPUT=${2//\\//}
LINUX_X64_ARTIFACT_INPUT=${3//\\//}
LINUX_ARM_ARTIFACT_INPUT=${4//\\//}
MACOS_BASENAME=$(basename "$MACOS_ARTIFACT_INPUT")
LINUX_X64_BASENAME=$(basename "$LINUX_X64_ARTIFACT_INPUT")
LINUX_ARM_BASENAME=$(basename "$LINUX_ARM_ARTIFACT_INPUT")

mkdir -p artifacts

find_artifact() {
    local INPUT=$1
    local BASENAME=$2

    if [ -f "$INPUT" ]; then
        echo "$INPUT"
        return
    fi

    FOUND=$(find artifacts -type f -name "$BASENAME" -print -quit || true)
    if [ -n "$FOUND" ]; then
        echo "$FOUND"
        return
    fi

    echo "Error: Artifact '$BASENAME' not found under artifacts/" >&2
    exit 1
}

MACOS_PATH=$(find_artifact "$MACOS_ARTIFACT_INPUT" "$MACOS_BASENAME")
LINUX_X64_PATH=$(find_artifact "$LINUX_X64_ARTIFACT_INPUT" "$LINUX_X64_BASENAME")
LINUX_ARM_PATH=$(find_artifact "$LINUX_ARM_ARTIFACT_INPUT" "$LINUX_ARM_BASENAME")

MACOS_CHECKSUM=$(sha256sum "$MACOS_PATH" | awk '{print $1}')
LINUX_X64_CHECKSUM=$(sha256sum "$LINUX_X64_PATH" | awk '{print $1}')
LINUX_ARM_CHECKSUM=$(sha256sum "$LINUX_ARM_PATH" | awk '{print $1}')

echo "macOS: $MACOS_PATH $MACOS_CHECKSUM"
echo "Linux x86_64: $LINUX_X64_PATH $LINUX_X64_CHECKSUM"
echo "Linux arm64: $LINUX_ARM_PATH $LINUX_ARM_CHECKSUM"

if [ -z "${HOMEBREW_TAP_DEPLOY_KEY:-}" ]; then
  echo "Error: HOMEBREW_TAP_DEPLOY_KEY is not set!"
  exit 1
fi

# The tap's main branch is locked by a ruleset that only its deploy key can
# bypass, so the push must go over SSH with that key. The host key is pinned
# to GitHub's published ed25519 key rather than trusted on first use.
SSH_DIR=$(mktemp -d)
chmod 700 "$SSH_DIR"
printf '%s\n' "$HOMEBREW_TAP_DEPLOY_KEY" > "$SSH_DIR/deploy_key"
chmod 600 "$SSH_DIR/deploy_key"
echo "github.com ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl" > "$SSH_DIR/known_hosts"
export GIT_SSH_COMMAND="ssh -i $SSH_DIR/deploy_key -o IdentitiesOnly=yes -o UserKnownHostsFile=$SSH_DIR/known_hosts -o StrictHostKeyChecking=yes"
trap 'rm -rf "$SSH_DIR"' EXIT

TAP_DIR=$(mktemp -d)

echo "Cloning homebrew-tap into $TAP_DIR"
git clone "git@github.com:Glubiz/homebrew-tap.git" "$TAP_DIR"
FORMULA="$TAP_DIR/Formula/workuum.rb"
mkdir -p "$TAP_DIR/Formula"

BASE_URL="https://github.com/Glubiz/workuum/releases/download/v${VERSION}"

echo "Writing updated formula"

cat > "$FORMULA" << RUBY
class Workuum < Formula
  desc "Removes merged, stale and least-recently-used git worktrees to keep disk space free"
  homepage "https://github.com/Glubiz/workuum"
  license "MIT"
  version "${VERSION}"

  on_macos do
    url "${BASE_URL}/${MACOS_BASENAME}"
    sha256 "${MACOS_CHECKSUM}"
  end

  on_linux do
    if Hardware::CPU.arm?
      url "${BASE_URL}/${LINUX_ARM_BASENAME}"
      sha256 "${LINUX_ARM_CHECKSUM}"
    else
      url "${BASE_URL}/${LINUX_X64_BASENAME}"
      sha256 "${LINUX_X64_CHECKSUM}"
    end
  end

  def install
    bin.install "workuum"
  end

  test do
    system "#{bin}/workuum", "--version"
  end
end
RUBY

echo "Formula after update:"
cat "$FORMULA"

cd "$TAP_DIR"
git config user.email "ci@github.com"
git config user.name "GitHub Actions"

git add "$FORMULA"
if git diff-index --quiet HEAD --; then
    echo "No changes to commit"
else
    git commit -m "workuum: bump to v${VERSION}"
    git push origin main
    echo "Pushed formula update"
fi

rm -rf "$TAP_DIR"
