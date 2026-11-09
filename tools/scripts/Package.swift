// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "grace-live",
    products: [
        .library(name: "GraceProto", targets: ["GraceProto"]),
        .library(name: "GraceLive", targets: ["GraceLive"]),
    ],
    dependencies: [
        // The standard library's Atomic is ~Copyable and made to live inside a type, while
        // UnsafeAtomic is made for exactly this: an atomic at an address someone else owns.
        .package(url: "https://github.com/apple/swift-atomics.git", from: "1.2.0"),
    ],
    targets: [
        .target(name: "GraceProto", dependencies: [.product(name: "Atomics", package: "swift-atomics")]),
        .target(name: "GraceLive", dependencies: ["GraceProto"]),
        .testTarget(name: "GraceLiveTests", dependencies: ["GraceProto", "GraceLive"]),
    ]
)
