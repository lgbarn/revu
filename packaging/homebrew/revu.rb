# Homebrew formula for revu (prebuilt-binary formula).
#
# This is a TEMPLATE kept in-repo. It is NOT a live tap on its own. To make
# `brew install <tap>/revu` work, a human must:
#
#   1. Create the tap repository (a HUMAN DECISION - suggested name:
#      `lgbarn/homebrew-tap`, which Homebrew resolves as the tap
#      `lgbarn/tap`). Any `homebrew-<name>` repo under the org works.
#   2. Copy this file to `Formula/revu.rb` in that repo.
#   3. Per release: bump `version` and fill in the four `sha256` values below
#      with the checksums of the uploaded release tarballs, e.g.:
#         shasum -a 256 revu-aarch64-apple-darwin.tar.gz
#      (The release workflow uploads `revu-<target>.tar.gz` for each target.)
#
# Until the sha256 values are filled, `brew install` will refuse to proceed.
class Revu < Formula
  desc "Fast, memory-safe terminal diff/review tool (a Rust port of hunk)"
  homepage "https://github.com/lgbarn/revu"
  version "0.3.1" # Must match Cargo.toml and the release tag (without leading 'v').
  license any_of: ["MIT", "Apache-2.0"]

  on_macos do
    on_arm do
      url "https://github.com/lgbarn/revu/releases/download/v#{version}/revu-aarch64-apple-darwin.tar.gz"
      sha256 "TODO_FILL_SHA256_aarch64-apple-darwin"
    end
    on_intel do
      url "https://github.com/lgbarn/revu/releases/download/v#{version}/revu-x86_64-apple-darwin.tar.gz"
      sha256 "TODO_FILL_SHA256_x86_64-apple-darwin"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/lgbarn/revu/releases/download/v#{version}/revu-aarch64-unknown-linux-musl.tar.gz"
      sha256 "TODO_FILL_SHA256_aarch64-unknown-linux-musl"
    end
    on_intel do
      url "https://github.com/lgbarn/revu/releases/download/v#{version}/revu-x86_64-unknown-linux-musl.tar.gz"
      sha256 "TODO_FILL_SHA256_x86_64-unknown-linux-musl"
    end
  end

  def install
    bin.install "revu"
  end

  test do
    assert_match "revu", shell_output("#{bin}/revu --version")
  end
end
