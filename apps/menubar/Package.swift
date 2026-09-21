// swift-tools-version: 5.9
// The menu bar shell (design 09, "常驻"; design 06). A plain executable, no
// Xcode project: `swift build -c release` produces `genatrix-menubar`.
import PackageDescription

let package = Package(
    name: "genatrix-menubar",
    platforms: [.macOS(.v13)],
    targets: [
        .executableTarget(
            name: "genatrix-menubar",
            path: "Sources/genatrix-menubar"
        )
    ]
)
