# Template: the release workflow fills in the version and sha256 and attaches the result to the release.
cask "opencleaner" do
  version "{{VERSION}}"
  sha256 "{{SHA256}}"

  url "https://github.com/BuiHung1612/OpenCleaner/releases/download/v#{version}/OpenCleaner-macos.zip"
  name "OpenCleaner"
  desc "Free, open-source disk cleaner"
  homepage "https://github.com/BuiHung1612/OpenCleaner"

  auto_updates true
  depends_on macos: ">= :tahoe"

  app "OpenCleaner.app"

  zap trash: [
    "~/Library/Caches/OpenCleaner",
    "~/Library/Preferences/io.github.opencleaner.plist",
  ]
end
