// swift-tools-version:6.2
import PackageDescription

// The engine is a Rust static library: engine/build-lib.sh writes engine/target/oc/liboc_engine.a
// (build.sh runs it; run it yourself before a plain `swift build`).
let engineLib = Context.packageDirectory + "/../engine/target/oc"

let package = Package(
    name: "OpenCleaner",
    platforms: [.macOS(.v26)],
    targets: [
        .systemLibrary(name: "COcEngine", path: "Sources/COcEngine"),
        .executableTarget(
            name: "OpenCleaner",
            dependencies: ["COcEngine"],
            swiftSettings: [.swiftLanguageMode(.v5)],
            linkerSettings: [.unsafeFlags(["-L", engineLib])]
        ),
        .testTarget(name: "OpenCleanerTests", dependencies: ["OpenCleaner"], swiftSettings: [.swiftLanguageMode(.v5)]),
    ]
)
