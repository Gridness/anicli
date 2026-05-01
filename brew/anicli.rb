class Anicli < Formula
  desc "Anime browser and player inspired by ani-cli"
  homepage "https://github.com/Gridness/anicli-rs"
  license "GPL-3.0-or-later"
  head "https://github.com/Gridness/anicli-rs.git", branch: "main"

  depends_on "rust" => :build
  depends_on "aria2" => :recommended
  depends_on "ffmpeg" => :recommended
  depends_on "yt-dlp" => :recommended

  def install
    system "cargo", "install", *std_cargo_args(path: "crates/anicli")
  end

  test do
    assert_match "Anime browser", shell_output("#{bin}/anicli --help")
  end
end
