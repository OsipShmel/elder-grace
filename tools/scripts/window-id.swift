// Номер окна по подстроке заголовка (для `screencapture -l`): xcrun swift window-id.swift "ELDEN RING"
// Печатает первый найденный номер окна, у которого заголовок содержит аргумент.
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
