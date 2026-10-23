# Homebrew formula template for orq. Copy it to Formula/orq.rb in a tap
# repository (for example circe07/homebrew-tap) and keep it up to date.
#
# To fill the placeholders after a release vX.Y.Z:
#   1. Set `version` below to X.Y.Z.
#   2. For each target, download the matching `.sha256` asset from the GitHub
#      release (orq-X.Y.Z-<target>.tar.gz.sha256). The first field of that file
#      is the hash; replace the matching `sha256` value below with it.
#      Example:
#        curl -sL https://github.com/circe07/orq/releases/download/vX.Y.Z/orq-X.Y.Z-aarch64-apple-darwin.tar.gz.sha256
#
# Install with:  brew install circe07/tap/orq
class Orq < Formula
  desc "Rust build orchestrator for AI agents: verify changes fast, in few tokens"
  homepage "https://github.com/circe07/orq"
  version "0.4.0"
  license any_of: ["MIT", "Apache-2.0"]

  on_macos do
    on_arm do
      url "https://github.com/circe07/orq/releases/download/v#{version}/orq-#{version}-aarch64-apple-darwin.tar.gz"
      sha256 "9ef817b406b5077bac5e51b206bd01b708e483d6133e93e05584d469a9c90304"
    end
    on_intel do
      url "https://github.com/circe07/orq/releases/download/v#{version}/orq-#{version}-x86_64-apple-darwin.tar.gz"
      sha256 "eaceaa58aa0401435e723b85b39859ebf7873722155d428f20bdda78602413f6"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/circe07/orq/releases/download/v#{version}/orq-#{version}-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "c0771c473cad5fa11c1415d4d38b4577db44dc971da0e1ef9b428c79dd4af205"
    end
    on_intel do
      url "https://github.com/circe07/orq/releases/download/v#{version}/orq-#{version}-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "b83c812fb6b755e9ba88a58059fe4e54a607edf5d172e0368e4b3ad2604d421e"
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
