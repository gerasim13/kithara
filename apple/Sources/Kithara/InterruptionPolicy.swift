#if canImport(AVFoundation) && (os(iOS) || os(tvOS) || os(watchOS) || targetEnvironment(macCatalyst))
import AVFoundation

/// What one interruption notification asks the transport to do.
struct InterruptionAction: Equatable {
    /// Stop playback: the system has taken the output away.
    var pause = false
    /// Rebuild the native output stream before anything plays again.
    var rebuildOutput = false
    /// Resume playback on the player's own account.
    var resume = false
    /// Whether an interruption is still holding a resume permission.
    var armed = false
}

/// Decides what an interruption notification means, apart from carrying it out.
///
/// The decision is pure so the notification sequences iOS actually delivers can
/// be pinned directly. Carrying it out is covered where the effects live: the
/// stream rebuild in the session's restart path, the transport in playback.
enum InterruptionPolicy {
    static func action(
        for type: AVAudioSession.InterruptionType,
        options: AVAudioSession.InterruptionOptions,
        isPlaying: Bool,
        armed: Bool
    ) -> InterruptionAction {
        switch type {
        case .began:
            // iOS raises the interruption again without an intervening `ended`.
            // The permission belongs to the interruption, not to a single
            // notification, so a repeat neither arms nor disarms.
            guard isPlaying else {
                return InterruptionAction(armed: armed)
            }
            return InterruptionAction(pause: true, armed: true)
        case .ended:
            // The interruption took the native output away and the system does
            // not hand it back, so the rebuild is owed whether or not anything
            // resumes now: a later user `play()` needs a live output too.
            return InterruptionAction(
                rebuildOutput: true,
                resume: armed && options.contains(.shouldResume),
                armed: false
            )
        @unknown default:
            return InterruptionAction(armed: armed)
        }
    }
}
#endif
