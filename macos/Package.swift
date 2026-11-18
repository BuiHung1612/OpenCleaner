// swift-tools-version:6.2
import PackageDescription

let package = Package(
    name: "OpenCleaner",
    platforms: [.macOS(.v26)],
    targets: [.executableTarget(name: "OpenCleaner", swiftSettings: [.swiftLanguageMode(.v5)])]
)
