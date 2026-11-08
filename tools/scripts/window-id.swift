// Window number by title substring (for `screencapture -l`): xcrun swift window-id.swift "ELDEN RING"
// Prints the number of the first window whose title contains the argument.
import CoreGraphics
let needle = CommandLine.arguments.dropFirst().first ?? ""
let list = CGWindowListCopyWindowInfo([.optionAll], kCGNullWindowID) as! [[String: Any]]
for w in list {
    let name = w[kCGWindowName as String] as? String ?? ""
    if !needle.isEmpty && name.contains(needle) {
        print(w[kCGWindowNumber as String]!)
        break
    }
}
