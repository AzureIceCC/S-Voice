import Foundation

private func emit(_ value: [String: Any], status: Int32 = EXIT_FAILURE) -> Never {
    let data = try! JSONSerialization.data(withJSONObject: value, options: [.sortedKeys])
    FileHandle.standardOutput.write(data)
    FileHandle.standardOutput.write(Data("\n".utf8))
    exit(status)
}

@main
private enum Main {
    static func main() {
        emit([
            "ok": false,
            "error": "Apple SpeechAnalyzer is unavailable: this macOS SDK does not provide the required Speech framework modules",
        ])
    }
}
