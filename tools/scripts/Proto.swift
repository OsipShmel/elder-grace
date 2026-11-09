// STUB of `grace-proto`, which was not part of the ported sources: only the messages that
// LiveSession touches. The wire format is a protocol to be implemented by the real thing
// (one JSON line per message, whatever serde produces on the Rust side).

/// The part of `anyhow` the host uses. `{e:#}` is `String(describing: error)`.
public struct GraceError: Error, Equatable, Sendable, CustomStringConvertible {
    public let description: String

    public init(_ description: String) {
        self.description = description
    }

    public func context(_ outer: String) -> GraceError {
        GraceError("\(outer): \(description)")
    }
}

public enum Request: Equatable, Sendable {
    case mirror(regions: [MirrorRegion])
}

public enum Response: Equatable, Sendable {
    case mirror(generation: UInt32)
    case error(message: String)
    /// Stands in for every other `Response` variant.
    case other(debug: String)
}

public protocol WireFormat: Sendable {
    /// One message without the trailing newline.
    func encode(_ request: Request) throws -> String
    func decode(_ line: String) throws -> Response
}
