import Dispatch
import Foundation
import FoundationModels
import Speech

// Read-only capability probe and future native-helper contract.
// It deliberately does not request authorization, download assets, or start
// recognition. S-Voice can use this JSON shape to decide whether to expose an
// Apple backend. Backend selection remains explicit; this does not enable an
// automatic fallback.
Task {
    let requested = Locale(identifier: "zh-CN")
    let speechLocale = await SpeechTranscriber.supportedLocale(equivalentTo: requested)
    let dictationLocale = await DictationTranscriber.supportedLocale(equivalentTo: requested)

    let result: [String: Any] = [
        "schema_version": 1,
        "requested_locale": requested.identifier,
        "speech_transcriber": [
            "available": SpeechTranscriber.isAvailable,
            "supported_locale": speechLocale?.identifier as Any,
            "supported_locale_count": await SpeechTranscriber.supportedLocales.count,
            "installed_locales": await SpeechTranscriber.installedLocales.map(\.identifier).sorted(),
        ],
        "dictation_transcriber": [
            "supported_locale": dictationLocale?.identifier as Any,
            "supported_locale_count": await DictationTranscriber.supportedLocales.count,
            "installed_locales": await DictationTranscriber.installedLocales.map(\.identifier).sorted(),
        ],
        "foundation_models": [
            "availability": String(describing: SystemLanguageModel.default.availability),
        ],
    ]

    do {
        let data = try JSONSerialization.data(withJSONObject: result, options: [.prettyPrinted, .sortedKeys])
        FileHandle.standardOutput.write(data)
        FileHandle.standardOutput.write(Data("\n".utf8))
        exit(EXIT_SUCCESS)
    } catch {
        FileHandle.standardError.write(Data("probe JSON error: \(error)\n".utf8))
        exit(EXIT_FAILURE)
    }
}

dispatchMain()
