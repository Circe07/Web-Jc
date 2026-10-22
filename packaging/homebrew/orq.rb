# Homebrew formula template for orq. Copy it to Formula/orq.rb in a tap
# repository (for example circe07/homebrew-tap) and keep it up to date.
#
# To fill the placeholders after a release vX.Y.Z:
#   1. Set `version` below to X.Y.Z.
#   2. For each target, download the matching `.sha256` asset from the GitHub
#      release (orq-X.Y.Z-<target>.tar.gz.sha256). The first field of that file
#      is the hash; paste it in place of the REPLACE_WITH_SHA256_* value.
#      Example:
#        curl -sL https://github.com/circe07/orq/releases/download/vX.Y.Z/orq-X.Y.Z-aarch64-apple-darwin.tar.gz.sha256
#
# Install with:  brew install circe07/tap/orq
class Orq < Formula
  desc "Rust build orchestrator for AI agents: verify changes fast, in few tokens"
  homepage "https://github.com/circe07/orq"
  version "0.2.0"
  license any_of: ["MIT", "Apache-2.0"]

  on_macos do
    on_arm do
      url "https://github.com/circe07/orq/releases/download/v#{version}/orq-#{version}-aarch64-apple-darwin.tar.gz"
      sha256 "REPLACE_WITH_SHA256_AARCH64_APPLE_DARWIN"
    end
    on_intel do
      url "https://github.com/circe07/orq/releases/download/v#{version}/orq-#{version}-x86_64-apple-darwin.tar.gz"
      sha256 "REPLACE_WITH_SHA256_X86_64_APPLE_DARWIN"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/circe07/orq/releases/download/v#{version}/orq-#{version}-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "REPLACE_WITH_SHA256_AARCH64_UNKNOWN_LINUX_GNU"
    end
    on_intel do
      url "https://github.com/circe07/orq/releases/download/v#{version}/orq-#{version}-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "REPLACE_WITH_SHA256_X86_64_UNKNOWN_LINUX_GNU"
    end
  end

  def install
    # The tarball contains a single directory orq-<version>-<target>/ with the binary.
    bin.install "orq"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/orq --version")
  end
end
