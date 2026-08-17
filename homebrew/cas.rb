# typed: false
# frozen_string_literal: true

# Homebrew formula for CAS - Coding Agent System
# Install with: brew install --formula ./homebrew/cas.rb

# Homebrew formula for the CAS command-line application.
class Cas < Formula
  desc "Coding Agent System - persistent memory, tasks, rules, and skills for AI agents"
  homepage "https://github.com/Richards-LLC/cassy"
  version "2.55.5"
  license "MIT"

  on_macos do
    on_arm do
      url "https://github.com/Richards-LLC/cassy/releases/download/v#{version}/cas-aarch64-apple-darwin.tar.gz"
      sha256 "c97fb8358ed70a6d068be765cab2f30c102c8767b66a50707a4617ef8b9e34be"
    end
    on_intel do
      odie "CAS does not support Intel macOS. Please use an Apple Silicon Mac."
    end
  end

  on_linux do
    on_intel do
      url "https://github.com/Richards-LLC/cassy/releases/download/v#{version}/cas-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "8aa2c54f313a38a8df7cb29a1974e271a3f049342911940fecc6fec4fa7feb0a"
    end
    on_arm do
      odie "CAS does not support ARM64 Linux."
    end
  end

  def install
    bin.install "cas"
  end

  def caveats
    <<~EOS
      CAS has been installed!

      To get started:
        cas init          # Initialize in your project
        cas serve         # Start the MCP server

      CAS stores data in:
        ~/.config/cas/    (global data)
        .cas/             (project data)

      Optional companion tools (used by built-in skills):
        fallow            # JS/TS codebase intelligence — used by the
                          #   `fallow` skill for dead code, duplication,
                          #   complexity, and PR audit gates.
                          #   Install with: npm install -g fallow
                          #   Or run on demand: npx fallow

      To update CAS:
        cas update        (self-update)
        brew upgrade cas  (via Homebrew)
    EOS
  end

  test do
    assert_match "cas", shell_output("#{bin}/cas --version")
  end
end
