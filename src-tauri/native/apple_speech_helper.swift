import AVFoundation
import Foundation
import FoundationModels
import Speech

private struct HelperError: Error, CustomStringConvertible {
    let description: String
}

private func emit(_ value: [String: Any], status: Int32 = EXIT_SUCCESS) -> Never {
    let data = try! JSONSerialization.data(withJSONObject: value, options: [.sortedKeys])
    FileHandle.standardOutput.write(data)
    FileHandle.standardOutput.write(Data("\n".utf8))
    exit(status)
}

private func fail(_ message: String) -> Never {
    emit(["ok": false, "error": message], status: EXIT_FAILURE)
}

private func capabilities() async -> Never {
    let locale = Locale(identifier: "zh-CN")
    let modernLocale: String?
    let installedLocales: [String]
    let intelligence: String
    if #available(macOS 26.0, *) {
        modernLocale = await SpeechTranscriber.supportedLocale(equivalentTo: locale)?.identifier
        installedLocales = await SpeechTranscriber.installedLocales.map(\.identifier).sorted()
        intelligence = String(describing: SystemLanguageModel.default.availability)
    } else {
        modernLocale = nil
        installedLocales = []
        intelligence = "unavailable(osTooOld)"
    }
    emit([
        "ok": true,
        "apple_speech": [
            "modern_locale": modernLocale as Any,
            "installed_locales": installedLocales,
        ],
        "apple_intelligence": ["availability": intelligence],
    ])
}

@available(macOS 26.0, *)
private func transcribeModern(path: String, localeID: String) async throws -> String {
    let requested = Locale(identifier: localeID)
    guard let locale = await SpeechTranscriber.supportedLocale(equivalentTo: requested) else {
        throw HelperError(description: "SpeechTranscriber does not support locale \(localeID)")
    }
    let transcriber = SpeechTranscriber(locale: locale, preset: .transcription)
    let status = await AssetInventory.status(forModules: [transcriber])
    if status != .installed {
        guard let request = try await AssetInventory.assetInstallationRequest(supporting: [transcriber]) else {
            throw HelperError(description: "SpeechTranscriber asset is not installed and no download is available")
        }
        try await request.downloadAndInstall()
    }

    let audioFile = try AVAudioFile(forReading: URL(fileURLWithPath: path))
    async let transcript: AttributedString = try transcriber.results.reduce(AttributedString()) {
        partial, result in partial + result.text
    }
    let analyzer = SpeechAnalyzer(modules: [transcriber])
    if let lastSample = try await analyzer.analyzeSequence(from: audioFile) {
        try await analyzer.finalizeAndFinish(through: lastSample)
    } else {
        await analyzer.cancelAndFinishNow()
    }
    return String(try await transcript.characters)
}

private func transcribe(path: String, localeID: String) async -> Never {
    guard #available(macOS 26.0, *) else {
        fail("SpeechAnalyzer requires macOS 26 or newer")
    }
    do {
        emit(["ok": true, "text": try await transcribeModern(path: path, localeID: localeID)])
    } catch {
        fail("SpeechAnalyzer failed: \(error.localizedDescription)")
    }
}

@main
private enum Main {
    static func main() async {
        let args = CommandLine.arguments
        guard args.count >= 2 else { fail("usage: apple-speech-helper capabilities | transcribe <wav> <locale>") }
        switch args[1] {
        case "capabilities":
            await capabilities()
        case "transcribe":
            guard args.count == 4 else { fail("transcribe requires <wav> <locale>") }
            await transcribe(path: args[2], localeID: args[3])
        default:
            fail("unknown command: \(args[1])")
        }
    }
}
