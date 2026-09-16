import AVFAudio
import Testing

@testable import Kithara

@Suite("Interruption policy")
struct InterruptionPolicyDecides {
    @Test("An interruption that starts over playback pauses it and takes the permission")
    func beganOverPlaybackPausesAndArms() {
        let action = InterruptionPolicy.action(
            for: .began,
            options: [],
            isPlaying: true,
            armed: false
        )

        #expect(action == InterruptionAction(pause: true, armed: true))
    }

    /// The LABA-415 case: reading the live rate to arm made the second `began`
    /// store `false`, because the first had already paused playback.
    @Test("A repeated interruption-began keeps the permission it already holds")
    func repeatedBeganKeepsPermission() {
        let action = InterruptionPolicy.action(
            for: .began,
            options: [],
            isPlaying: false,
            armed: true
        )

        #expect(action == InterruptionAction(armed: true))
    }

    @Test("An interruption that starts over a paused player takes no permission")
    func beganOverPausedPlayerArmsNothing() {
        let action = InterruptionPolicy.action(
            for: .began,
            options: [],
            isPlaying: false,
            armed: false
        )

        #expect(action == InterruptionAction(armed: false))
    }

    @Test("An interruption that ends with permission rebuilds the output and resumes")
    func endedWithPermissionRebuildsAndResumes() {
        let action = InterruptionPolicy.action(
            for: .ended,
            options: .shouldResume,
            isPlaying: false,
            armed: true
        )

        #expect(action == InterruptionAction(rebuildOutput: true, resume: true, armed: false))
    }

    @Test("An interruption that ends without permission rebuilds the output and stays paused")
    func endedWithoutPermissionRebuildsAndStaysPaused() {
        let action = InterruptionPolicy.action(
            for: .ended,
            options: [],
            isPlaying: false,
            armed: true
        )

        #expect(action == InterruptionAction(rebuildOutput: true, armed: false))
    }

    /// The output belongs to the framework, and the interruption took it away
    /// whether or not anything resumes now — a later user `play()` needs a live
    /// stream just as much as an automatic resume does.
    @Test("An interruption that ends over a paused player still rebuilds the output")
    func endedWithoutPermissionHeldStillRebuilds() {
        let action = InterruptionPolicy.action(
            for: .ended,
            options: .shouldResume,
            isPlaying: false,
            armed: false
        )

        #expect(action == InterruptionAction(rebuildOutput: true, armed: false))
    }
}
